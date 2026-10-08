use tauri::State;

use crate::commands::auth::SessionState;
use crate::services::server_tool_config_service;
use crate::mcp::pool;

/// Toggle a tool/prompt/resource enabled state for a server.
/// POST /servers/:serverName/tools/:toolName/toggle
#[tauri::command]
pub async fn toggle_server_item(
    session: State<'_, SessionState>,
    server_name: String,
    item_type: String, // tool | prompt | resource
    item_name: String,
    enabled: bool,
) -> Result<serde_json::Value, String> {
    // Write op: non-admin sessions must not re-enable admin-disabled tools.
    crate::commands::config::require_admin(&session).await?;
    let payload = crate::models::server_tool_config::ServerToolConfigPayload {
        server_name,
        item_type,
        item_name,
        enabled,
        description: None,
        pinned: None,
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
    session: State<'_, SessionState>,
    server_name: String,
    item_type: String,
    item_name: String,
    description: Option<String>,
) -> Result<serde_json::Value, String> {
    // Write op: non-admin sessions must not re-enable admin-disabled tools.
    crate::commands::config::require_admin(&session).await?;
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
    session: State<'_, SessionState>,
    server_name: String,
    item_type: String,
    item_name: String,
) -> Result<serde_json::Value, String> {
    // Write op: non-admin sessions must not re-enable admin-disabled tools.
    crate::commands::config::require_admin(&session).await?;
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

/// Set the server-level $smart pin for a tool.
/// POST /servers/:serverName/tools/:toolName/pin
#[tauri::command]
pub async fn set_server_tool_pinned(
    session: State<'_, SessionState>,
    server_name: String,
    item_type: String,
    item_name: String,
    pinned: bool,
) -> Result<serde_json::Value, String> {
    // Write op: same admin gate as toggle/description.
    crate::commands::config::require_admin(&session).await?;
    // Pins are a tool-only concept (list_pinned_tools reads item_type='tool');
    // writing other types would create dead rows.
    if item_type != "tool" {
        return Err("pin is only supported for tools".to_string());
    }
    // Smart Routing meta tools are unreachable on every $smart surface
    // (listing/resolve both skip meta names) — persisting one would be a
    // lazy dead row; reject at the write side instead.
    if crate::smart_routing::meta::is_meta_tool(&item_name) {
        return Err("smart routing meta tools cannot be pinned".to_string());
    }
    // Un-pin must always succeed (dead-row cleanup); pin must reference a real
    // server+tool or the row is unlistable/unremovable from the UI.
    if pinned {
        let exists = crate::services::server_service::get_by_name(&server_name)
            .await
            .map(|s| s.is_some())
            .unwrap_or(false);
        if !exists {
            return Err(format!("server '{server_name}' not found"));
        }
        let cached = crate::mcp::pool::list_tools_for(&server_name)
            .await
            .unwrap_or_default();
        // A sleeping on-demand server has an empty cache — pinning is
        // allowed (call path cold-starts it). Same for a server with no
        // pool entry yet (never connected since restart): the pin stays
        // hidden from listings until the server connects, but un-pinning
        // always works, so accepting it is safe.
        if !cached.is_empty() && !cached.iter().any(|t| t.name == item_name) {
            return Err(format!("tool '{item_name}' not found on server '{server_name}'"));
        }
    }
    server_tool_config_service::set_pinned(&server_name, &item_type, &item_name, pinned)
        .await
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "success": true, "pinned": pinned }))
}

/// List server-level pinned tool names.
/// GET /servers/:serverName/tools/pins
#[tauri::command]
pub async fn list_server_tool_pins(
    session: State<'_, SessionState>,
    server_name: String,
) -> Result<Vec<String>, String> {
    let _ = session; // read op: desktop single-user, parity with list_servers
    server_tool_config_service::list_pinned_tools(&server_name)
        .await
        .map_err(|e| e.to_string())
}
