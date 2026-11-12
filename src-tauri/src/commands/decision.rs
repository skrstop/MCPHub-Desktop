//! Decision-model commands: structured decisions (score per option +
//! probabilities + argmax) from a bundled decision GGUF (Julia-1 style).

use crate::mv::decision::{DecisionModel, DecisionOutput, DecisionRequest};
use std::sync::Arc;
use tauri::{AppHandle, State};
use tokio::sync::Mutex;

/// Process-wide cached model (168 MB GGUF — load once, reuse across calls).
static DECISION_MODEL: Mutex<Option<(String, Arc<DecisionModel>)>> = Mutex::const_new(None);

async fn cached_model(app: &AppHandle, name: &str) -> Result<Arc<DecisionModel>, String> {
    let mut guard = DECISION_MODEL.lock().await;
    if let Some((cached, m)) = guard.as_ref() {
        if cached == name {
            return Ok(m.clone());
        }
    }
    let path = DecisionModel::resolve_model_path(app, name).map_err(|e| e.to_string())?;
    let device = candle_core::Device::Cpu;
    let model = tokio::task::spawn_blocking({
        let path = path.clone();
        move || -> Result<DecisionModel, String> {
            let mut file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
            let dir = path
                .parent()
                .ok_or_else(|| "model path has no parent".to_string())?
                .to_path_buf();
            DecisionModel::load(&mut file, &dir, &device).map_err(|e| e.to_string())
        }
    })
    .await
    .map_err(|e| format!("join load task: {e}"))?
    .map_err(|e| e.to_string())?;
    let arc = Arc::new(model);
    *guard = Some((name.to_string(), arc.clone()));
    Ok(arc)
}

/// Run one structured decision. Admin-gated (loads a global model + the
/// request may carry arbitrary context text).
#[tauri::command]
pub async fn decision_decide(
    session: State<'_, crate::commands::auth::SessionState>,
    app: AppHandle,
    model: String,
    request: DecisionRequest,
) -> Result<DecisionOutput, String> {
    crate::commands::config::require_admin(&session).await?;
    let m = cached_model(&app, &model).await?;
    tokio::task::spawn_blocking(move || m.decide(&request).map_err(|e| e.to_string()))
        .await
        .map_err(|e| format!("join decide task: {e}"))?
}
