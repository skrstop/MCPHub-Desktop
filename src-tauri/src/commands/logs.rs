use crate::{
    models::log::{ActivityPage, ActivityQuery, ActivityStats, LogEntry, LogQuery},
    services::log_service,
};

#[tauri::command]
pub async fn get_logs(query: LogQuery) -> Result<Vec<LogEntry>, String> {
    log_service::query_logs(&query)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn clear_logs() -> Result<(), String> {
    log_service::clear_logs().await.map_err(|e| e.to_string())
}

/// Returns { available: true } — activity logging is always on in the desktop app.
#[tauri::command]
pub async fn get_activity_available() -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({ "available": true }))
}

/// Returns a list of distinct server names seen in activity_log (for filter dropdowns).
#[tauri::command]
pub async fn get_activity_filters() -> Result<Vec<String>, String> {
    log_service::get_activity_filters()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_activity_stats(
    server: Option<String>,
    status: Option<String>,
    tool: Option<String>,
) -> Result<ActivityStats, String> {
    log_service::get_activity_stats(
        server.as_deref(),
        status.as_deref(),
        tool.as_deref(),
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_tool_activities(
    page: Option<u32>,
    page_size: Option<u32>,
    server: Option<String>,
    status: Option<String>,
    tool: Option<String>,
) -> Result<ActivityPage, String> {
    let q = ActivityQuery { page, page_size, server, status, tool };
    log_service::query_tool_activities(&q)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn clear_tool_activities() -> Result<serde_json::Value, String> {
    let deleted = log_service::clear_activities()
        .await
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "deletedCount": deleted,
    }))
}

/// Manually trigger log cleanup: delete entries older than 15 days and VACUUM.
#[tauri::command]
pub async fn cleanup_old_logs() -> Result<serde_json::Value, String> {
    let (app_deleted, activity_deleted, vacuum_done, size_before, size_after) = log_service::cleanup_old_logs()
        .await
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "appLogDeleted": app_deleted,
        "activityLogDeleted": activity_deleted,
        "vacuumDone": vacuum_done,
        "sizeBefore": size_before,
        "sizeAfter": size_after,
    }))
}

/// Delete activity log entries older than `days_old` days.
/// Returns { deletedCount, cutoffDate }.
#[tauri::command]
pub async fn cleanup_activity_logs(days_old: Option<i64>) -> Result<serde_json::Value, String> {
    let days = days_old.unwrap_or(30);
    let (deleted, cutoff_date) = log_service::cleanup_by_days(days)
        .await
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "deletedCount": deleted,
        "cutoffDate": cutoff_date,
    }))
}
