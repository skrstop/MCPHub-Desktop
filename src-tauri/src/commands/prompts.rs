use crate::{
    models::prompt::{BuiltinPrompt, BuiltinPromptPayload},
    services::prompt_service,
};

#[tauri::command]
pub async fn list_builtin_prompts() -> Result<Vec<BuiltinPrompt>, String> {
    prompt_service::list_all().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_builtin_prompt(id: String) -> Result<Option<BuiltinPrompt>, String> {
    prompt_service::find_by_id(&id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn create_builtin_prompt(
    payload: BuiltinPromptPayload,
) -> Result<BuiltinPrompt, String> {
    prompt_service::create(&payload)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_builtin_prompt(
    id: String,
    payload: BuiltinPromptPayload,
) -> Result<BuiltinPrompt, String> {
    prompt_service::update(&id, &payload)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Prompt '{}' not found", id))
}

#[tauri::command]
pub async fn delete_builtin_prompt(id: String) -> Result<bool, String> {
    prompt_service::delete(&id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn call_builtin_prompt(
    id: String,
    args: serde_json::Value,
) -> Result<String, String> {
    let prompt = prompt_service::find_by_id(&id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Prompt '{}' not found", id))?;

    if !prompt.enabled {
        return Err(format!("Prompt '{}' is disabled", id));
    }

    Ok(prompt_service::render_template(&prompt.template, &args))
}
