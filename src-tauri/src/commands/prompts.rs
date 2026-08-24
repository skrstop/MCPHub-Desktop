use crate::{
    models::prompt::{BuiltinPrompt, BuiltinPromptPayload, PromptPage},
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

/// Paginated builtin-prompt search for the Prompts page's toolbar:
/// `search_key` (name/title/description substring, empty = all) + `filter`
/// ("all" | "active" | "inactive"), SQL-level LIMIT/OFFSET. `page` is 0-based.
#[tauri::command]
pub async fn search_builtin_prompts(
    search_key: String,
    filter: String,
    page: u32,
    page_size: u32,
) -> Result<PromptPage, String> {
    prompt_service::search_paged(&search_key, &filter, page, page_size)
        .await
        .map_err(|e| e.to_string())
}
