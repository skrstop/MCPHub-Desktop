use crate::{db, models::log::{ActivityEntry, ActivityPage, ActivityQuery, ActivityStats, LogEntry, LogQuery}, services::config_service};
use anyhow::{anyhow, Result};
use chrono::Local;
use sqlx::Row;
use uuid::Uuid;

pub async fn add_log(level: &str, message: &str, server_name: Option<&str>) -> Result<()> {
    // log_event is unauthenticated — cap the message so a rogue frontend
    // cannot bloat app_log + fts_app_log with unbounded writes (same policy
    // as app_logger's 4000-char cap).
    const MAX_MESSAGE_CHARS: usize = 4000;
    let message = if message.chars().count() > MAX_MESSAGE_CHARS {
        let mut end = MAX_MESSAGE_CHARS;
        while !message.is_char_boundary(end) {
            end += 1;
        }
        &message[..end]
    } else {
        message
    };
    let id = Uuid::new_v4().to_string();
    let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let mut tx = db::pool().begin().await?;
    sqlx::query(
        "INSERT INTO app_log (id, level, message, server_name, created_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(level)
    .bind(message)
    .bind(server_name)
    .bind(&now)
    .execute(&mut *tx)
    .await?;
    // FTS 同步（§4.3 铁律：同事务）。高频写路径，tokenize 单条消息开销 <0.1ms。
    // Insert-only fast path: `id` is a fresh UUID — the upsert variant would
    // full-scan the UNINDEXED ref_id column on every log write (O(N) with log
    // volume; logging slowing down logging).
    crate::services::fts_service::sync_insert_tx(
        &mut tx,
        crate::services::fts_service::FtsTable::AppLog,
        &id,
        message,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn query_logs(q: &LogQuery) -> Result<Vec<LogEntry>> {
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(50).clamp(1, 200) as i64;
    let offset = (((page - 1) as i64) * page_size).max(0);

    // 全文搜索（F1d）：FTS5 中/英/拼音分词 + rank 排序；空关键词/空表/Err 降级原全量路径
    let search_key = q.search.as_deref().map(str::trim).unwrap_or("");
    if !search_key.is_empty() {
        match crate::services::fts_service::search_ref_ids_weighted(
            crate::services::fts_service::FtsTable::AppLog,
            search_key,
            5000,
        )
        .await
        {
            Ok(weighted) if !weighted.is_empty() => {
                // 命中词数 map：相关度第一优先，同数保持 created_at DESC（稳定排序）
                let counts: std::collections::HashMap<String, i64> =
                    weighted.into_iter().collect();
                // 回表 + level/server_name 过滤（原生 created_at DESC 序）
                // 条件必须拼在 ORDER BY 之前（此前 ORDER BY 先入队，
                // 再追加 AND 生成 "... ORDER BY created_at DESC AND level = ?" 非法 SQL）。
                // 分块 bind：SQLITE_MAX_VARIABLE_NUMBER 老上限 999，
                // 全量一次 bind 超限会报 too many parameters。
                let ids: Vec<&String> = counts.keys().collect();
                let mut ranked: Vec<LogEntry> = Vec::new();
                for chunk in ids.chunks(500) {
                    let mut qb = sqlx::QueryBuilder::new(
                        "SELECT id, level, message, server_name, created_at FROM app_log WHERE id IN (",
                    );
                    let mut first = true;
                    for id in chunk {
                        qb.push(if first { "" } else { ", " });
                        qb.push_bind((*id).clone());
                        first = false;
                    }
                    qb.push(")");
                    if let Some(level) = &q.level {
                        qb.push(" AND level = ").push_bind(level);
                    }
                    if let Some(server) = &q.server_name {
                        qb.push(" AND server_name = ").push_bind(server);
                    }
                    qb.push(" ORDER BY created_at DESC");
                    let rows = qb.build().fetch_all(db::pool()).await?;
                    let entries: Vec<LogEntry> = rows
                        .into_iter()
                        .map(|r| {
                            Ok(LogEntry {
                                id: r.try_get("id")?,
                                level: r.try_get("level")?,
                                message: r.try_get("message")?,
                                server_name: r.try_get("server_name")?,
                                created_at: r.try_get("created_at")?,
                            })
                        })
                        .collect::<Result<_>>()?;
                    ranked.extend(entries);
                }
                // Relevance first, then created_at DESC as an explicit
                // tiebreak. The chunked back-fill does NOT preserve a global
                // created_at order across chunks (ids come from a HashMap,
                // chunk-internal ORDER BY only), so the tiebreak must be in
                // the sort key — otherwise same-score entries paginate in
                // HashMap order (unstable across identical queries).
                ranked.sort_by(|a, b| {
                    let ca = counts.get(&a.id).copied().unwrap_or(0);
                    let cb = counts.get(&b.id).copied().unwrap_or(0);
                    cb.cmp(&ca)
                        .then_with(|| b.created_at.cmp(&a.created_at))
                        .then_with(|| b.id.cmp(&a.id))
                });
                let start = (offset.max(0)) as usize;
                return Ok(ranked
                    .into_iter()
                    .skip(start)
                    .take(page_size as usize)
                    .collect());
            }
            Ok(_)
                if crate::services::fts_service::table_is_empty(
                    crate::services::fts_service::FtsTable::AppLog,
                )
                .await
                .unwrap_or(false) =>
            {
                // 空表兜底（启动对账前）：走原 LIKE
                return like_search_logs(page_size, offset, search_key, q.level.as_ref(), q.server_name.as_ref()).await;
            }
            Ok(_) => {
                // 零结果降级 LIKE：FTS 词前缀查不到的 CJK 内部子串仍可命中
                return like_search_logs(page_size, offset, search_key, q.level.as_ref(), q.server_name.as_ref()).await;
            }
            Err(e) => {
                log::warn!("[fts] search app_log failed, fallback to LIKE: {e}");
                return like_search_logs(page_size, offset, search_key, q.level.as_ref(), q.server_name.as_ref()).await;
            }
        }
    }

    let rows = sqlx::query(
        "SELECT id, level, message, server_name, created_at FROM app_log
         ORDER BY created_at DESC LIMIT ? OFFSET ?",
    )
    .bind(page_size)
    .bind(offset)
    .fetch_all(db::pool())
    .await?;

    rows.into_iter()
        .map(|r| {
            Ok(LogEntry {
                id: r.try_get("id")?,
                level: r.try_get("level")?,
                message: r.try_get("message")?,
                server_name: r.try_get("server_name")?,
                created_at: r.try_get("created_at")?,
            })
        })
        .collect()
}

/// FTS 降级路径：message LIKE（§4.4 Err/空表兜底）。level/server_name 过滤必须
/// 与 FTS 路径同语义应用——降级丢弃过滤器会让「按级别筛选 + 搜索」返回混级结果。
async fn like_search_logs(
    page_size: i64,
    offset: i64,
    search_key: &str,
    level: Option<&String>,
    server_name: Option<&String>,
) -> Result<Vec<LogEntry>> {
    let pattern = format!(
        "%{}%",
        search_key.to_lowercase().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
    );
    // Build the query with optional filters:
    let mut qb = sqlx::QueryBuilder::new(
        "SELECT id, level, message, server_name, created_at FROM app_log WHERE LOWER(message) LIKE ",
    );
    qb.push_bind(pattern.clone());
    qb.push(" ESCAPE '\\'");
    if let Some(l) = level {
        qb.push(" AND level = ").push_bind(l.clone());
    }
    if let Some(srv) = server_name {
        qb.push(" AND server_name = ").push_bind(srv.clone());
    }
    qb.push(" ORDER BY created_at DESC LIMIT ? OFFSET ?");
    let rows = qb
    .build()
    .bind(page_size)
    .bind(offset)
    .fetch_all(db::pool())
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(LogEntry {
                id: r.try_get("id")?,
                level: r.try_get("level")?,
                message: r.try_get("message")?,
                server_name: r.try_get("server_name")?,
                created_at: r.try_get("created_at")?,
            })
        })
        .collect()
}

/// Write a single tool-call activity record to activity_log.
///
/// When `activityLog.storeToolPayload` is `false` in system config,
/// the `input` and `output` fields are stored as NULL to avoid
/// persisting potentially sensitive tool arguments/results.
pub async fn write_activity(
    server: &str,
    tool: &str,
    duration_ms: Option<i64>,
    status: &str,
    input: Option<serde_json::Value>,
    output: Option<serde_json::Value>,
    error_message: Option<&str>,
    source_ip: Option<&str>,
) -> Result<()> {
    // Check storeToolPayload config — default to true (store everything)
    let store_payload = config_service::get()
        .await
        .ok()
        .and_then(|c| {
            c.get("activityLog")
                .and_then(|al| al.get("storeToolPayload"))
                .and_then(|v| v.as_bool())
        })
        .unwrap_or(true);

    let id = Uuid::new_v4().to_string();
    const MAX_PAYLOAD_BYTES: usize = 64 * 1024; // 64KB per field
    let truncate_payload = |s: String| -> String {
        if s.len() <= MAX_PAYLOAD_BYTES {
            return s;
        }
        // Byte-cap with char-boundary alignment (payload is user/tool data,
        // may be arbitrary UTF-8).
        let mut end = MAX_PAYLOAD_BYTES;
        while !s.is_char_boundary(end) {
            end += 1;
        }
        format!("{}\n…(truncated)", &s[..end])
    };
    let (input_str, output_str) = if store_payload {
        (
            input
                .map(|v| serde_json::to_string(&v))
                .transpose()?
                .map(truncate_payload),
            output
                .map(|v| serde_json::to_string(&v))
                .transpose()?
                .map(truncate_payload),
        )
    } else {
        (None, None)
    };
    sqlx::query(
        "INSERT INTO activity_log (id, server, tool, duration_ms, status, input, output, error_message, source_ip) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(server)
    .bind(tool)
    .bind(duration_ms)
    .bind(status)
    .bind(&input_str)
    .bind(&output_str)
    .bind(error_message.map(|e| {
        // Error chains can embed upstream payloads (e.g. the 32KB stderr
        // tail) — cap for parity with input/output.
        if e.len() <= MAX_PAYLOAD_BYTES {
            e.to_string()
        } else {
            let mut end = MAX_PAYLOAD_BYTES;
            while !e.is_char_boundary(end) {
                end += 1;
            }
            format!("{}\n…(truncated)", &e[..end])
        }
    }))
    .bind(source_ip)
    .execute(db::pool())
    .await?;
    Ok(())
}

fn row_to_activity(r: &sqlx::sqlite::SqliteRow) -> Result<ActivityEntry> {
    let input_str: Option<String> = r.try_get("input")?;
    let output_str: Option<String> = r.try_get("output")?;
    Ok(ActivityEntry {
        id: r.try_get("id")?,
        created_at: r.try_get("created_at")?,
        server: r.try_get("server")?,
        tool: r.try_get("tool")?,
        duration_ms: r.try_get("duration_ms")?,
        status: r.try_get("status")?,
        input: input_str.and_then(|s| serde_json::from_str(&s).ok()),
        output: output_str.and_then(|s| serde_json::from_str(&s).ok()),
        group_name: r.try_get("group_name")?,
        key_id: r.try_get("key_id")?,
        key_name: r.try_get("key_name")?,
        error_message: r.try_get("error_message")?,
        source_ip: r.try_get("source_ip").ok().flatten(),
    })
}

pub async fn query_tool_activities(q: &ActivityQuery) -> Result<ActivityPage> {
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).clamp(1, 200) as i64;
    let offset = (((page - 1) as i64) * page_size).max(0);

    // Build a dynamic WHERE clause
    let mut conditions: Vec<&'static str> = Vec::new();
    if q.server.is_some() {
        conditions.push("server = ?");
    }
    if q.status.is_some() {
        conditions.push("status = ?");
    }
    if q.tool.is_some() {
        conditions.push("tool LIKE ? ESCAPE '\\'");
    }
    if q.group_name.is_some() {
        conditions.push("group_name = ?");
    }
    if q.key_name.is_some() {
        conditions.push("key_name = ?");
    }
    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    // Count query
    let count_sql = format!("SELECT COUNT(*) as cnt FROM activity_log {}", where_clause);
    let mut count_q = sqlx::query(sqlx::AssertSqlSafe(&*count_sql));
    if let Some(ref s) = q.server { count_q = count_q.bind(s); }
    if let Some(ref s) = q.status { count_q = count_q.bind(s); }
    if let Some(ref t) = q.tool {
        // Escape LIKE wildcards (tool names commonly contain `_`)
        count_q = count_q.bind(format!(
            "%{}%",
            t.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
        ));
    }
    if let Some(ref g) = q.group_name { count_q = count_q.bind(g); }
    if let Some(ref k) = q.key_name { count_q = count_q.bind(k); }
    let total: i64 = count_q.fetch_one(db::pool()).await?.try_get("cnt")?;

    // Data query
    let data_sql = format!(
        "SELECT id, created_at, server, tool, duration_ms, status, input, output, \
         group_name, key_id, key_name, error_message, source_ip FROM activity_log {} \
         ORDER BY created_at DESC LIMIT ? OFFSET ?",
        where_clause
    );
    let mut data_q = sqlx::query(sqlx::AssertSqlSafe(&*data_sql));
    if let Some(ref s) = q.server { data_q = data_q.bind(s); }
    if let Some(ref s) = q.status { data_q = data_q.bind(s); }
    if let Some(ref t) = q.tool {
        data_q = data_q.bind(format!(
            "%{}%",
            t.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
        ));
    }
    if let Some(ref g) = q.group_name { data_q = data_q.bind(g); }
    if let Some(ref k) = q.key_name { data_q = data_q.bind(k); }
    data_q = data_q.bind(page_size).bind(offset);
    let rows = data_q.fetch_all(db::pool()).await?;
    let data: Vec<ActivityEntry> = rows.iter().map(row_to_activity).collect::<Result<_>>()?;

    Ok(ActivityPage { data, page, page_size: page_size as u32, total })
}

/// 筛选候选字段白名单（G2：field 枚举 → 列名，杜绝注入）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityFilterField {
    Server,
    Tool,
    Group,
    KeyName,
}

impl ActivityFilterField {
    pub fn parse(field: &str) -> Option<Self> {
        match field {
            "server" => Some(Self::Server),
            "tool" => Some(Self::Tool),
            "group" => Some(Self::Group),
            "keyName" => Some(Self::KeyName),
            _ => None,
        }
    }

    fn column(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::Tool => "tool",
            Self::Group => "group_name",
            Self::KeyName => "key_name",
        }
    }
}

/// 分页可搜索筛选候选（G2）：DISTINCT 列值 + LIKE 前缀过滤 + 分页。
/// 供活动日志页的 SearchableSelect 下拉框调用（§8）。
pub async fn get_activity_filter_options(
    field: &str,
    search: Option<&str>,
    page: u32,
    page_size: u32,
) -> Result<crate::models::log::ActivityFilterOptionsPage> {
    use crate::models::log::ActivityFilterOptionsPage;

    let field = ActivityFilterField::parse(field)
        .ok_or_else(|| anyhow!("invalid filter field '{}'", field))?;
    let page = page.max(1);
    let page_size = page_size.clamp(1, 100);
    let offset = ((page - 1) as i64) * (page_size as i64);

    let col = field.column();
    let key = search.unwrap_or("").trim().to_lowercase();

    let (count_sql, data_sql): (String, String) = if key.is_empty() {
        (
            format!("SELECT COUNT(*) FROM (SELECT DISTINCT {col} AS v FROM activity_log WHERE {col} IS NOT NULL AND {col} != '')"),
            format!(
                "SELECT DISTINCT {col} AS v FROM activity_log \
                 WHERE {col} IS NOT NULL AND {col} != '' \
                 ORDER BY v LIMIT ? OFFSET ?"
            ),
        )
    } else {
        (
            format!(
                "SELECT COUNT(*) FROM (SELECT DISTINCT {col} AS v FROM activity_log \
                 WHERE {col} IS NOT NULL AND {col} != '' AND LOWER({col}) LIKE ? ESCAPE '\\')"
            ),
            format!(
                "SELECT DISTINCT {col} AS v FROM activity_log \
                 WHERE {col} IS NOT NULL AND {col} != '' AND LOWER({col}) LIKE ? ESCAPE '\\' \
                 ORDER BY v LIMIT ? OFFSET ?"
            ),
        )
    };

    let total: i64 = if key.is_empty() {
        sqlx::query_scalar(sqlx::AssertSqlSafe(&*count_sql))
            .fetch_one(db::pool())
            .await?
    } else {
        let pattern = format!("%{}%", key.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        sqlx::query_scalar(sqlx::AssertSqlSafe(&*count_sql))
            .bind(&pattern)
            .fetch_one(db::pool())
            .await?
    };

    let rows = if key.is_empty() {
        sqlx::query(sqlx::AssertSqlSafe(&*data_sql))
            .bind(page_size as i64)
            .bind(offset)
            .fetch_all(db::pool())
            .await?
    } else {
        let pattern = format!("%{}%", key.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        sqlx::query(sqlx::AssertSqlSafe(&*data_sql))
            .bind(&pattern)
            .bind(page_size as i64)
            .bind(offset)
            .fetch_all(db::pool())
            .await?
    };

    let options: Vec<String> = rows
        .iter()
        .filter_map(|r| r.try_get::<Option<String>, _>("v").ok().flatten())
        .collect();

    Ok(ActivityFilterOptionsPage {
        options,
        total: total.max(0),
        page,
        page_size,
    })
}

pub async fn get_activity_stats(
    server: Option<&str>,
    status: Option<&str>,
    tool: Option<&str>,
    group_name: Option<&str>,
    key_name: Option<&str>,
) -> Result<ActivityStats> {
    // Build optional WHERE clause from filters
    let mut conditions: Vec<String> = Vec::new();
    if server.is_some() { conditions.push("server = ?".into()); }
    if status.is_some() { conditions.push("status = ?".into()); }
    if tool.is_some() { conditions.push("tool LIKE ? ESCAPE '\\'".into()); }
    if group_name.is_some() { conditions.push("group_name = ?".into()); }
    if key_name.is_some() { conditions.push("key_name = ?".into()); }
    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let sql = format!(
        "SELECT \
           COUNT(*) as total, \
           SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END) as success, \
           SUM(CASE WHEN status = 'error' THEN 1 ELSE 0 END) as error, \
           COALESCE(AVG(duration_ms), 0) as avg_duration \
         FROM activity_log {}",
        where_clause
    );
    let mut q = sqlx::query(sqlx::AssertSqlSafe(&*sql));
    if let Some(s) = server { q = q.bind(s); }
    if let Some(s) = status { q = q.bind(s); }
    if let Some(t) = tool {
        q = q.bind(format!("%{}%", t.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")));
    }
    if let Some(g) = group_name { q = q.bind(g); }
    if let Some(k) = key_name { q = q.bind(k); }
    let row = q.fetch_one(db::pool()).await?;
    Ok(ActivityStats {
        total: row.try_get("total")?,
        success: row.try_get::<Option<i64>, _>("success")?.unwrap_or(0),
        error: row.try_get::<Option<i64>, _>("error")?.unwrap_or(0),
        avg_duration: row.try_get::<Option<f64>, _>("avg_duration")?.unwrap_or(0.0),
    })
}

/// Returns a distinct list of server names that appear in activity_log (for filter UI).
pub async fn get_activity_filters() -> Result<Vec<String>> {
    let rows = sqlx::query(
        "SELECT DISTINCT server FROM activity_log WHERE server != '' ORDER BY server",
    )
    .fetch_all(db::pool())
    .await?;
    rows.into_iter().map(|r| Ok(r.try_get("server")?)).collect()
}

/// Delete all activity log entries and vacuum.
/// Returns the number of deleted rows.
pub async fn clear_activities() -> Result<i64> {
    let result = sqlx::query("DELETE FROM activity_log")
        .execute(db::pool())
        .await?;
    let deleted = result.rows_affected() as i64;
    // Reclaim disk space after bulk delete
    let _ = sqlx::raw_sql("VACUUM").execute(db::pool()).await;
    Ok(deleted)
}

/// Delete activity log entries older than `days_old` days and vacuum.
/// Returns the number of deleted rows and the cutoff date string.
pub async fn cleanup_by_days(days_old: i64) -> Result<(i64, String)> {
    // Clamp：负数会把 cutoff 推到未来 → 清空全部日志；上限防极端值
    let days_old = days_old.clamp(1, 3650);
    let cutoff = format!("datetime('now', 'localtime', '-{} days')", days_old);
    let sql = format!("DELETE FROM activity_log WHERE created_at < {}", cutoff);
    let result = sqlx::query(sqlx::AssertSqlSafe(&*sql)).execute(db::pool()).await?;
    let deleted = result.rows_affected() as i64;
    if deleted > 0 {
        let _ = sqlx::raw_sql("VACUUM").execute(db::pool()).await;
    }
    // Read back the actual cutoff datetime for the response
    let cutoff_row = sqlx::query(sqlx::AssertSqlSafe(&*format!("SELECT {} as c", cutoff)))
        .fetch_one(db::pool())
        .await?;
    let cutoff_date: String = cutoff_row.try_get("c")?;
    Ok((deleted, cutoff_date))
}

/// Delete all application log entries and vacuum.
pub async fn clear_logs() -> Result<()> {
    let mut tx = db::pool().begin().await?;
    sqlx::query("DELETE FROM app_log")
        .execute(&mut *tx)
        .await?;
    // FTS 联动（§4.3 铁律）：必须与源表删除同事务（同一连接）执行。
    // 若用 pool 版 clear_table（另一连接），tx 已持有 SQLite 写锁，
    // 第二个连接的 DELETE 会等锁直至 busy_timeout（5s）后报 database is locked —— 死锁。
    crate::services::fts_service::clear_table_tx(&mut tx, crate::services::fts_service::FtsTable::AppLog)
        .await?;
    tx.commit().await?;
    // Reclaim disk space after bulk delete (VACUUM 不能在事务内，单独执行)
    let _ = sqlx::raw_sql("VACUUM").execute(db::pool()).await;
    Ok(())
}

/// Retention period for logs (days).
const LOG_RETENTION_DAYS: i64 = 15;

/// Get the database file size in bytes by querying SQLite page count and page size.
async fn get_db_size() -> u64 {
    let page_count: i64 = sqlx::query_scalar("SELECT page_count FROM pragma_page_count()")
        .fetch_one(db::pool())
        .await
        .unwrap_or(0);
    let page_size: i64 = sqlx::query_scalar("SELECT page_size FROM pragma_page_size()")
        .fetch_one(db::pool())
        .await
        .unwrap_or(4096);
    (page_count * page_size).max(0) as u64
}

/// Format bytes to human readable string (KB/MB/GB).
fn format_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.2} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

/// Clean up logs older than the retention period and vacuum the database.
///
/// This function:
/// 1. Gets DB size before cleanup
/// 2. Deletes app_log entries older than 15 days (uses created_at column)
/// 3. Deletes activity_log entries older than 15 days (uses created_at column)
/// 4. Runs VACUUM to reclaim disk space
/// 5. Gets DB size after cleanup
///
/// Returns (app_log_deleted, activity_log_deleted, vacuum_done, size_before, size_after).
pub async fn cleanup_old_logs() -> Result<(i64, i64, bool, u64, u64)> {
    let cutoff = format!("datetime('now', 'localtime', '-{} days')", LOG_RETENTION_DAYS);

    // Get DB size before cleanup
    let size_before = get_db_size().await;
    log::info!("[log_cleanup] DB size before cleanup: {}", format_size(size_before));

    // Count entries before deletion
    let app_log_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM app_log")
        .fetch_one(db::pool())
        .await
        .unwrap_or(0);
    let activity_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM activity_log")
        .fetch_one(db::pool())
        .await
        .unwrap_or(0);
    log::info!("[log_cleanup] Current entries: app_log={}, activity_log={}", app_log_total, activity_total);

    // Delete old app_log entries + FTS 联动（§4.3 铁律：先取待删 id 集 →
    // 同事务删 app_log + 删 fts_app_log——FTS5 无范围删除，按 ref_id→rowid 两步删）
    let ids_sql = format!(
        "SELECT id FROM app_log WHERE created_at < {}",
        cutoff
    );
    let stale_ids: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(&*ids_sql))
        .fetch_all(db::pool())
        .await
        // Propagate: swallowing this error would DELETE source rows while
        // leaving their fts_app_log counterparts forever (permanent FTS
        // orphans) — the exact §4.3 violation this two-step delete exists
        // to prevent. A failed cleanup retries on the next cycle instead.
        ?;

    let mut tx = db::pool().begin().await?;
    let app_log_sql = format!(
        "DELETE FROM app_log WHERE created_at < {}",
        cutoff
    );
    let app_result = sqlx::query(sqlx::AssertSqlSafe(&*app_log_sql))
        .execute(&mut *tx)
        .await?;
    let app_deleted = app_result.rows_affected() as i64;

    if !stale_ids.is_empty() {
        // FTS5 无范围删除：rowid IN (SELECT rowid WHERE ref_id IN (...ids...))
        // 分批删除：SQLite 变量上限 32766，极端积压下一次性 bind 可能超限
        for chunk in stale_ids.chunks(1000) {
            let mut qb = sqlx::QueryBuilder::new(
                "DELETE FROM fts_app_log WHERE rowid IN (SELECT rowid FROM fts_app_log WHERE ref_id IN (",
            );
            let mut sep = qb.separated(", ");
            for id in chunk {
                sep.push_bind(id);
            }
            qb.push("))");
            qb.build().execute(&mut *tx).await?;
        }
    }
    tx.commit().await?;

    // Delete old activity_log entries (uses created_at column)
    let activity_sql = format!(
        "DELETE FROM activity_log WHERE created_at < {}",
        cutoff
    );
    let activity_result = sqlx::query(sqlx::AssertSqlSafe(&*activity_sql))
        .execute(db::pool())
        .await?;
    let activity_deleted = activity_result.rows_affected() as i64;

    log::info!(
        "[log_cleanup] Deleted: app_log={}, activity_log={} (retention={}d)",
        app_deleted, activity_deleted, LOG_RETENTION_DAYS
    );

    // Run VACUUM to reclaim disk space
    let (vacuum_done, size_after) = if app_deleted > 0 || activity_deleted > 0 {
        match sqlx::raw_sql("VACUUM")
            .execute(db::pool())
            .await
        {
            Ok(_) => {
                let size = get_db_size().await;
                log::info!(
                    "[log_cleanup] VACUUM completed: {} -> {}",
                    format_size(size_before), format_size(size)
                );
                (true, size)
            }
            Err(e) => {
                log::warn!("[log_cleanup] VACUUM failed: {}", e);
                (false, size_before)
            }
        }
    } else {
        log::info!("[log_cleanup] No old entries to delete, skipping VACUUM");
        (true, size_before)
    };

    // Trim on-disk daily log files with the same retention as the DB, so file
    // and DB logs age out together (file mirror written by app_logger).
    crate::services::app_logger::cleanup_old_log_files(LOG_RETENTION_DAYS);

    Ok((app_deleted, activity_deleted, vacuum_done, size_before, size_after))
}

/// Run log cleanup and return a summary message.
/// Logs the result to both stderr and database.
pub async fn run_cleanup_with_summary() -> String {
    match cleanup_old_logs().await {
        Ok((app_deleted, activity_deleted, vacuum_done, size_before, size_after)) => {
            let vacuum_status = if vacuum_done { "done" } else { "failed" };
            let msg = format!(
                "Log cleanup: deleted {} app_log + {} activity_log (retention={}d), DB {} -> {}, vacuum={}",
                app_deleted, activity_deleted, LOG_RETENTION_DAYS,
                format_size(size_before), format_size(size_after), vacuum_status
            );
            log::info!("[log_cleanup] {}", msg);
            // Write to database so it shows in the app's log view
            crate::services::app_logger::log_to_db("info", &format!("[log_cleanup] {}", msg));
            msg
        }
        Err(e) => {
            let msg = format!("Log cleanup failed: {}", e);
            log::warn!("[log_cleanup] {}", msg);
            crate::services::app_logger::log_to_db("warn", &format!("[log_cleanup] {}", msg));
            msg
        }
    }
}
