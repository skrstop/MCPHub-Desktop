use crate::{
    mcp::pool,
    models::server::{ServerConfig, ServerInfo, ServerStatus},
    services::{mcp_manager, server_service, runtime_env},
};

#[tauri::command]
pub async fn list_servers() -> Result<Vec<ServerInfo>, String> {
    let configs = server_service::list_all().await.map_err(|e| e.to_string())?;
    let mut result = Vec::new();

    for cfg in configs {
        let (status, tools) = pool::get_entry_info(&cfg.name).await.unwrap_or_else(|| (
            ServerStatus {
                name: cfg.name.clone(),
                connected: false,
                starting: false,
                tool_count: 0,
                error: None,
                last_connected: None,
            },
            vec![],
        ));
        result.push(ServerInfo { config: cfg, status, tools });
    }
    Ok(result)
}

#[tauri::command]
pub async fn get_server(name: String) -> Result<Option<ServerInfo>, String> {
    let cfg = server_service::get_by_name(&name)
        .await
        .map_err(|e| e.to_string())?;

    if let Some(cfg) = cfg {
        let (status, tools) = pool::get_entry_info(&name).await.unwrap_or_else(|| (
            ServerStatus {
                name: name.clone(),
                connected: false,
                starting: false,
                tool_count: 0,
                error: None,
                last_connected: None,
            },
            vec![],
        ));
        Ok(Some(ServerInfo { config: cfg, status, tools }))
    } else {
        Ok(None)
    }
}

#[tauri::command]
pub async fn add_server(config: ServerConfig) -> Result<ServerInfo, String> {
    let saved = server_service::create(&config).await.map_err(|e| e.to_string())?;
    let status = if saved.enabled {
        pool::connect_server(&saved).await
    } else {
        ServerStatus {
            name: saved.name.clone(),
            connected: false,
            starting: false,
            tool_count: 0,
            error: None,
            last_connected: None,
        }
    };
    let tools = if status.connected {
        pool::list_tools_for(&saved.name).await.unwrap_or_default()
    } else {
        vec![]
    };
    Ok(ServerInfo { config: saved, status, tools })
}

#[tauri::command]
pub async fn update_server(name: String, config: ServerConfig) -> Result<ServerInfo, String> {
    // Disconnect current connection before update
    pool::disconnect_server(&name).await.ok();
    let saved = server_service::update(&name, &config)
        .await
        .map_err(|e| e.to_string())?;
    let status = if saved.enabled {
        pool::connect_server(&saved).await
    } else {
        ServerStatus {
            name: saved.name.clone(),
            connected: false,
            starting: false,
            tool_count: 0,
            error: None,
            last_connected: None,
        }
    };
    let tools = if status.connected {
        pool::list_tools_for(&saved.name).await.unwrap_or_default()
    } else {
        vec![]
    };
    Ok(ServerInfo { config: saved, status, tools })
}

#[tauri::command]
pub async fn delete_server(name: String) -> Result<(), String> {
    pool::disconnect_server(&name).await.ok();
    server_service::delete(&name).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn toggle_server(name: String) -> Result<bool, String> {
    mcp_manager::toggle_server(&name)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn reload_server(name: String) -> Result<ServerStatus, String> {
    mcp_manager::reload_server(&name)
        .await
        .map_err(|e| e.to_string())?;
    let status = pool::get_status(&name).await.unwrap_or(ServerStatus {
        name: name.clone(),
        connected: false,
        starting: false,
        tool_count: 0,
        error: Some("Not connected".to_string()),
        last_connected: None,
    });
    Ok(status)
}

#[tauri::command]
pub async fn reinstall_server(name: String) -> Result<serde_json::Value, String> {
    // Disconnect first
    pool::disconnect_server(&name).await.ok();

    // Get server config to check command type
    let cfg = server_service::get_by_name(&name)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Server '{}' not found", name))?;

    let command = cfg.command.as_ref().map(|c| c.to_lowercase()).unwrap_or_default();
    let mut cleared: Vec<String> = Vec::new();

    // Clear npx cache if the server uses npx
    if command == "npx" {
        if let Some(cache_dir) = runtime_env::npm_cache_dir() {
            if cache_dir.exists() {
                let _ = std::fs::remove_dir_all(&cache_dir);
                cleared.push("npx".to_string());
            }
        }
    }

    // Clear uvx cache if the server uses uvx
    if command == "uvx" {
        if let Some(cache_dir) = runtime_env::uvx_cache_dir() {
            if cache_dir.exists() {
                let _ = std::fs::remove_dir_all(&cache_dir);
                cleared.push("uvx".to_string());
            }
        }
    }

    // Reconnect the server
    if cfg.enabled {
        pool::connect_server(&cfg).await;
    }

    Ok(serde_json::json!({
        "success": true,
        "cleared": cleared
    }))
}

#[tauri::command]
pub async fn clear_cache() -> Result<serde_json::Value, String> {
    let mut results = serde_json::Map::new();

    // Clear npm/npx cache
    if let Some(npm_cache) = runtime_env::npm_cache_dir() {
        if npm_cache.exists() {
            match std::fs::remove_dir_all(&npm_cache) {
                Ok(_) => { results.insert("npx".to_string(), serde_json::json!({"status": "cleared"})); }
                Err(e) => { results.insert("npx".to_string(), serde_json::json!({"status": "error", "message": e.to_string()})); }
            }
        } else {
            results.insert("npx".to_string(), serde_json::json!({"status": "skipped"}));
        }
    } else {
        results.insert("npx".to_string(), serde_json::json!({"status": "skipped"}));
    }

    // Clear uv/uvx cache
    if let Some(uvx_cache) = runtime_env::uvx_cache_dir() {
        if uvx_cache.exists() {
            match std::fs::remove_dir_all(&uvx_cache) {
                Ok(_) => { results.insert("uvx".to_string(), serde_json::json!({"status": "cleared"})); }
                Err(e) => { results.insert("uvx".to_string(), serde_json::json!({"status": "error", "message": e.to_string()})); }
            }
        } else {
            results.insert("uvx".to_string(), serde_json::json!({"status": "skipped"}));
        }
    } else {
        results.insert("uvx".to_string(), serde_json::json!({"status": "skipped"}));
    }

    Ok(serde_json::json!({
        "success": true,
        "results": results
    }))
}
