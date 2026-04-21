use crate::{
    mcp::pool,
    models::server::{ServerConfig, ServerInfo, ServerStatus},
    services::{mcp_manager, server_service},
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
        tool_count: 0,
        error: Some("Not connected".to_string()),
        last_connected: None,
    });
    Ok(status)
}
