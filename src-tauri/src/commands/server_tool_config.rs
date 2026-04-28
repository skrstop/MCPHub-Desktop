use crate::services::server_tool_config_service;
use crate::mcp::pool;

/// Toggle a tool/prompt/resource enabled state for a server.
/// POST /servers/:serverName/tools/:toolName/toggle
#[tauri::command]
pub async fn toggle_server_item(
    server_name: String,
    item_type: String, // tool | prompt | resource
    item_name: String,
    enabled: bool,
) -> Result<serde_json::Value, String> {
    let payload = crate::models::server_tool_config::ServerToolConfigPayload {
        server_name,
        item_type,
        item_name,
        enabled,
        description: None,
    };
    let cfg = server_tool_config_service::upsert(&payload)
        .await
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "enabled": cfg.enabled, "itemName": cfg.item_name }))
}

/// Update tool/prompt/resource description override.
/// PUT /servers/:serverName/tools/:toolName/description
#[tauri::command]
pub async fn update_server_item_description(
    server_name: String,
    item_type: String,
    item_name: String,
    description: Option<String>,
) -> Result<serde_json::Value, String> {
    server_tool_config_service::update_description(
        &server_name,
        &item_type,
        &item_name,
        description.as_deref(),
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "success": true, "description": description }))
}

/// Reset tool/prompt/resource description override (DELETE).
/// 重置后会从连接池缓存中读取该 item 的原始描述并返回，
/// 避免前端在恢复默认时把字段误清空。
#[tauri::command]
pub async fn reset_server_item_description(
    server_name: String,
    item_type: String,
    item_name: String,
) -> Result<serde_json::Value, String> {
    server_tool_config_service::reset_description(&server_name, &item_type, &item_name)
        .await
        .map_err(|e| e.to_string())?;

    // 取回该 item 的原始描述（仅 tool 类型支持，prompt/resource 暂返回 null）
    let original_description = if item_type == "tool" {
        pool::list_tools_for(&server_name)
            .await
            .ok()
            .and_then(|tools| {
                tools
                    .into_iter()
                    .find(|t| t.name == item_name)
                    .and_then(|t| t.description)
            })
    } else {
        None
    };

    Ok(serde_json::json!({
        "success": true,
        "description": original_description,
    }))
}

/// List all tool config overrides for a server.
#[tauri::command]
pub async fn list_server_item_configs(
    server_name: String,
    item_type: Option<String>,
) -> Result<Vec<crate::models::server_tool_config::ServerToolConfig>, String> {
    server_tool_config_service::list_for_server(&server_name, item_type.as_deref())
        .await
        .map_err(|e| e.to_string())
}
