/// Global MCP connection pool — manages live connections to all enabled servers.
use super::{
    client::McpClient,
    http_transport::HttpTransport,
    openapi_transport::{OpenApiConfig as TransportOpenApiConfig, OpenApiSecurity as TransportOpenApiSecurity, OpenapiTransport},
    sse_transport::SseTransport,
    stdio_transport::StdioTransport,
};
use crate::models::server::{ServerConfig, ServerStatus, ServerType, Tool, ToolCallResult};
use crate::services::app_logger;
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};
use tokio::sync::RwLock;

/// Holds a live client + last known status + cached tools
struct PoolEntry {
    client: Option<McpClient>,
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
            let openapi_cfg = cfg.openapi.as_ref()
                .ok_or_else(|| anyhow!("OpenAPI server '{}' missing openapi config", name))?;

            let security = openapi_cfg.security.as_ref().map(|s| {
                let security_type = s.security_type.clone();
                match security_type.as_str() {
                    "apiKey" => {
                        let ak = s.api_key.as_ref().unwrap();
                        TransportOpenApiSecurity::ApiKey {
                            name: ak.name.clone(),
                            location: ak.location.clone(),
                            value: ak.value.clone(),
                        }
                    }
                    "http" => {
                        let h = s.http.as_ref().unwrap();
                        TransportOpenApiSecurity::Http {
                            scheme: h.scheme.clone(),
                            credentials: h.credentials.clone(),
                        }
                    }
                    "oauth2" => {
                        let o = s.oauth2.as_ref().unwrap();
                        TransportOpenApiSecurity::OAuth2 {
                            token: o.token.clone(),
                        }
                    }
                    "openIdConnect" => {
                        let oidc = s.open_id_connect.as_ref().unwrap();
                        TransportOpenApiSecurity::OpenIdConnect {
                            url: oidc.url.clone(),
                            token: oidc.token.clone(),
                        }
                    }
                    _ => TransportOpenApiSecurity::Http {
                        scheme: "bearer".to_string(),
                        credentials: String::new(),
                    },
                }
            });

            // Build passthrough headers from the config
            let mut passthrough_headers = HashMap::new();
            for header_name in &openapi_cfg.passthrough_headers {
                if let Some(ref hdrs) = cfg.headers {
                    if let Some(val) = hdrs.get(header_name) {
                        passthrough_headers.insert(header_name.clone(), val.clone());
                    }
                }
            }

            let transport_config = TransportOpenApiConfig {
                spec_url: openapi_cfg.url.clone(),
                spec_schema: openapi_cfg.schema.clone(),
                version: openapi_cfg.version.clone(),
                security,
                passthrough_headers,
                headers: cfg.headers.clone().unwrap_or_default(),
            };

            let transport = OpenapiTransport::new(&name, transport_config);
            Ok(McpClient::new(name, Box::new(transport)))
        }
    }
}

/// Connect a single server and insert into pool.
/// Immediately inserts a "starting" placeholder so the frontend can show "connecting" status
/// while the actual connect (process spawn, handshake) is in progress.
pub async fn connect_server(cfg: &ServerConfig) -> ServerStatus {
    let name = cfg.name.clone();

    // 1. Insert "starting" placeholder immediately
    let start_msg = format!("[{}] Starting connection (type={:?})...", name, cfg.server_type);
    log::info!("{}", start_msg);
    app_logger::log_to_db("info", &start_msg);
    {
        let mut map = pool().write().await;
        map.insert(name.clone(), PoolEntry {
            client: None,
            status: ServerStatus {
                name: name.clone(),
                connected: false,
                starting: true,
                tool_count: 0,
                error: None,
                last_connected: None,
            },
            tools: vec![],
        });
    }

    // 2. Build client
    let mut entry_client = match build_client(cfg) {
        Ok(c) => c,
        Err(e) => {
            log::error!("[{}] Failed to build client: {}", name, e);
            let status = ServerStatus {
                name: name.clone(),
                connected: false,
                starting: false,
                tool_count: 0,
                error: Some(e.to_string()),
                last_connected: None,
            };
            let mut map = pool().write().await;
            map.insert(name.clone(), PoolEntry { client: None, status: status.clone(), tools: vec![] });
            return status;
        }
    };

    // 3. Attempt connect
    match entry_client.connect().await {
        Ok(()) => {
            let tools = entry_client.list_tools().await.unwrap_or_default();
            let tool_count = tools.len();
            let last_connected = Some(chrono::Utc::now().to_rfc3339());
            let status = ServerStatus {
                name: name.clone(),
                connected: true,
                starting: false,
                tool_count,
                error: None,
                last_connected,
            };
            let mut map = pool().write().await;
            map.insert(
                name.clone(),
                PoolEntry {
                    client: Some(entry_client),
                    status: status.clone(),
                    tools,
                },
            );
            let conn_msg = format!("[{}] Connected ({} tools)", name, tool_count);
            log::info!("{}", conn_msg);
            app_logger::log_to_db("info", &conn_msg);
            status
        }
        Err(e) => {
            let err_msg = format!("[{}] Connect failed: {}", name, e);
            log::error!("{}", err_msg);
            app_logger::log_to_db("error", &err_msg);
            let status = ServerStatus {
                name: name.clone(),
                connected: false,
                starting: false,
                tool_count: 0,
                error: Some(e.to_string()),
                last_connected: None,
            };
            let mut map = pool().write().await;
            map.insert(name.clone(), PoolEntry { client: None, status: status.clone(), tools: vec![] });
            status
        }
    }
}

/// Disconnect and remove a server from the pool.
/// The entry is removed from the map while holding the write lock, then the
/// actual disconnect I/O happens after the lock is released so that read
/// operations are not blocked during the network round-trip.
pub async fn disconnect_server(name: &str) -> Result<()> {
    log::info!("[{}] Disconnecting...", name);
    app_logger::log_to_db("info", &format!("[{}] Disconnecting...", name));
    let entry = {
        let mut map = pool().write().await;
        map.remove(name)
    }; // write lock released before any I/O
    if let Some(mut e) = entry {
        if let Some(mut client) = e.client.take() {
            client.disconnect().await?;
        }
    }
    log::info!("[{}] Disconnected", name);
    app_logger::log_to_db("info", &format!("[{}] Disconnected", name));
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
    let client = entry.client.as_ref()
        .ok_or_else(|| anyhow!("Server '{}' is still starting", server_name))?;

    log::debug!("[{}] Calling tool '{}'...", server_name, tool_name);
    let result = client.call_tool(tool_name, arguments).await;
    match &result {
        Ok(r) => {
            let status = if r.is_error { "error" } else { "success" };
            log::debug!("[{}] Tool '{}' call {}", server_name, tool_name, status);
        }
        Err(e) => {
            log::warn!("[{}] Tool '{}' call failed: {}", server_name, tool_name, e);
        }
    }
    result
}
