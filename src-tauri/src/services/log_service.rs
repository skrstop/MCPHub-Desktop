use crate::{db, models::log::{ActivityEntry, ActivityPage, ActivityQuery, ActivityStats, LogEntry, LogQuery}};
use anyhow::Result;
use chrono::Local;
use sqlx::Row;
use uuid::Uuid;

pub async fn add_log(level: &str, message: &str, server_name: Option<&str>) -> Result<()> {
    let id = Uuid::new_v4().to_string();
    let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    sqlx::query(
        "INSERT INTO app_log (id, level, message, server_name, created_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(level)
    .bind(message)
    .bind(server_name)
    .bind(&now)
    .execute(db::pool())
    .await?;
    Ok(())
}

pub async fn query_logs(q: &LogQuery) -> Result<Vec<LogEntry>> {
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(50).min(200) as i64;
    let offset = ((page - 1) as i64) * page_size;

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

/// Write a single tool-call activity record to activity_log.
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
    let id = Uuid::new_v4().to_string();
    let input_str = input.map(|v| serde_json::to_string(&v)).transpose()?;
    let output_str = output.map(|v| serde_json::to_string(&v)).transpose()?;
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
    .bind(error_message)
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
        timestamp: r.try_get("timestamp")?,
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
    let page_size = q.page_size.unwrap_or(20).min(200) as i64;
    let offset = ((page - 1) as i64) * page_size;

    // Build a dynamic WHERE clause
    let mut conditions: Vec<&'static str> = Vec::new();
    if q.server.is_some() {
        conditions.push("server = ?");
    }
    if q.status.is_some() {
        conditions.push("status = ?");
    }
    if q.tool.is_some() {
        conditions.push("tool LIKE ?");
    }
    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    // Count query
    let count_sql = format!("SELECT COUNT(*) as cnt FROM activity_log {}", where_clause);
    let mut count_q = sqlx::query(&count_sql);
    if let Some(ref s) = q.server { count_q = count_q.bind(s); }
    if let Some(ref s) = q.status { count_q = count_q.bind(s); }
    if let Some(ref t) = q.tool { count_q = count_q.bind(format!("%{}%", t)); }
    let total: i64 = count_q.fetch_one(db::pool()).await?.try_get("cnt")?;

    // Data query
    let data_sql = format!(
        "SELECT id, timestamp, server, tool, duration_ms, status, input, output, \
         group_name, key_id, key_name, error_message, source_ip FROM activity_log {} \
         ORDER BY timestamp DESC LIMIT ? OFFSET ?",
        where_clause
    );
    let mut data_q = sqlx::query(&data_sql);
    if let Some(ref s) = q.server { data_q = data_q.bind(s); }
    if let Some(ref s) = q.status { data_q = data_q.bind(s); }
    if let Some(ref t) = q.tool { data_q = data_q.bind(format!("%{}%", t)); }
    data_q = data_q.bind(page_size).bind(offset);
    let rows = data_q.fetch_all(db::pool()).await?;
    let data: Vec<ActivityEntry> = rows.iter().map(row_to_activity).collect::<Result<_>>()?;

    Ok(ActivityPage { data, page, page_size: page_size as u32, total })
}

pub async fn get_activity_stats() -> Result<ActivityStats> {
    let row = sqlx::query(
        "SELECT \
           COUNT(*) as total, \
           SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END) as success, \
           SUM(CASE WHEN status = 'error' THEN 1 ELSE 0 END) as error, \
           COALESCE(AVG(duration_ms), 0) as avg_duration \
         FROM activity_log",
    )
    .fetch_one(db::pool())
    .await?;
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
pub async fn clear_activities() -> Result<()> {
    sqlx::query("DELETE FROM activity_log")
        .execute(db::pool())
        .await?;
    // Reclaim disk space after bulk delete
    let _ = sqlx::raw_sql("VACUUM").execute(db::pool()).await;
    Ok(())
}

/// Delete all application log entries and vacuum.
pub async fn clear_logs() -> Result<()> {
    sqlx::query("DELETE FROM app_log")
        .execute(db::pool())
        .await?;
    // Reclaim disk space after bulk delete
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
/// 2. Deletes app_log entries older than 15 days
/// 3. Deletes activity_log entries older than 15 days
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

    // Delete old app_log entries (uses created_at column)
    let app_log_sql = format!(
        "DELETE FROM app_log WHERE created_at < {}",
        cutoff
    );
    let app_result = sqlx::query(&app_log_sql)
        .execute(db::pool())
        .await?;
    let app_deleted = app_result.rows_affected() as i64;

    // Delete old activity_log entries (uses created_at column)
    let activity_sql = format!(
        "DELETE FROM activity_log WHERE created_at < {}",
        cutoff
    );
    let activity_result = sqlx::query(&activity_sql)
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
