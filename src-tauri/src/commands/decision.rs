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
    /// The persisted selection when it exists but its files no longer resolve
    /// (deleted download dir / removed bundled size). Invisible in `model` —
    /// the decide path silently falls back to a ready default — so the UI
    /// must surface it separately to prompt a re-download.
    pub selection_lost: Option<String>,
}

/// Current decision-support status (enable flag + selected model).
#[tauri::command]
pub async fn decision_status(
    session: State<'_, crate::commands::auth::SessionState>,
    app: AppHandle,
) -> Result<DecisionStatus, String> {
    crate::commands::config::require_admin(&session).await?;
    let enabled = crate::services::decision_service::is_enabled();
    // Effective model (includes the decide path's ready-fallback): the status
    // row must say what will actually run.
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
    // The persisted selection's readiness (NOT the effective model's — the
    // ready-fallback would make it always true and kill the lost-model
    // warning). The pure derivation below pins that distinction.
    let persisted = crate::mv::models::current_decision_model().await;
    let persisted_ready = match &persisted {
        Some(m) => crate::mv::models::resolve_model_paths_kind(&app, ModelKind::Decision, m)
            .ok()
            .flatten()
            .is_some(),
        None => false,
    };
    let view = crate::services::decision_service::decision_status_view(
        crate::services::decision_service::EffectiveModel(
            crate::services::decision_service::ResolvedModel {
                name: model,
                ready: model_ready,
            },
        ),
        crate::services::decision_service::PersistedModel(
            crate::services::decision_service::ResolvedModel {
                name: persisted,
                ready: persisted_ready,
            },
        ),
    );
    Ok(DecisionStatus {
        enabled,
        model: view.model,
        model_ready: view.model_ready,
        selection_lost: view.selection_lost,
    })
}

/// The app-level decision tool (decision_decide) as MCP tool definitions
/// (name / description / inputSchema). Always returns the catalog — the
/// "view tools" dialog documents the tool even while decision support is
/// disabled (enable gating applies to the MCP tools/list surface, not this
/// documentation view). Read-only, no admin gate (mirrors `rag_tools`).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_status_serde_camel_case() {
        // The frontend reads `modelReady` / `selectionLost` — a serde rename
        // drift would silently drop the lost-model warning data.
        let v = serde_json::to_value(DecisionStatus {
            enabled: true,
            model: Some("julia/1".into()),
            model_ready: true,
            selection_lost: Some("user/2".into()),
        })
        .unwrap();
        assert_eq!(v["enabled"], true);
        assert_eq!(v["model"], "julia/1");
        assert_eq!(v["modelReady"], true);
        assert_eq!(v["selectionLost"], "user/2");
        assert!(v.get("model_ready").is_none());
        assert!(v.get("selection_lost").is_none());
        // None serializes as an explicit null with the KEY PRESENT (get()
        // distinguishes present-null from absent; Index does not).
        let v = serde_json::to_value(DecisionStatus {
            enabled: false,
            model: None,
            model_ready: false,
            selection_lost: None,
        })
        .unwrap();
        assert!(v.get("selectionLost").is_some());
        assert!(v["selectionLost"].is_null());
    }

    #[tokio::test]
    async fn decision_tools_command_returns_catalog_regardless_of_flag() {
        // Holding the service TEST_LOCK pins DECISION_ACTIVE at its resting
        // false (every lock-holder restores false), so a command-level
        // re-gate regression deterministically returns [] and fails here —
        // the command must delegate to the ungated catalog view.
        let _g = crate::services::decision_service::test_lock();
        let defs = decision_tools().await.unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0]["name"], "decision_decide");
    }

    #[test]
    fn decision_status_view_pins_effective_vs_persisted() {
        use crate::services::decision_service::{
            decision_status_view, EffectiveModel, PersistedModel, ResolvedModel,
        };
        // The bug class this exists for: the effective model's readiness
        // (always true under the ready-fallback) must NOT decide
        // selection_lost. Persisted unready → warning data even though the
        // effective model is ready.
        let v = decision_status_view(
            EffectiveModel(ResolvedModel { name: Some("julia/1".into()), ready: true }),
            PersistedModel(ResolvedModel { name: Some("user/2".into()), ready: false }),
        );
        assert_eq!(v.model, Some("julia/1".to_string()));
        assert!(v.model_ready);
        assert_eq!(v.selection_lost, Some("user/2".to_string()));
        // Persisted ready → no warning (even with an unready effective model —
        // pins that selection_lost uses ONLY the persisted pair's readiness,
        // not a conjunction with the effective's).
        let v = decision_status_view(
            EffectiveModel(ResolvedModel { name: Some("julia/1".into()), ready: false }),
            PersistedModel(ResolvedModel { name: Some("user/2".into()), ready: true }),
        );
        assert!(!v.model_ready);
        assert_eq!(v.selection_lost, None);
        // Both ready → no warning.
        let v = decision_status_view(
            EffectiveModel(ResolvedModel { name: Some("user/2".into()), ready: true }),
            PersistedModel(ResolvedModel { name: Some("user/2".into()), ready: true }),
        );
        assert_eq!(v.selection_lost, None);
        // Nothing persisted → no warning; effective passthrough.
        let v = decision_status_view(
            EffectiveModel(ResolvedModel { name: None, ready: false }),
            PersistedModel(ResolvedModel { name: None, ready: false }),
        );
        assert_eq!(v.model, None);
        assert!(!v.model_ready);
        assert_eq!(v.selection_lost, None);
    }
}
