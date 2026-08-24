use crate::{
    models::resource::{BuiltinResource, BuiltinResourcePayload, ResourcePage},
    services::resource_service,
};

#[tauri::command]
pub async fn list_builtin_resources() -> Result<Vec<BuiltinResource>, String> {
    resource_service::list_all().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_builtin_resource(id: String) -> Result<Option<BuiltinResource>, String> {
    resource_service::find_by_id(&id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn create_builtin_resource(
    payload: BuiltinResourcePayload,
) -> Result<BuiltinResource, String> {
    resource_service::create(&payload)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_builtin_resource(
    id: String,
    payload: BuiltinResourcePayload,
) -> Result<BuiltinResource, String> {
    resource_service::update(&id, &payload)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Resource '{}' not found", id))
}

#[tauri::command]
pub async fn delete_builtin_resource(id: String) -> Result<bool, String> {
    resource_service::delete(&id)
        .await
        .map_err(|e| e.to_string())
}

/// Paginated builtin-resource search for the Resources page's toolbar:
/// `search_key` (uri/name/description substring, empty = all) + `filter`
/// ("all" | "active" | "inactive"), SQL-level LIMIT/OFFSET. `page` is 0-based.
#[tauri::command]
pub async fn search_builtin_resources(
    search_key: String,
    filter: String,
    page: u32,
    page_size: u32,
) -> Result<ResourcePage, String> {
    resource_service::search_paged(&search_key, &filter, page, page_size)
        .await
        .map_err(|e| e.to_string())
}
