use crate::{
    models::bearer_key::{BearerKey, BearerKeyPayload},
    services::bearer_key_service,
};
use tauri::State;
use crate::commands::auth::SessionState;

#[tauri::command]
pub async fn list_bearer_keys(
    session: State<'_, SessionState>,
) -> Result<Vec<BearerKey>, String> {
    let lock = session.0.lock().await;
    if lock.is_none() {
        return Err("Not authenticated".to_string());
    }
    drop(lock);
    bearer_key_service::list_all()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn create_bearer_key(
    session: State<'_, SessionState>,
    payload: BearerKeyPayload,
) -> Result<BearerKey, String> {
    let lock = session.0.lock().await;
    if lock.is_none() {
        return Err("Not authenticated".to_string());
    }
    drop(lock);
    bearer_key_service::create(&payload)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_bearer_key(
    session: State<'_, SessionState>,
    id: String,
    payload: BearerKeyPayload,
) -> Result<BearerKey, String> {
    let lock = session.0.lock().await;
    if lock.is_none() {
        return Err("Not authenticated".to_string());
    }
    drop(lock);
    bearer_key_service::update(&id, &payload)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Bearer key '{}' not found", id))
}

#[tauri::command]
pub async fn delete_bearer_key(
    session: State<'_, SessionState>,
    id: String,
) -> Result<bool, String> {
    let lock = session.0.lock().await;
    if lock.is_none() {
        return Err("Not authenticated".to_string());
    }
    drop(lock);
    bearer_key_service::delete(&id)
        .await
        .map_err(|e| e.to_string())
}
