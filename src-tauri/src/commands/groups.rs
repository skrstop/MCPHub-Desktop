use crate::{models::group::{Group, GroupPayload, GroupPage}, services::group_service};

#[tauri::command]
pub async fn list_groups() -> Result<Vec<Group>, String> {
    group_service::list_all().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn add_group(payload: GroupPayload) -> Result<Group, String> {
    group_service::create(&payload).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_group(id: String, payload: GroupPayload) -> Result<Group, String> {
    group_service::update(&id, &payload).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_group(id: String) -> Result<(), String> {
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
