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
use super::progress::{self, ServerInstallProgress};
use anyhow::{anyhow, Result};
use serde_json::Value;
use tokio::time::{timeout, Duration};
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
    /// Cached `per_session_client` flag from the server config. When true,
    /// the HTTP MCP path routes `tools/call` to a per-session isolated client
    /// (see `session_pool`) instead of this shared client.
    per_session_client: bool,
}

type Pool = Arc<RwLock<HashMap<String, PoolEntry>>>;

static POOL: OnceLock<Pool> = OnceLock::new();

fn pool() -> &'static Pool {
    POOL.get_or_init(|| Arc::new(RwLock::new(HashMap::new())))
}

/// Build a McpClient from a ServerConfig
pub(crate) fn build_client(cfg: &ServerConfig) -> Result<McpClient> {
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
    let per_session_client = cfg.per_session_client.unwrap_or(false);

    // 0. Check if already connecting (prevent re-entry from rapid disable/enable clicks)
    {
        let map = pool().read().await;
        if let Some(entry) = map.get(&name) {
            if entry.status.starting {
                log::warn!("[{}] Already connecting, skipping duplicate connect_server call", name);
                app_logger::log_to_db("warn", &format!("[{}] Already connecting, skipping duplicate connect", name));
                return entry.status.clone();
            }
        }
    }

    // 1. Clean up any existing entry (e.g., zombie process from a previous connection).
    // Also tears down any per-session isolated clients for this server via the
    // shared `disconnect_server` path — important on reconnect so a stale
    // perSessionClient child tree is reaped before spawning a fresh shared one.
    disconnect_server(&name).await.ok();

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
                server_version: None,
            },
            tools: vec![],
            per_session_client,
        });
    }

    // 2. Build client + connect with retry for transient failures
    const MAX_RETRIES: u32 = 3;
    let mut last_error = String::new();

    for attempt in 1..=MAX_RETRIES {
        // Build client
        let mut entry_client = match build_client(cfg) {
            Ok(c) => c,
            Err(e) => {
                log::error!("[{}] Failed to build client: {}", name, e);
                if progress::is_package_manager(&cfg.command) {
                    progress::emit_install_progress(&ServerInstallProgress {
                        server: name.clone(),
                        phase: "error".to_string(),
                        progress: None,
                        message: Some(e.to_string()),
                    });
                }
                let status = ServerStatus {
                    name: name.clone(),
                    connected: false,
                    starting: false,
                    tool_count: 0,
                    error: Some(e.to_string()),
                    last_connected: None,
                    server_version: None,
                };
                let mut map = pool().write().await;
                map.insert(name.clone(), PoolEntry { client: None, status: status.clone(), tools: vec![], per_session_client });
                return status;
            }
        };

        // Attempt connect (with timeout)
        let connect_result = timeout(
            Duration::from_secs(120),
            entry_client.connect(),
        ).await;

        match connect_result {
            Ok(Ok(())) => {
                let tools = entry_client.list_tools().await.unwrap_or_default();
                let tool_count = tools.len();
                // Capture the server-reported version before moving the client
                // into the pool, for a best-effort "update available" check.
                let running_version = entry_client.server_version();
                let last_connected = Some(chrono::Utc::now().to_rfc3339());
                let status = ServerStatus {
                    name: name.clone(),
                    connected: true,
                    starting: false,
                    tool_count,
                    error: None,
                    last_connected,
                    server_version: running_version.clone(),
                };
                let mut map = pool().write().await;
                map.insert(name.clone(), PoolEntry {
                    client: Some(entry_client),
                    status: status.clone(),
                    tools,
                    per_session_client,
                });
                let conn_msg = if attempt > 1 {
                    format!("[{}] Connected ({} tools) after {} attempts", name, tool_count, attempt)
                } else {
                    format!("[{}] Connected ({} tools)", name, tool_count)
                };
                log::info!("{}", conn_msg);
                app_logger::log_to_db("info", &conn_msg);
                // For npx/uvx servers: signal download done, then run a
                // background "update available" check (only on start, never
                // scheduled) comparing the running version to the registry.
                if progress::is_package_manager(&cfg.command) {
                    progress::emit_install_progress(&ServerInstallProgress {
                        server: name.clone(),
                        phase: "done".to_string(),
                        progress: Some(100),
                        message: Some("连接成功".to_string()),
                    });
                    progress::spawn_update_check(
                        name.clone(),
                        cfg.command.clone().unwrap_or_default(),
                        cfg.args.clone().unwrap_or_default(),
                        running_version,
                    );
                }
                return status;
            }
            Ok(Err(e)) => {
                last_error = e.to_string();
                // Retry on transient errors (child process exited unexpectedly)
                if attempt < MAX_RETRIES && last_error.contains("child process exited") {
                    let retry_msg = format!(
                        "[{}] Connect failed (attempt {}/{}): {} — retrying in 1s...",
                        name, attempt, MAX_RETRIES, last_error
                    );
                    log::warn!("{}", retry_msg);
                    app_logger::log_to_db("warn", &retry_msg);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
                // Non-transient error or retries exhausted
                let err_msg = format!("[{}] Connect failed after {} attempt(s): {}", name, attempt, last_error);
                log::error!("{}", err_msg);
                app_logger::log_to_db("error", &err_msg);
                if progress::is_package_manager(&cfg.command) {
                    progress::emit_install_progress(&ServerInstallProgress {
                        server: name.clone(),
                        phase: "error".to_string(),
                        progress: None,
                        message: Some(last_error.clone()),
                    });
                }
                let status = ServerStatus {
                    name: name.clone(),
                    connected: false,
                    starting: false,
                    tool_count: 0,
                    error: Some(last_error.clone()),
                    last_connected: None,
                    server_version: None,
                };
                let mut map = pool().write().await;
                map.insert(name.clone(), PoolEntry { client: None, status: status.clone(), tools: vec![], per_session_client });
                return status;
            }
            Err(_elapsed) => {
                last_error = "Connection timed out after 120 seconds".to_string();
                let err_msg = format!("[{}] Connect timed out after 120s", name);
                log::error!("{}", err_msg);
                app_logger::log_to_db("error", &err_msg);
                let _ = entry_client.disconnect().await;
                if progress::is_package_manager(&cfg.command) {
                    progress::emit_install_progress(&ServerInstallProgress {
                        server: name.clone(),
                        phase: "error".to_string(),
                        progress: None,
                        message: Some("连接超时".to_string()),
                    });
                }
                let status = ServerStatus {
                    name: name.clone(),
                    connected: false,
                    starting: false,
                    tool_count: 0,
                    error: Some(last_error.clone()),
                    last_connected: None,
                    server_version: None,
                };
                let mut map = pool().write().await;
                map.insert(name.clone(), PoolEntry { client: None, status: status.clone(), tools: vec![], per_session_client });
                return status;
            }
        }
    }

    // All retries exhausted (should not reach here for timeout, only for "child process exited")
    let err_msg = format!("[{}] Connect failed after {} attempts: {}", name, MAX_RETRIES, last_error);
    log::error!("{}", err_msg);
    app_logger::log_to_db("error", &err_msg);
    if progress::is_package_manager(&cfg.command) {
        progress::emit_install_progress(&ServerInstallProgress {
            server: name.clone(),
            phase: "error".to_string(),
            progress: None,
            message: Some(last_error.clone()),
        });
    }
    let status = ServerStatus {
        name: name.clone(),
        connected: false,
        starting: false,
        tool_count: 0,
        error: Some(last_error),
        last_connected: None,
        server_version: None,
    };
    let mut map = pool().write().await;
    map.insert(name.clone(), PoolEntry { client: None, status: status.clone(), tools: vec![], per_session_client });
    status
}

/// Disconnect and remove a server from the pool.
/// The entry is removed from the map while holding the write lock, then the
/// actual disconnect I/O happens after the lock is released so that read
/// operations are not blocked during the network round-trip.
pub async fn disconnect_server(name: &str) -> Result<()> {
    log::info!("[{}] Disconnecting...", name);
    app_logger::log_to_db("info", &format!("[{}] Disconnecting...", name));
    // Tear down any per-session isolated upstream clients for this server too
    // (perSessionClient servers spawn a child process per session; without this
    // they'd leak on delete/disable/reload/update/reinstall). No-op for
    // shared-pool servers (read-lock fast path inside).
    super::session_pool::cleanup_server(name).await;
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

/// Disconnect and remove **every** server from the shared pool.
///
/// Intended for application shutdown: drains all entries out of the map under a
/// single write lock, then disconnects each client (killing its child process
/// tree for stdio) *after* the lock is released so we never hold the pool lock
/// across I/O. Also clears per-session isolated clients via
/// [`super::session_pool::cleanup_all`].
///
/// Best-effort: disconnect errors are logged, not propagated, so one stuck
/// server doesn't block the rest of the shutdown.
pub async fn disconnect_all() {
    log::info!("[pool] Disconnecting all servers (shutdown)...");
    app_logger::log_to_db("info", "[pool] Disconnecting all servers (shutdown)");

    // Per-session isolated clients first — they reference the same upstream
    // servers; clearing them avoids killing the shared client out from under an
    // in-flight isolated call (though at shutdown that's moot anyway).
    super::session_pool::cleanup_all().await;

    let entries: Vec<(String, PoolEntry)> = {
        let mut map = pool().write().await;
        map.drain().collect()
    }; // write lock released before any I/O

    for (name, mut e) in entries {
        if let Some(mut client) = e.client.take() {
            if let Err(err) = client.disconnect().await {
                log::warn!("[{}] Error during shutdown disconnect: {}", name, err);
                app_logger::log_to_db("warn", &format!("[{}] Error during shutdown disconnect: {}", name, err));
            } else {
                log::info!("[{}] Disconnected (shutdown)", name);
                app_logger::log_to_db("info", &format!("[{}] Disconnected (shutdown)", name));
            }
        }
    }
    log::info!("[pool] All servers disconnected (shutdown)");
    app_logger::log_to_db("info", "[pool] All servers disconnected (shutdown)");
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

/// Whether a server is configured for per-session upstream client isolation.
/// Reads the cached flag from the pool entry (set at connect time); performs no
/// DB lookup. Returns `false` for servers not currently in the pool — those are
/// not reachable via `tools/call` anyway, so isolation routing is irrelevant.
pub async fn is_per_session_client(name: &str) -> bool {
    let map = pool().read().await;
    map.get(name).map(|e| e.per_session_client).unwrap_or(false)
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
