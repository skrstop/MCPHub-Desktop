use crate::services::{bearer_key_service, config_service, http_server, settings_import};

#[tauri::command]
pub async fn get_system_config() -> Result<serde_json::Value, String> {
    config_service::get().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_system_config(config: serde_json::Value) -> Result<serde_json::Value, String> {
    let result = config_service::update(&config).await.map_err(|e| e.to_string())?;
    // Sync HTTP server state (start/stop) based on updated config
    http_server::sync_with_config().await;
    Ok(result)
}

/// Returns the full settings payload expected by the frontend SettingsContext:
/// { systemConfig: { routing, install, smartRouting, ... }, bearerKeys: [...] }
#[tauri::command]
pub async fn get_settings() -> Result<serde_json::Value, String> {
    let system_config = config_service::get().await.map_err(|e| e.to_string())?;
    let bearer_keys = bearer_key_service::list_all().await.map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "systemConfig": system_config,
        "bearerKeys": bearer_keys
    }))
}

/// Import servers/users from a mcp_settings.json string
#[tauri::command]
pub async fn import_settings(json: String) -> Result<settings_import::ImportSummary, String> {
    settings_import::import_from_json(&json)
        .await
        .map_err(|e| e.to_string())
}

/// Export current server/group/system config as a JSON string (mcp_settings.json compatible)
#[tauri::command]
pub async fn export_settings() -> Result<String, String> {
    use crate::services::{group_service, server_service};
    use std::collections::HashMap;

    let servers = server_service::list_all().await.map_err(|e| e.to_string())?;
    let groups = group_service::list_all().await.map_err(|e| e.to_string())?;

    let mut mcp_servers: HashMap<String, serde_json::Value> = HashMap::new();
    for s in &servers {
        mcp_servers.insert(
            s.name.clone(),
            serde_json::json!({
                "type": s.server_type,
                "description": s.description,
                "command": s.command,
                "args": s.args,
                "env": s.env,
                "url": s.url,
                "disabled": !s.enabled,
            }),
        );
    }

    let mut groups_map: HashMap<String, serde_json::Value> = HashMap::new();
    for g in &groups {
        groups_map.insert(g.name.clone(), serde_json::json!({ "servers": g.servers }));
    }

    let output = serde_json::json!({
        "mcpServers": mcp_servers,
        "groups": groups_map,
    });

    serde_json::to_string_pretty(&output).map_err(|e| e.to_string())
}
