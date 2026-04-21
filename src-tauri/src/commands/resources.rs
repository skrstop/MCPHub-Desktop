use crate::{
    models::resource::{BuiltinResource, BuiltinResourcePayload},
    services::resource_service,
    commands::auth::SessionState,
};
use tauri::State;

#[tauri::command]
pub async fn list_builtin_resources(
    session: State<'_, SessionState>,
) -> Result<Vec<BuiltinResource>, String> {
    let lock = session.0.lock().await;
    if lock.is_none() {
        return Err("Not authenticated".to_string());
    }
    drop(lock);
    resource_service::list_all().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_builtin_resource(
    session: State<'_, SessionState>,
    id: String,
) -> Result<Option<BuiltinResource>, String> {
    let lock = session.0.lock().await;
    if lock.is_none() {
        return Err("Not authenticated".to_string());
    }
    drop(lock);
    resource_service::find_by_id(&id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn create_builtin_resource(
    session: State<'_, SessionState>,
    payload: BuiltinResourcePayload,
) -> Result<BuiltinResource, String> {
    let lock = session.0.lock().await;
    if lock.is_none() {
        return Err("Not authenticated".to_string());
    }
    drop(lock);
    resource_service::create(&payload)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_builtin_resource(
    session: State<'_, SessionState>,
    id: String,
    payload: BuiltinResourcePayload,
) -> Result<BuiltinResource, String> {
    let lock = session.0.lock().await;
    if lock.is_none() {
        return Err("Not authenticated".to_string());
    }
    drop(lock);
    resource_service::update(&id, &payload)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Resource '{}' not found", id))
}

#[tauri::command]
pub async fn delete_builtin_resource(
    session: State<'_, SessionState>,
    id: String,
) -> Result<bool, String> {
    let lock = session.0.lock().await;
    if lock.is_none() {
        return Err("Not authenticated".to_string());
    }
    drop(lock);
    resource_service::delete(&id)
        .await
        .map_err(|e| e.to_string())
}
