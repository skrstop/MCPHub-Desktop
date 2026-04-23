/// mcp_manager — loads server configs from DB and connects all enabled servers at startup.
use crate::{
    mcp::pool,
    services::{config_service, server_service},
};
use anyhow::Result;
use tauri::AppHandle;

/// Called once at application startup — reads all enabled servers from DB and connects.
pub async fn start_all(app: &AppHandle) -> Result<()> {
    let _ = app; // AppHandle reserved for future event emitting
    let configs = server_service::list_all_enabled().await?;
    log::info!("Starting {} MCP servers...", configs.len());

    for cfg in configs {
        let name = cfg.name.clone();
        tokio::spawn(async move {
            let status = pool::connect_server(&cfg).await;
            if status.connected {
                log::info!("Server '{}' connected with {} tools", name, status.tool_count);
            } else {
                log::warn!(
                    "Server '{}' failed to connect: {}",
                    name,
                    status.error.as_deref().unwrap_or("unknown")
                );
            }
        });
    }

    // Background task: auto-reconnect disconnected servers when enableSessionRebuild is true
    tokio::spawn(async {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            let enabled = config_service::get().await.ok()
                .and_then(|c| c.get("enableSessionRebuild").and_then(|v| v.as_bool()))
                .unwrap_or(false);
            if !enabled {
                continue;
            }
            // Find enabled servers that are currently disconnected
            let all_statuses = pool::get_all_statuses().await;
            let disconnected: Vec<String> = all_statuses
                .into_iter()
                .filter(|s| !s.connected)
                .map(|s| s.name)
                .collect();
            if disconnected.is_empty() {
                continue;
            }
            let enabled_configs = match server_service::list_all_enabled().await {
                Ok(cfgs) => cfgs,
                Err(e) => {
                    log::warn!("[session_rebuild] Failed to list enabled servers: {}", e);
                    continue;
                }
            };
            for cfg in enabled_configs {
                if disconnected.contains(&cfg.name) {
                    let name = cfg.name.clone();
                    log::info!("[session_rebuild] Reconnecting server '{}'", name);
                    tokio::spawn(async move {
                        let status = pool::connect_server(&cfg).await;
                        if status.connected {
                            log::info!("[session_rebuild] Server '{}' reconnected", name);
                        } else {
                            log::warn!("[session_rebuild] Server '{}' still failed: {}", name,
                                status.error.as_deref().unwrap_or("unknown"));
                        }
                    });
                }
            }
        }
    });

    Ok(())
}

/// Reload (disconnect + reconnect) a single server
pub async fn reload_server(server_name: &str) -> Result<()> {
    // Disconnect if connected
    pool::disconnect_server(server_name).await.ok();

    // Re-fetch config and reconnect
    if let Some(cfg) = server_service::get_by_name(server_name).await? {
        if cfg.enabled {
            pool::connect_server(&cfg).await;
        }
    }
    Ok(())
}

/// Toggle enabled/disabled for a server
pub async fn toggle_server(server_name: &str) -> Result<bool> {
    let cfg = server_service::toggle_enabled(server_name).await?;
    if cfg.enabled {
        pool::connect_server(&cfg).await;
    } else {
        pool::disconnect_server(server_name).await.ok();
    }
    Ok(cfg.enabled)
}
