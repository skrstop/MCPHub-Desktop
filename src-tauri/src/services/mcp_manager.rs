/// mcp_manager — loads server configs from DB and connects all enabled servers at startup.
use crate::{mcp::pool, services::server_service};
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
