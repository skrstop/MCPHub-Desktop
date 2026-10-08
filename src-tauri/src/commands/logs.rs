use tauri::State;

use crate::commands::auth::SessionState;
use crate::{
    models::log::{ActivityPage, ActivityQuery, ActivityStats, LogEntry, LogQuery},
    services::log_service,
};

/// Activity logs carry tool-call arguments verbatim (API keys/tokens appear
/// in inputs); reads and clears are admin-gated (skipAuth short-circuits).
async fn require_admin(session: &SessionState) -> Result<(), String> {
    crate::commands::config::require_admin(session).await
}

#[tauri::command]
pub async fn get_logs(
    session: State<'_, SessionState>,
    query: LogQuery,
) -> Result<Vec<LogEntry>, String> {
    require_admin(&session).await?;
    log_service::query_logs(&query)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn clear_logs(session: State<'_, SessionState>) -> Result<(), String> {
    require_admin(&session).await?;
    log_service::clear_logs().await.map_err(|e| e.to_string())
}

/// Write an app-level log entry from the frontend.
///
/// Routed through `app_logger::log_to_db` so it lands in the same `app_log`
/// table the Logs page reads (via `get_logs`). Used to record update-check
/// lifecycle events (check started, new version available, up-to-date, error).
/// `level` is one of: info | warn | error | debug.
#[tauri::command]
pub async fn log_event(level: String, message: String) -> Result<(), String> {
    // Whitelist the level: arbitrary strings would land in app_log but be
    // filtered out by the Logs page's known-level filters (invisible rows),
    // and the endpoint has no rate limit, so junk values shouldn't persist.
    let level = match level.as_str() {
        "info" | "warn" | "error" | "debug" => level,
        _ => "info".to_string(),
    };
    // Cap the message: the endpoint is unauthenticated by design (frontend
    // telemetry) — an unbounded string would bloat app_log (VACUUM-only
    // shrink) until the 15-day cleanup.
    let message = if message.len() > 4096 {
        let mut end = 4096;
        while !message.is_char_boundary(end) {
            end += 1;
        }
        message[..end].to_string()
    } else {
        message
    };
    crate::services::app_logger::log_to_db(&level, &message);
    Ok(())
}

/// Returns { available: true } — activity logging is always on in the desktop app.
#[tauri::command]
pub async fn get_activity_available() -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({ "available": true }))
}

/// Returns a list of distinct server names seen in activity_log (for filter dropdowns).
#[tauri::command]
pub async fn get_activity_filters(session: State<'_, SessionState>) -> Result<Vec<String>, String> {
    // Admin-gated (matches get_activity_stats/get_tool_activities): the
    // candidate lists enumerate server/tool/bearer-key display names.
    crate::commands::config::require_admin(&session).await?;
    log_service::get_activity_filters()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_activity_stats(
    session: State<'_, SessionState>,
    server: Option<String>,
    status: Option<String>,
    tool: Option<String>,
    group_name: Option<String>,
    key_name: Option<String>,
) -> Result<ActivityStats, String> {
    require_admin(&session).await?;
    log_service::get_activity_stats(
        server.as_deref(),
        status.as_deref(),
        tool.as_deref(),
        group_name.as_deref(),
        key_name.as_deref(),
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_tool_activities(
    session: State<'_, SessionState>,
    page: Option<u32>,
    page_size: Option<u32>,
    server: Option<String>,
    status: Option<String>,
    tool: Option<String>,
    group_name: Option<String>,
    key_name: Option<String>,
) -> Result<ActivityPage, String> {
    // Activity rows carry tool-call arguments verbatim — admin-gated read.
    crate::commands::config::require_admin(&session).await?;
    let q = ActivityQuery {
        page,
        page_size,
        server,
        status,
        tool,
        group_name,
        key_name,
    };
    log_service::query_tool_activities(&q)
        .await
        .map_err(|e| e.to_string())
}

/// 活动日志筛选候选（可搜索分页下拉，§8）：field ∈ server|tool|group|keyName
#[tauri::command]
pub async fn get_activity_filter_options(
    session: State<'_, SessionState>,
    field: String,
    search: Option<String>,
    page: Option<u32>,
    page_size: Option<u32>,
) -> Result<crate::models::log::ActivityFilterOptionsPage, String> {
    // Admin-gated (matches the other activity reads).
    crate::commands::config::require_admin(&session).await?;
    log_service::get_activity_filter_options(
        &field,
        search.as_deref(),
        page.unwrap_or(1),
        page_size.unwrap_or(50),
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn clear_tool_activities(
    session: State<'_, SessionState>,
) -> Result<serde_json::Value, String> {
    require_admin(&session).await?;
    let deleted = log_service::clear_activities()
        .await
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "deletedCount": deleted,
    }))
}

/// Manually trigger log cleanup: delete entries older than 15 days and VACUUM.
#[tauri::command]
pub async fn cleanup_old_logs(
    session: State<'_, SessionState>,
) -> Result<serde_json::Value, String> {
    require_admin(&session).await?;
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
pub async fn cleanup_activity_logs(
    session: State<'_, SessionState>,
    days_old: Option<i64>,
) -> Result<serde_json::Value, String> {
    require_admin(&session).await?;
    let days = days_old.unwrap_or(30);
    let (deleted, cutoff_date) = log_service::cleanup_by_days(days)
        .await
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "deletedCount": deleted,
        "cutoffDate": cutoff_date,
    }))
}
