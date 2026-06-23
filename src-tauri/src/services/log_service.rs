use crate::{db, models::log::{ActivityEntry, ActivityPage, ActivityQuery, ActivityStats, LogEntry, LogQuery}};
use anyhow::Result;
use sqlx::Row;
use uuid::Uuid;

pub async fn add_log(level: &str, message: &str, server_name: Option<&str>) -> Result<()> {
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO app_log (id, level, message, server_name) VALUES (?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(level)
    .bind(message)
    .bind(server_name)
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
) -> Result<()> {
    let id = Uuid::new_v4().to_string();
    let input_str = input.map(|v| serde_json::to_string(&v)).transpose()?;
    let output_str = output.map(|v| serde_json::to_string(&v)).transpose()?;
    sqlx::query(
        "INSERT INTO activity_log (id, server, tool, duration_ms, status, input, output, error_message) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(server)
    .bind(tool)
    .bind(duration_ms)
    .bind(status)
    .bind(&input_str)
    .bind(&output_str)
    .bind(error_message)
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
         group_name, key_id, key_name, error_message FROM activity_log {} \
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

/// Delete all activity log entries.
pub async fn clear_activities() -> Result<()> {
    sqlx::query("DELETE FROM activity_log")
        .execute(db::pool())
        .await?;
    Ok(())
}

/// Delete all application log entries.
pub async fn clear_logs() -> Result<()> {
    sqlx::query("DELETE FROM app_log")
        .execute(db::pool())
        .await?;
    Ok(())
}

/// Retention period for logs (days).
const LOG_RETENTION_DAYS: i64 = 15;

/// Clean up logs older than the retention period and vacuum the database.
///
/// This function:
/// 1. Deletes app_log entries older than 15 days
/// 2. Deletes activity_log entries older than 15 days
/// 3. Runs VACUUM to reclaim disk space
///
/// Returns (app_log_deleted, activity_log_deleted, vacuum_done).
pub async fn cleanup_old_logs() -> Result<(i64, i64, bool)> {
    let cutoff = format!("datetime('now', '-{} days')", LOG_RETENTION_DAYS);

    // Delete old app_log entries
    let app_log_sql = format!(
        "DELETE FROM app_log WHERE created_at < {}",
        cutoff
    );
    let app_result = sqlx::query(&app_log_sql)
        .execute(db::pool())
        .await?;
    let app_deleted = app_result.rows_affected() as i64;

    // Delete old activity_log entries
    let activity_sql = format!(
        "DELETE FROM activity_log WHERE created_at < {}",
        cutoff
    );
    let activity_result = sqlx::query(&activity_sql)
        .execute(db::pool())
        .await?;
    let activity_deleted = activity_result.rows_affected() as i64;

    // Run VACUUM to reclaim disk space
    let vacuum_done = if app_deleted > 0 || activity_deleted > 0 {
        match sqlx::raw_sql("VACUUM")
            .execute(db::pool())
            .await
        {
            Ok(_) => {
                log::info!(
                    "[log_cleanup] VACUUM completed (deleted {} app_log, {} activity_log)",
                    app_deleted, activity_deleted
                );
                true
            }
            Err(e) => {
                log::warn!("[log_cleanup] VACUUM failed: {}", e);
                false
            }
        }
    } else {
        true // nothing to vacuum
    };

    Ok((app_deleted, activity_deleted, vacuum_done))
}

/// Run log cleanup and return a summary message.
pub async fn run_cleanup_with_summary() -> String {
    match cleanup_old_logs().await {
        Ok((app_deleted, activity_deleted, vacuum_done)) => {
            let vacuum_status = if vacuum_done { "done" } else { "failed" };
            format!(
                "Cleanup completed: {} app_log, {} activity_log deleted (retention={}d), vacuum={}",
                app_deleted, activity_deleted, LOG_RETENTION_DAYS, vacuum_status
            )
        }
        Err(e) => {
            format!("Cleanup failed: {}", e)
        }
    }
}
