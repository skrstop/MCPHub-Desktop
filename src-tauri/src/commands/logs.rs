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
pub async fn get_activity_stats() -> Result<ActivityStats, String> {
    log_service::get_activity_stats()
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
pub async fn clear_tool_activities() -> Result<(), String> {
    log_service::clear_activities()
        .await
        .map_err(|e| e.to_string())
}
