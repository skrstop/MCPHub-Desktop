use crate::{
    auth as auth_util,
    models::bearer_key::{BearerKey, BearerKeyPayload},
    services::bearer_key_service,
};
use tauri::State;
use crate::commands::auth::SessionState;

/// Helper to verify that the current session belongs to an admin user.
async fn require_admin(session: &SessionState) -> Result<(), String> {
    let token_str = {
        let guard = session.0.lock().await;
        guard.as_ref().ok_or("Not authenticated")?.token.clone()
    };
    let claims = auth_util::verify_token(&token_str).map_err(|e| e.to_string())?;
    if claims.role != "admin" {
        return Err("Admin access required".to_string());
    }
    Ok(())
}

#[tauri::command]
pub async fn list_bearer_keys(
    session: State<'_, SessionState>,
) -> Result<Vec<BearerKey>, String> {
    require_admin(&session).await?;
    bearer_key_service::list_all()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn create_bearer_key(
    session: State<'_, SessionState>,
    payload: BearerKeyPayload,
) -> Result<BearerKey, String> {
    require_admin(&session).await?;
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
    require_admin(&session).await?;
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
    require_admin(&session).await?;
    bearer_key_service::delete(&id)
        .await
        .map_err(|e| e.to_string())
}
