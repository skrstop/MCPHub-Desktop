//! Tauri command for MCPB (`.mcpb` bundle) upload & install.
//!
//! Gate is intentionally NOT admin-only (unlike origin's
//! `requireAdminMiddleware`): per product decision, any logged-in user and
//! guests (skipAuth mode) may install MCPB bundles. In skipAuth mode the gate
//! passes unconditionally; otherwise a valid JWT for ANY role is required.

use crate::auth as auth_util;
use crate::commands::auth::{is_skip_auth_enabled, SessionState};
use crate::services::mcpb_service::{self, McpbUploadResult};
use tauri::{AppHandle, State};

/// Any-authenticated-user gate: skipAuth (guest mode) passes; otherwise a
/// valid session token for any role (admin or user) is required. Deliberately
/// weaker than `require_admin` — MCPB install is a local-tool operation.
async fn require_authenticated(session: &State<'_, SessionState>) -> Result<(), String> {
    if is_skip_auth_enabled().await {
        return Ok(());
    }
    let token_str = {
        let guard = session.0.lock().await;
        guard
            .as_ref()
            .ok_or("Not authenticated")?
            .token
            .clone()
    };
    auth_util::verify_token(&token_str).map_err(|e| e.to_string())?;
    Ok(())
}

/// Open the native file picker, extract and validate the `.mcpb` bundle, and
/// return `{ manifest, extractDir }`. `Ok(None)` = the user cancelled the
/// picker (the frontend stays silent).
#[tauri::command]
pub async fn upload_mcpb(
    app: AppHandle,
    session: State<'_, SessionState>,
) -> Result<Option<McpbUploadResult>, String> {
    require_authenticated(&session).await?;
    mcpb_service::upload_mcpb(&app)
        .await
        .map_err(|e| e.to_string())
}
