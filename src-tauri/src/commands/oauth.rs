//! OAuth Tauri commands (v1.1.1004 §A4).
//!
//! Thin admin-gated wrappers over `services::oauth_upstream`:
//! - `oauth_connect`: start the authorization flow (discovery + registration +
//!   PKCE + system browser + loopback listener). The flow completes
//!   asynchronously; the frontend listens for `server://oauth-connected` /
//!   `server://oauth-error` events.
//! - `oauth_disconnect`: clear stored credentials/pending state.
//! - `get_server_oauth_status`: persisted flow status (disabled/pending/
//!   connected/error) for the server card badge.

use tauri::State;

use crate::commands::config::require_admin;
use crate::models::server::ServerConfig;
use crate::services::{oauth_upstream, server_service};

/// Start the OAuth authorization flow for a server. The upstream URL is read
/// from the stored config; the optional `challenge` (a captured
/// `WWW-Authenticate` value) seeds discovery when the frontend passes one.
#[tauri::command]
pub async fn oauth_connect(
    session: State<'_, crate::commands::auth::SessionState>,
    server_name: String,
    challenge: Option<String>,
) -> Result<String, String> {
    require_admin(&session).await?;

    let cfg: ServerConfig = server_service::get_by_name(&server_name)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Server '{server_name}' not found"))?;

    let oauth_cfg = cfg
        .oauth
        .as_ref()
        .ok_or_else(|| format!("Server '{server_name}' has no OAuth configuration"))?;
    if oauth_cfg.access_token.as_deref().map_or(false, |t| !t.is_empty()) {
        return Err(format!(
            "Server '{server_name}' uses a static access token; clear it in the server form to use the authorization flow"
        ));
    }
    let upstream_url = cfg
        .url
        .clone()
        .filter(|u| !u.is_empty())
        .ok_or_else(|| format!("Server '{server_name}' has no upstream URL (OAuth applies to sse/streamable-http/openapi servers)"))?;

    oauth_upstream::start_authorization(&server_name, &upstream_url, oauth_cfg, challenge.as_deref())
        .await
        .map(|_| format!("authorization started for '{server_name}'"))
        .map_err(|e| e.to_string())
}

/// Clear stored OAuth credentials / pending authorization state for a server.
#[tauri::command]
pub async fn oauth_disconnect(
    session: State<'_, crate::commands::auth::SessionState>,
    server_name: String,
) -> Result<(), String> {
    require_admin(&session).await?;
    oauth_upstream::disconnect(&server_name).await
}

/// Persisted OAuth flow status for a server: disabled | pending | connected | error.
#[tauri::command]
pub async fn get_server_oauth_status(
    session: State<'_, crate::commands::auth::SessionState>,
    server_name: String,
) -> Result<String, String> {
    require_admin(&session).await?;
    Ok(oauth_upstream::status_of(&server_name).await)
}

// ── OAuth Provider client management (v1.1.1004 §B4) ────────────────────────

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthClientInfo {
    pub client_id: String,
    pub name: String,
    pub redirect_uris: Vec<String>,
    pub scopes: Vec<String>,
    pub created_at: i64,
    pub revoked: bool,
}

#[tauri::command]
pub async fn list_oauth_clients(
    session: State<'_, crate::commands::auth::SessionState>,
) -> Result<Vec<OAuthClientInfo>, String> {
    require_admin(&session).await?;
    let clients = crate::services::oauth_provider::list_clients().await?;
    Ok(clients
        .into_iter()
        .map(|c| OAuthClientInfo {
            redirect_uris: serde_json::from_str(&c.redirect_uris).unwrap_or_default(),
            scopes: c.scopes.split_whitespace().map(|s| s.to_string()).collect(),
            client_id: c.client_id,
            name: c.name,
            created_at: c.created_at,
            revoked: c.revoked,
        })
        .collect())
}

#[tauri::command]
pub async fn revoke_oauth_client(
    session: State<'_, crate::commands::auth::SessionState>,
    client_id: String,
) -> Result<bool, String> {
    require_admin(&session).await?;
    crate::services::oauth_provider::revoke_client(&client_id).await
}
