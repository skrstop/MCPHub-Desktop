use tauri::State;

use crate::commands::auth::SessionState;
use crate::{models::group::{Group, GroupPayload, GroupPage}, services::group_service};

#[tauri::command]
pub async fn list_groups() -> Result<Vec<Group>, String> {
    group_service::list_all().await.map_err(|e| e.to_string())
}

/// Reject names that would make the group unreachable or ambiguous via the
/// MCP scope paths: empty/whitespace-only, names containing `/` (scope
/// separator — `/mcp/{group}` would truncate and silently yield an empty
/// tools/list), and leading `$` (`$smart` is the reserved meta scope).
fn validate_group_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("group name cannot be empty".to_string());
    }
    if name.contains('/') {
        return Err("group name cannot contain '/'".to_string());
    }
    if name.starts_with('$') {
        return Err("group name cannot start with '$' (reserved for $smart)".to_string());
    }
    Ok(())
}

#[tauri::command]
pub async fn add_group(
    session: State<'_, SessionState>,
    payload: GroupPayload,
) -> Result<Group, String> {
    crate::commands::config::require_admin(&session).await?;
    validate_group_name(&payload.name)?;
    group_service::create(&payload).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_group(
    session: State<'_, SessionState>,
    id: String,
    payload: GroupPayload,
) -> Result<Group, String> {
    crate::commands::config::require_admin(&session).await?;
    validate_group_name(&payload.name)?;
    group_service::update(&id, &payload).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_group(
    session: State<'_, SessionState>,
    id: String,
) -> Result<(), String> {
    crate::commands::config::require_admin(&session).await?;
    group_service::delete(&id).await.map_err(|e| e.to_string())
}

/// Paginated group search (name/description substring, empty = all).
/// `page` is 0-based. Backs the ServerForm group dropdown.
#[tauri::command]
pub async fn search_groups(
    search_key: String,
    page: u32,
    page_size: u32,
) -> Result<GroupPage, String> {
    group_service::search_paged(&search_key, page, page_size)
        .await
        .map_err(|e| e.to_string())
}
