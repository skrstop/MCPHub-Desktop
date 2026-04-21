use crate::{
    auth as auth_util,
    models::{
        auth::{AuthToken, LoginRequest},
        user::{UserInfo, UserPayload, UserRole},
    },
    services::user_service,
};
use tauri::State;
use tokio::sync::Mutex;

/// In-memory session: stores current user token
pub struct SessionState(pub Mutex<Option<AuthToken>>);

#[tauri::command]
pub async fn login(
    request: LoginRequest,
    session: State<'_, SessionState>,
) -> Result<AuthToken, String> {
    let user = user_service::find_by_username(&request.username)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Invalid username or password".to_string())?;

    let valid = auth_util::verify_password(&request.password, &user.password_hash)
        .map_err(|e| e.to_string())?;

    if !valid {
        return Err("Invalid username or password".to_string());
    }

    let role_str = match user.role {
        crate::models::user::UserRole::Admin => "admin",
        crate::models::user::UserRole::User => "user",
    };

    let token = auth_util::issue_token(&user.id, &user.username, role_str)
        .map_err(|e| e.to_string())?;

    let mut guard = session.0.lock().await;
    *guard = Some(token.clone());

    Ok(token)
}

// Logout user
#[tauri::command]
pub async fn logout(session: State<'_, SessionState>) -> Result<(), String> {
    let mut guard = session.0.lock().await;
    *guard = None;
    Ok(())
}

/// Register a new (non-admin) user. Only usable when registration is open.
#[tauri::command]
pub async fn register(
    username: String,
    password: String,
    session: State<'_, SessionState>,
) -> Result<AuthToken, String> {
    // Check username uniqueness
    if user_service::find_by_username(&username)
        .await
        .map_err(|e| e.to_string())?
        .is_some()
    {
        return Err("Username already exists".to_string());
    }

    let payload = UserPayload {
        username: username.clone(),
        password,
        role: Some(UserRole::User),
    };
    let user = user_service::create(&payload).await.map_err(|e| e.to_string())?;

    let token =
        auth_util::issue_token(&user.id, &user.username, "user").map_err(|e| e.to_string())?;

    let mut guard = session.0.lock().await;
    *guard = Some(token.clone());

    Ok(token)
}

#[tauri::command]
pub async fn get_current_user(session: State<'_, SessionState>) -> Result<Option<UserInfo>, String> {
    // Clone the token string while holding the lock, then release before any async work
    let token_str = {
        let guard = session.0.lock().await;
        guard.as_ref().map(|t| t.token.clone())
    };
    let Some(token_str) = token_str else {
        return Ok(None);
    };
    let claims = auth_util::verify_token(&token_str).map_err(|e| e.to_string())?;
    let user = user_service::find_by_username(&claims.username)
        .await
        .map_err(|e| e.to_string())?;
    Ok(user.map(UserInfo::from))
}

#[tauri::command]
pub async fn change_password(
    old_password: String,
    new_password: String,
    session: State<'_, SessionState>,
) -> Result<(), String> {
    // Clone the token string while holding the lock, then release before any async work
    let token_str = {
        let guard = session.0.lock().await;
        guard.as_ref().ok_or("Not authenticated")?.token.clone()
    };
    let claims = auth_util::verify_token(&token_str).map_err(|e| e.to_string())?;

    let user = user_service::find_by_username(&claims.username)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("User not found")?;

    let valid = auth_util::verify_password(&old_password, &user.password_hash)
        .map_err(|e| e.to_string())?;
    if !valid {
        return Err("Current password is incorrect".to_string());
    }

    user_service::update_password(&user.id, &new_password)
        .await
        .map_err(|e| e.to_string())
}
