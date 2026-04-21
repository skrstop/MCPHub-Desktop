/// Global MCP connection pool — manages live connections to all enabled servers.
use super::{
    client::McpClient,
    http_transport::HttpTransport,
    sse_transport::SseTransport,
    stdio_transport::StdioTransport,
};
use crate::models::server::{ServerConfig, ServerStatus, ServerType, Tool, ToolCallResult};
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};
use tokio::sync::RwLock;

/// Holds a live client + last known status + cached tools
struct PoolEntry {
    client: McpClient,
    status: ServerStatus,
    tools: Vec<Tool>,  // cached at connect time; refreshed on reconnect
}

type Pool = Arc<RwLock<HashMap<String, PoolEntry>>>;

static POOL: OnceLock<Pool> = OnceLock::new();

fn pool() -> &'static Pool {
    POOL.get_or_init(|| Arc::new(RwLock::new(HashMap::new())))
}

/// Build a McpClient from a ServerConfig
fn build_client(cfg: &ServerConfig) -> Result<McpClient> {
    let name = cfg.name.clone();
    match cfg.server_type {
        ServerType::Stdio => {
            let command = cfg
                .command
                .as_deref()
                .ok_or_else(|| anyhow!("stdio server '{}' missing command", name))?
                .to_string();
            let args = cfg.args.clone().unwrap_or_default();
            let env = cfg.env.clone().unwrap_or_default();
            let transport = StdioTransport::new(&name, command, args, env);
            Ok(McpClient::new(name, Box::new(transport)))
        }
        ServerType::Sse => {
            let url = cfg
                .url
                .as_deref()
                .ok_or_else(|| anyhow!("SSE server '{}' missing url", name))?
                .to_string();
            let headers = cfg.headers.clone().unwrap_or_default();
            let transport = SseTransport::new(&name, url, headers);
            Ok(McpClient::new(name, Box::new(transport)))
        }
        ServerType::StreamableHttp => {
            let url = cfg
                .url
                .as_deref()
                .ok_or_else(|| anyhow!("HTTP server '{}' missing url", name))?
                .to_string();
            let headers = cfg.headers.clone().unwrap_or_default();
            let transport = HttpTransport::new(&name, url, headers);
            Ok(McpClient::new(name, Box::new(transport)))
        }
        ServerType::Openapi => {
            Err(anyhow!("OpenAPI servers are not yet supported in desktop client"))
        }
    }
}

/// Connect a single server and insert into pool
pub async fn connect_server(cfg: &ServerConfig) -> ServerStatus {
    let name = cfg.name.clone();
    let mut entry_client = match build_client(cfg) {
        Ok(c) => c,
        Err(e) => {
            log::error!("[{}] Failed to build client: {}", name, e);
            return ServerStatus {
                name: name.clone(),
                connected: false,
                tool_count: 0,
                error: Some(e.to_string()),
                last_connected: None,
            };
        }
    };

    match entry_client.connect().await {
        Ok(()) => {
            let tools = entry_client.list_tools().await.unwrap_or_default();
            let tool_count = tools.len();
            let last_connected = Some(chrono::Utc::now().to_rfc3339());
            let status = ServerStatus {
                name: name.clone(),
                connected: true,
                tool_count,
                error: None,
                last_connected,
            };
            let mut map = pool().write().await;
            map.insert(
                name.clone(),
                PoolEntry {
                    client: entry_client,
                    status: status.clone(),
                    tools,
                },
            );
            log::info!("[{}] Connected ({} tools)", name, tool_count);
            status
        }
        Err(e) => {
            log::error!("[{}] Connect failed: {}", name, e);
            ServerStatus {
                name,
                connected: false,
                tool_count: 0,
                error: Some(e.to_string()),
                last_connected: None,
            }
        }
    }
}

/// Disconnect and remove a server from the pool.
/// The entry is removed from the map while holding the write lock, then the
/// actual disconnect I/O happens after the lock is released so that read
/// operations are not blocked during the network round-trip.
pub async fn disconnect_server(name: &str) -> Result<()> {
    let entry = {
        let mut map = pool().write().await;
        map.remove(name)
    }; // write lock released before any I/O
    if let Some(mut e) = entry {
        e.client.disconnect().await?;
    }
    Ok(())
}

/// Get status for all servers in pool
pub async fn get_all_statuses() -> Vec<ServerStatus> {
    let map = pool().read().await;
    map.values().map(|e| e.status.clone()).collect()
}

/// Get status for a single server
pub async fn get_status(name: &str) -> Option<ServerStatus> {
    let map = pool().read().await;
    map.get(name).map(|e| e.status.clone())
}

/// List all tools across connected servers (returns cached list, no network call)
pub async fn list_all_tools() -> Vec<Tool> {
    let map = pool().read().await;
    map.values()
        .filter(|e| e.status.connected)
        .flat_map(|e| e.tools.clone())
        .collect()
}

/// List tools for a specific server (returns cached list, no network call)
pub async fn list_tools_for(server_name: &str) -> Result<Vec<Tool>> {
    let map = pool().read().await;
    let entry = map.get(server_name).ok_or_else(|| anyhow!("Server '{}' not connected", server_name))?;
    Ok(entry.tools.clone())
}

/// Get status + cached tools for a server in a single lock acquisition
pub async fn get_entry_info(name: &str) -> Option<(ServerStatus, Vec<Tool>)> {
    let map = pool().read().await;
    map.get(name).map(|e| (e.status.clone(), e.tools.clone()))
}

/// Call a tool — automatically routes to the correct server
pub async fn call_tool(server_name: &str, tool_name: &str, arguments: Value) -> Result<ToolCallResult> {
    let map = pool().read().await;
    let entry = map
        .get(server_name)
        .ok_or_else(|| anyhow!("Server '{}' not connected", server_name))?;
    entry.client.call_tool(tool_name, arguments).await
}
