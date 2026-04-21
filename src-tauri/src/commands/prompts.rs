use crate::{
    models::prompt::{BuiltinPrompt, BuiltinPromptPayload},
    services::prompt_service,
    commands::auth::SessionState,
};
use tauri::State;

#[tauri::command]
pub async fn list_builtin_prompts(
    session: State<'_, SessionState>,
) -> Result<Vec<BuiltinPrompt>, String> {
    let lock = session.0.lock().await;
    if lock.is_none() {
        return Err("Not authenticated".to_string());
    }
    drop(lock);
    prompt_service::list_all().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_builtin_prompt(
    session: State<'_, SessionState>,
    id: String,
) -> Result<Option<BuiltinPrompt>, String> {
    let lock = session.0.lock().await;
    if lock.is_none() {
        return Err("Not authenticated".to_string());
    }
    drop(lock);
    prompt_service::find_by_id(&id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn create_builtin_prompt(
    session: State<'_, SessionState>,
    payload: BuiltinPromptPayload,
) -> Result<BuiltinPrompt, String> {
    let lock = session.0.lock().await;
    if lock.is_none() {
        return Err("Not authenticated".to_string());
    }
    drop(lock);
    prompt_service::create(&payload)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_builtin_prompt(
    session: State<'_, SessionState>,
    id: String,
    payload: BuiltinPromptPayload,
) -> Result<BuiltinPrompt, String> {
    let lock = session.0.lock().await;
    if lock.is_none() {
        return Err("Not authenticated".to_string());
    }
    drop(lock);
    prompt_service::update(&id, &payload)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Prompt '{}' not found", id))
}

#[tauri::command]
pub async fn delete_builtin_prompt(
    session: State<'_, SessionState>,
    id: String,
) -> Result<bool, String> {
    let lock = session.0.lock().await;
    if lock.is_none() {
        return Err("Not authenticated".to_string());
    }
    drop(lock);
    prompt_service::delete(&id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn call_builtin_prompt(
    session: State<'_, SessionState>,
    id: String,
    args: serde_json::Value,
) -> Result<String, String> {
    let lock = session.0.lock().await;
    if lock.is_none() {
        return Err("Not authenticated".to_string());
    }
    drop(lock);
    let prompt = prompt_service::find_by_id(&id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Prompt '{}' not found", id))?;

    if !prompt.enabled {
        return Err(format!("Prompt '{}' is disabled", id));
    }

    Ok(prompt_service::render_template(&prompt.template, &args))
}
