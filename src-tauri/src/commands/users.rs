use crate::{models::user::{UserInfo, UserPayload}, services::user_service};

#[tauri::command]
pub async fn list_users() -> Result<Vec<UserInfo>, String> {
    user_service::list_all().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn add_user(payload: UserPayload) -> Result<UserInfo, String> {
    user_service::create(&payload).await.map_err(|e| e.to_string())
}

/// Update a user by username. Supports changing role (isAdmin) and/or password.
#[tauri::command]
pub async fn update_user(
    username: String,
    is_admin: Option<bool>,
    new_password: Option<String>,
) -> Result<UserInfo, String> {
    user_service::update_by_username(&username, is_admin, new_password.as_deref())
        .await
        .map_err(|e| e.to_string())
}

/// Delete a user by username.
#[tauri::command]
pub async fn delete_user(username: String) -> Result<(), String> {
    user_service::delete_by_username(&username).await.map_err(|e| e.to_string())
}
