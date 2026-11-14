//! Decision-support commands: enable toggle, model management (list/select/
//! download — full parity with the embedding model flow), structured decide
//! (records request history), and history list/get/clear.

use crate::mv::decision::{DecisionOutput, DecisionRequest};
use crate::mv::models::{ModelKind, RagModelInfo};
use serde::Serialize;
use tauri::{AppHandle, State};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionStatus {
    pub enabled: bool,
    pub model: Option<String>,
    pub model_ready: bool,
}

/// Current decision-support status (enable flag + selected model).
#[tauri::command]
pub async fn decision_status(
    session: State<'_, crate::commands::auth::SessionState>,
    app: AppHandle,
) -> Result<DecisionStatus, String> {
    crate::commands::config::require_admin(&session).await?;
    let enabled = crate::services::decision_service::is_enabled();
    let model = crate::services::decision_service::selected_model(&app).await;
    let model_ready = model
        .as_ref()
        .map(|m| {
            crate::mv::models::resolve_model_paths_kind(&app, ModelKind::Decision, m)
                .ok()
                .flatten()
                .is_some()
        })
        .unwrap_or(false);
    Ok(DecisionStatus {
        enabled,
        model,
        model_ready,
    })
}

/// The app-level decision tool (decision_decide) as MCP tool definitions
/// (name / description / inputSchema). Empty when decision support is
/// disabled. Powers the "view tools" dialog in the decision page — the same
/// source the HTTP MCP layer advertises in tools/list. Read-only, no admin
/// gate (mirrors `rag_tools`).
#[tauri::command]
pub async fn decision_tools() -> Result<Vec<serde_json::Value>, String> {
    Ok(crate::services::decision_service::decision_tool_definitions())
}

/// Toggle decision support (persisted; the builtin tool exposure follows).
#[tauri::command]
pub async fn decision_set_enabled(
    session: State<'_, crate::commands::auth::SessionState>,
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    crate::commands::config::require_admin(&session).await?;
    crate::services::decision_service::set_enabled(&app, enabled)
        .await
        .map_err(|e| e.to_string())
}

/// List decision models (same shape as the embedding model dropdown).
#[tauri::command]
pub async fn decision_list_models(
    session: State<'_, crate::commands::auth::SessionState>,
    app: AppHandle,
) -> Result<Vec<RagModelInfo>, String> {
    crate::commands::config::require_admin(&session).await?;
    crate::mv::models::list_models_kind(&app, ModelKind::Decision).map_err(|e| e.to_string())
}

/// Select the decision model (persisted; the cached runtime reloads on next
/// use). The model must be ready.
#[tauri::command]
pub async fn decision_select_model(
    session: State<'_, crate::commands::auth::SessionState>,
    app: AppHandle,
    model: String,
) -> Result<(), String> {
    crate::commands::config::require_admin(&session).await?;
    crate::mv::models::ensure_model_ready_kind(&app, ModelKind::Decision, &model)
        .map_err(|e| e.to_string())?;
    crate::mv::models::set_decision_model(&model)
        .await
        .map_err(|e| e.to_string())?;
    crate::services::decision_service::invalidate_model_cache().await;
    Ok(())
}

/// Start a background decision-model download (same machinery as embedding
/// models; progress on `rag://model-download`, queryable via
/// `decision_download_status`).
#[tauri::command]
pub async fn decision_download_model(
    session: State<'_, crate::commands::auth::SessionState>,
    app: AppHandle,
    model: String,
    source: Option<String>,
) -> Result<(), String> {
    crate::commands::config::require_admin(&session).await?;
    tauri::async_runtime::spawn_blocking(move || {
        crate::mv::models::download_model_kind(&app, ModelKind::Decision, &model, source.as_deref())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("join download task: {e}"))?
    // Note: download_model_kind is fire-and-forget (the GGUF is replaced
    // asynchronously by the background task), so there is deliberately NO
    // cache invalidation here — cached_model's file fingerprint (mtime+len)
    // detects the replacement on the next decide.
}

/// Download progress snapshot for a decision model size.
#[tauri::command]
pub async fn decision_download_status(
    session: State<'_, crate::commands::auth::SessionState>,
    model: String,
) -> Result<Option<crate::mv::models::RagModelDownloadProgress>, String> {
    crate::commands::config::require_admin(&session).await?;
    Ok(crate::mv::models::download_status_kind(
        ModelKind::Decision,
        &model,
    ))
}

/// Cancel a running decision-model download.
#[tauri::command]
pub async fn decision_download_cancel(
    session: State<'_, crate::commands::auth::SessionState>,
    model: String,
) -> Result<bool, String> {
    crate::commands::config::require_admin(&session).await?;
    Ok(crate::mv::models::cancel_download_kind(ModelKind::Decision, &model))
}

/// Run one structured decision (records the request into the history table).
#[tauri::command]
pub async fn decision_decide(
    session: State<'_, crate::commands::auth::SessionState>,
    app: AppHandle,
    request: DecisionRequest,
) -> Result<DecisionOutput, String> {
    crate::commands::config::require_admin(&session).await?;
    crate::services::decision_service::decide_and_record(&app, request, "ui")
        .await
        .map_err(|e| e.to_string())
}

/// Decision request history, newest first.
#[tauri::command]
pub async fn decision_history_list(
    session: State<'_, crate::commands::auth::SessionState>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<crate::services::decision_service::DecisionHistoryEntry>, String> {
    crate::commands::config::require_admin(&session).await?;
    crate::services::decision_service::list_history(limit.unwrap_or(50), offset.unwrap_or(0))
        .await
        .map_err(|e| e.to_string())
}

/// One history entry by id.
#[tauri::command]
pub async fn decision_history_get(
    session: State<'_, crate::commands::auth::SessionState>,
    id: String,
) -> Result<Option<crate::services::decision_service::DecisionHistoryEntry>, String> {
    crate::commands::config::require_admin(&session).await?;
    crate::services::decision_service::get_history(&id)
        .await
        .map_err(|e| e.to_string())
}

/// Delete ALL decision history.
#[tauri::command]
pub async fn decision_history_clear(
    session: State<'_, crate::commands::auth::SessionState>,
) -> Result<u64, String> {
    crate::commands::config::require_admin(&session).await?;
    crate::services::decision_service::clear_history()
        .await
        .map_err(|e| e.to_string())
}
