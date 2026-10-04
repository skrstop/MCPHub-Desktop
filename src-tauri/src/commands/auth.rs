use crate::{
    auth as auth_util,
    models::{
        auth::{AuthToken, LoginRequest},
        user::{UserInfo, UserRole},
    },
    services::{config_service, user_service},
};
use tauri::State;
use tokio::sync::Mutex;

/// In-memory session: stores current user token
pub struct SessionState(pub Mutex<Option<AuthToken>>);

/// Check if skipAuth is enabled in config.
/// Defaults to true (desktop): when the config read fails, the frontend enters
/// guest mode while require_admin would reject — a mismatch that breaks
/// settings export / bearer-key management. Migration 0005 seeds this key, so
/// the fallback only triggers on corrupted configs.
pub(crate) async fn is_skip_auth_enabled() -> bool {
    match config_service::get().await {
        Ok(c) => c
            .get("routing")
            .and_then(|r| r.get("skipAuth"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        // DB read failure: keep the desktop-friendly default (skipAuth on) so a
        // transient error never locks the local user out, but log loudly —
        // in an auth-enabled deployment this window keeps require_admin open.
        Err(e) => {
            log::warn!("[auth] skipAuth config read failed ({e}); falling back to skipAuth=true");
            true
        }
    }
}

#[tauri::command]
pub async fn login(
    request: LoginRequest,
    session: State<'_, SessionState>,
) -> Result<AuthToken, String> {
    let user = match user_service::find_by_username(&request.username).await {
        Ok(Some(u)) => Some(u),
        Ok(None) => None,
        Err(e) => return Err(e.to_string()),
    };

    // Constant-time-ish login: when the user doesn't exist, verify against a
    // dummy hash so the response time matches the "wrong password" path
    // (prevents username enumeration via bcrypt timing).
    let (password_hash, user) = match user {
        Some(u) => (u.password_hash.clone(), Some(u)),
        None => (
            // bcrypt hashes are exactly 60 bytes (7-byte prefix + 53 chars);
            // bcrypt::verify rejects any other length in microseconds without
            // running the KDF, which would reintroduce the timing oracle.
            "$2b$12$00000000000000000000000000000000000000000000000000000"
                .to_string(),
            None,
        ),
    };
    let valid = auth_util::verify_password(&request.password, &password_hash)
        // A malformed stored hash (or the dummy) must NOT leak a different
        // error string than the wrong-password path — that difference is a
        // username-enumeration oracle. Treat verify errors as "invalid".
        .unwrap_or(false);

    let Some(user) = user.filter(|_| valid) else {
        return Err("Invalid username or password".to_string());
    };

    let role_str = match user.role {
        crate::models::user::UserRole::Admin => "admin",
        crate::models::user::UserRole::User => "user",
        crate::models::user::UserRole::Guest => "guest",
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

/// Register a new (non-admin) user. Disabled on desktop: the app ships a
/// fixed admin account and has no open-registration flow — leaving the
/// endpoint ungated would let any local IPC caller create accounts in
/// auth-enabled mode.
#[tauri::command]
pub async fn register(
    username: String,
    password: String,
    session: State<'_, SessionState>,
) -> Result<AuthToken, String> {
    let _ = (username, password, session);
    Err("Registration is disabled in the desktop app".to_string())
}

#[tauri::command]
pub async fn get_current_user(session: State<'_, SessionState>) -> Result<Option<UserInfo>, String> {
    // Clone the token string while holding the lock, then release before any async work
    let token_str = {
        let guard = session.0.lock().await;
        guard.as_ref().map(|t| t.token.clone())
    };
    let Some(token_str) = token_str else {
        // If no token and skipAuth is enabled, return a guest user
        if is_skip_auth_enabled().await {
            let guest_token = auth_util::issue_guest_token().map_err(|e| e.to_string())?;
            let mut guard = session.0.lock().await;
            *guard = Some(guest_token);
            return Ok(Some(UserInfo {
                id: "guest".to_string(),
                username: "guest".to_string(),
                role: UserRole::Guest,
                created_at: chrono::Utc::now().to_rfc3339(),
            }));
        }
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
