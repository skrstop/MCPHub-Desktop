use crate::{models::group::{Group, GroupPayload}, services::group_service};

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
