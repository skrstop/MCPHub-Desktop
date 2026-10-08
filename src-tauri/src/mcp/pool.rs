/// Global MCP connection pool — manages live connections to all enabled servers.
use super::{
    client::McpClient,
    openapi_transport::{OpenApiConfig as TransportOpenApiConfig, OpenApiSecurity as TransportOpenApiSecurity, OpenapiTransport},
    rmcp_http_transport::RmcpHttpTransport,
    sse_transport::SseTransport,
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
    /// Arc<Mutex<>> so tool calls can run OUTSIDE the pool read lock: a slow
    /// upstream call (up to 600s) used to pin the read lock and freeze every
    /// concurrent reader (UI status polling, list_all_tools) behind any queued
    /// writer (30s reconnect loop / reload). The per-client Mutex preserves
    /// the old call-vs-disconnect serialization for the same server.
    client: Option<std::sync::Arc<tokio::sync::Mutex<McpClient>>>,
    status: ServerStatus,
    tools: Vec<Tool>,  // cached at connect time; refreshed on reconnect
    /// Cached `per_session_client` flag from the server config. When true,
    /// the HTTP MCP path routes `tools/call` to a per-session isolated client
    /// (see `session_pool`) instead of this shared client.
    per_session_client: bool,
    /// Cached `start_on_demand` flag (stdio only). When true the server skips
    /// startup connect and is lazily spawned by `on_demand::call_tool_on_demand`
    /// on the first tool call. The live client lives in the on-demand store,
    /// not here (`client` stays `None`); this entry is a "shadow" carrying
    /// status + cached tools + the flag.
    start_on_demand: bool,
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
            // rmcp-backed transport (AGENTS.md §3.9.4): official SDK protocol
            // layer, all desktop behaviours (runtime_env resolution, download
            // progress, stderr tail, process-tree kill) preserved.
            let transport = super::rmcp_stdio_transport::RmcpStdioTransport::new(&name, command, args, env);
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
            // rmcp-backed transport (AGENTS.md §3.9.6): official SDK protocol
            // layer — Auto lifecycle (discover probe → legacy fallback),
            // native MRTR, native tasks/get polling, custom headers.
            let transport =
                RmcpHttpTransport::new(&name, url, headers);
            Ok(McpClient::new(name, Box::new(transport)))
        }
        ServerType::Openapi => {
            let openapi_cfg = cfg.openapi.as_ref()
                .ok_or_else(|| anyhow!("OpenAPI server '{}' missing openapi config", name))?;

            let security = openapi_cfg.security.as_ref().map(|s| {
                let security_type = s.security_type.clone();
                // Malformed configs (declared type but missing payload) must not
                // panic the connect path — fall back to an unauthenticated
                // bearer/no-op security and let the upstream 401 speak.
                match (security_type.as_str(), s) {
                    ("apiKey", crate::models::server::OpenApiSecurity { api_key: Some(ak), .. }) => {
                        TransportOpenApiSecurity::ApiKey {
                            name: ak.name.clone(),
                            location: ak.location.clone(),
                            value: ak.value.clone(),
                        }
                    }
                    ("http", crate::models::server::OpenApiSecurity { http: Some(h), .. }) => {
                        TransportOpenApiSecurity::Http {
                            scheme: h.scheme.clone(),
                            credentials: h.credentials.clone(),
                        }
                    }
                    ("oauth2", crate::models::server::OpenApiSecurity { oauth2: Some(o), .. }) => {
                        TransportOpenApiSecurity::OAuth2 {
                            token: o.token.clone(),
                        }
                    }
                    ("openIdConnect", crate::models::server::OpenApiSecurity { open_id_connect: Some(oidc), .. }) => {
                        TransportOpenApiSecurity::OpenIdConnect {
                            url: oidc.url.clone(),
                            token: oidc.token.clone(),
                        }
                    }
                    // Malformed security config (declared type but missing
                    // payload): fall back to no auth at all — the spec's own
                    // security schemes may still satisfy the upstream. An
                    // empty bearer used to produce a garbage Authorization
                    // header that MASKED the spec's scheme.
                    _ => TransportOpenApiSecurity::ApiKey {
                        name: String::new(),
                        location: "header".to_string(),
                        value: String::new(),
                    }
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
        // Builtin servers (RAG) are virtual - they have no transport and are
        // never connected to the pool. Their tools come from rag::service and
        // are aggregated separately. Reaching here is a logic error.
        ServerType::Builtin => Err(anyhow!("builtin server '{}' has no transport", name)),
    }
}

/// Connect a single server and insert into pool.
/// Immediately inserts a "starting" placeholder so the frontend can show "connecting" status
/// while the actual connect (process spawn, handshake) is in progress.
/// Per-name connect lock, shared by connect_server AND disconnect_server so a
/// disable racing an in-flight connect cannot be resurrected by the connect's
/// final entry insert (the disconnect waits for the connect flow to finish).
static CONNECT_LOCKS: std::sync::OnceLock<tokio::sync::Mutex<HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>> =
    std::sync::OnceLock::new();

pub(crate) async fn pool_connect_lock(name: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    CONNECT_LOCKS
        .get_or_init(|| tokio::sync::Mutex::new(HashMap::new()))
        .lock()
        .await
        .entry(name.to_string())
        .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

pub async fn connect_server(cfg: &ServerConfig) -> ServerStatus {
    let name = cfg.name.clone();
    let per_session_client = cfg.per_session_client.unwrap_or(false);

    // 0. Check if already connecting (prevent re-entry from rapid disable/enable clicks)
    // Per-server connect serialization: the `starting` re-entry guard below
    // reads under a read lock and releases before the placeholder is inserted
    // — two concurrent connect_server calls for the same name could both pass
    // the guard, build clients, and the later map.insert would overwrite the
    // first LIVE client (orphaning its stdio grandchild tree). The per-name
    // mutex is held for the ENTIRE connect (guard-check → placeholder-insert →
    // retries → final state): slow connects DO serialize subsequent connects
    // for the same name. That is the correct semantics — a concurrent connect
    // must never tear down a freshly built runtime. Do NOT "optimize" the
    // guard to release before connect based on the old (wrong) comment.
    let name_lock = pool_connect_lock(&name).await;
    let _connect_guard = name_lock.lock().await;
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
    // `disconnect_server_inner` (we already hold the per-name lock) also reaps
    // any live on-demand client (so reloading an awake on-demand server kills
    // the old process before re-inserting the sleeping placeholder below).
    disconnect_server_inner(&name).await.ok();

    // 1a. On-demand stdio servers skip startup connect: insert a "sleeping"
    // placeholder (client None, connected false, start_on_demand true) and
    // return. The process is spawned lazily on the first tool call by
    // `on_demand::call_tool_on_demand`.
    let start_on_demand = cfg.start_on_demand.unwrap_or(false) && cfg.server_type == ServerType::Stdio;
    if start_on_demand {
        let sleep_msg = format!("[{}] Skipping startup connect for on-demand server (sleeping)", name);
        log::info!("{}", sleep_msg);
        app_logger::log_to_db("info", &sleep_msg);
        let status = ServerStatus {
            name: name.clone(),
            connected: false,
            starting: false,
            start_on_demand: true,
            tool_count: 0,
            error: None,
            last_connected: None,
            server_version: None,
        };
        let mut map = pool().write().await;
        map.insert(name.clone(), PoolEntry {
            client: None,
            status: status.clone(),
            tools: vec![],
            per_session_client,
            start_on_demand: true,
        });
        return status;
    }

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
                start_on_demand: false,
                tool_count: 0,
                error: None,
                last_connected: None,
                server_version: None,
            },
            tools: vec![],
            per_session_client,
            start_on_demand: false,
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
                    start_on_demand: false,
                    tool_count: 0,
                    error: Some(e.to_string()),
                    last_connected: None,
                    server_version: None,
                };
                let mut map = pool().write().await;
                map.insert(name.clone(), PoolEntry { client: None, status: status.clone(), tools: vec![], per_session_client, start_on_demand: false });
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
                // rmcp peer requests have no default timeout; a server that
                // completes initialize then stalls on tools/list would leave
                // the placeholder "starting" forever (re-entry guard skips
                // every subsequent connect). Bound the tool discovery.
                let (tools, _tools_ttl) = match tokio::time::timeout(
                    std::time::Duration::from_secs(60),
                    entry_client.list_tools_with_ttl(),
                )
                .await
                {
                    // Err (JSON-RPC error from the server, e.g. method not
                    // found) degrades to an empty tool list but must not be
                    // silent — a "connected, 0 tools" server with no error
                    // hint is a diagnosis dead-end.
                    Ok(Ok((t, ttl))) => {
                        crate::services::list_freshness::record(&name, "tools", ttl);
                        (t, ttl)
                    }
                    Ok(Err(e)) => {
                        let msg = format!("[{}] tools/list failed post-handshake (continuing with 0 tools): {}", name, e);
                        log::warn!("{}", msg);
                        app_logger::log_to_db("warn", &msg);
                        crate::services::list_freshness::record(&name, "tools", None);
                        (Vec::new(), None)
                    }
                    Err(_) => {
                        let msg = format!(
                            "[{}] tools/list timed out after 60s post-handshake — treating as failure",
                            name
                        );
                        log::warn!("{}", msg);
                        app_logger::log_to_db("warn", &msg);
                        let _ = entry_client.disconnect().await;
                        last_error = "tools/list timed out after handshake".to_string();
                        continue;
                    }
                };
                let tools_for_index = tools.clone();
                let tool_count = tools.len();
                // Capture the server-reported version before moving the client
                // into the pool, for a best-effort "update available" check.
                let running_version = entry_client.server_version();
                let last_connected = Some(chrono::Utc::now().to_rfc3339());
                let status = ServerStatus {
                    name: name.clone(),
                    connected: true,
                    starting: false,
                    start_on_demand: false,
                    tool_count,
                    error: None,
                    last_connected,
                    server_version: running_version.clone(),
                };
                let mut map = pool().write().await;
                map.insert(name.clone(), PoolEntry {
                    client: Some(std::sync::Arc::new(tokio::sync::Mutex::new(entry_client))),
                    status: status.clone(),
                    tools,
                    per_session_client,
                    start_on_demand: false,
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
                // Smart Routing index (Phase 3): background, skip-check makes
                // repeat connects cheap; errors are logged inside.
                crate::smart_routing::index::on_server_connected(
                    name.clone(),
                    tools_for_index,
                    cfg.description.clone(),
                );
                return status;
            }
            Ok(Err(e)) => {
                last_error = e.to_string();
                // Handshake failed — disconnect the half-built client like the
                // timeout branch (and session_pool/on_demand) do: rmcp drops
                // the transport on error which SIGKILLs only the direct child;
                // without an explicit disconnect npx/uvx grandchildren orphan.
                let _ = entry_client.disconnect().await;
                // Retry on transient errors. rmcp 3.4.1 reports a child that
                // died mid-handshake as `connection closed: …` (service/client.rs
                // ClientInitializeError) or `Send message error …`; the legacy
                // string "child process exited" came from the old handwritten
                // transport and never matches post-migration, which silently
                // disabled the whole retry mechanism.
                let transient = last_error.contains("child process exited")
                    || last_error.contains("connection closed")
                    || last_error.contains("Send message error");
                if attempt < MAX_RETRIES && transient {
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
                    start_on_demand: false,
                    tool_count: 0,
                    error: Some(last_error.clone()),
                    last_connected: None,
                    server_version: None,
                };
                let mut map = pool().write().await;
                map.insert(name.clone(), PoolEntry { client: None, status: status.clone(), tools: vec![], per_session_client, start_on_demand: false });
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
                    start_on_demand: false,
                    tool_count: 0,
                    error: Some(last_error.clone()),
                    last_connected: None,
                    server_version: None,
                };
                let mut map = pool().write().await;
                map.insert(name.clone(), PoolEntry { client: None, status: status.clone(), tools: vec![], per_session_client, start_on_demand: false });
                return status;
            }
        }
    }

    // Defensive fallback: under the current control flow every path inside the
    // loop either returns or `continue`s (gated on attempt < MAX_RETRIES), so
    // this block is unreachable today. It stays as insurance only — if a future
    // edit introduces a fall-through arm, this still produces a correct
    // terminal status instead of an unhandled fall-out.
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
        start_on_demand: false,
        tool_count: 0,
        error: Some(last_error),
        last_connected: None,
        server_version: None,
    };
    let mut map = pool().write().await;
    map.insert(name.clone(), PoolEntry { client: None, status: status.clone(), tools: vec![], per_session_client, start_on_demand: false });
    status
}

/// Disconnect and remove a server from the pool.
/// The entry is removed from the map while holding the write lock, then the
/// actual disconnect I/O happens after the lock is released so that read
/// operations are not blocked during the network round-trip.
/// Drop the per-name connect lock entry for a deleted/renamed server.
/// Without this, CONNECT_LOCKS grows without bound over a long session
/// (one entry per name ever seen). Only call from server delete/rename —
/// NOT from disconnect: removing while a concurrent connect holds the Arc
/// would let a new connect mint a fresh lock and run concurrently.
pub async fn forget_connect_lock(name: &str) {
    if let Some(m) = CONNECT_LOCKS.get() {
        m.lock().await.remove(name);
    }
}

pub async fn disconnect_server(name: &str) -> Result<()> {
    // Take the same per-name connect lock connect_server holds for its ENTIRE
    // flow. Without it, a disable racing an in-flight connect lets the connect
    // finish AFTER our removal and insert a connected entry back — a "disabled"
    // server stays alive until a second disable. The helper shares the SAME
    // lock table as connect_server (Arc equality guaranteed).
    let _disconnect_guard = pool_connect_lock(name).await;
    disconnect_server_inner(name).await
}

/// Lock-free core of disconnect_server — call only while ALREADY holding the
/// per-name connect lock (connect_server calls this mid-flow; re-taking the
/// tokio Mutex there would self-deadlock since it is not reentrant).
pub(crate) async fn disconnect_server_inner(name: &str) -> Result<()> {
    log::info!("[{}] Disconnecting...", name);
    app_logger::log_to_db("info", &format!("[{}] Disconnecting...", name));
    // Tear down any per-session isolated upstream clients for this server too
    // (perSessionClient servers spawn a child process per session; without this
    // they'd leak on delete/disable/reload/update/reinstall). No-op for
    // shared-pool servers (read-lock fast path inside).
    super::session_pool::cleanup_server(name).await;
    // Tear down any live on-demand client (awake stdio server) so its child
    // process is reaped on disable/reload/delete/update. No-op when the server
    // is sleeping or not on-demand.
    super::on_demand::shutdown_on_demand_lifecycle(name).await;
    let entry = {
        let mut map = pool().write().await;
        map.remove(name)
    }; // write lock released before any I/O
    crate::services::list_freshness::invalidate_server(name);
    if let Some(mut e) = entry {
        if let Some(client) = e.client.take() {
            let mut c = client.lock().await;
            c.disconnect().await?;
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
    // On-demand clients live in a separate store; reap them too so their child
    // processes are killed via kill_process_tree rather than relying on
    // kill_on_drop at process exit.
    super::on_demand::cleanup_all_on_demand().await;

    let entries: Vec<(String, PoolEntry)> = {
        let mut map = pool().write().await;
        map.drain().collect()
    }; // write lock released before any I/O

    for (name, mut e) in entries {
        if let Some(client) = e.client.take() {
            let mut c = client.lock().await;
            if let Err(err) = c.disconnect().await {
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
        .filter(|e| e.status.connected || (e.start_on_demand && !e.tools.is_empty()))
        .flat_map(|e| e.tools.clone())
        .collect()
}

/// List tools for a specific server (returns cached list, no network call)
pub async fn list_tools_for(server_name: &str) -> Result<Vec<Tool>> {
    // Builtin "mcphub-desktop" server is virtual (no pool entry). Its tools are
    // the RAG builtin tools - return them so the Tauri call_tool path and the
    // "servers" panel can list/enable/disable them like any other server.
    if server_name == crate::rag::service::BUILTIN_SERVER_NAME {
        let mut tools = crate::rag::service::builtin_tools();
        // Smart Routing meta tools ride on the builtin server too (UI listing
        // + test invocation) — empty when Smart Routing is disabled.
        tools.extend(crate::smart_routing::meta::builtin_meta_tools().await);
        return Ok(tools);
    }
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
/// A2/A9: call_tool variant forwarding request-level `_meta` upstream
/// (MRTR inputResponses retry, OTel trace keys). Falls back to the plain
/// path for builtin/on-demand/isolated routing (they drop _meta).
pub async fn call_tool_with_meta(
    server_name: &str,
    tool_name: &str,
    arguments: Value,
    request_meta: Option<Value>,
) -> Result<ToolCallResult> {
    // NOTE: `request_meta` is consumed further down by the shared-pool path
    // (forwarded to transports via call_tool_with_meta); the builtin/on-demand
    // early returns below take no meta — those paths simply don't carry one.
    if server_name == crate::rag::service::BUILTIN_SERVER_NAME {
        return call_tool(server_name, tool_name, arguments).await;
    }
    let on_demand = {
        let map = pool().read().await;
        map.get(server_name).map(|e| e.start_on_demand).unwrap_or(false)
    };
    if on_demand {
        return super::on_demand::call_tool_on_demand(server_name, tool_name, arguments).await;
    }
    let client_arc = {
        let map = pool().read().await;
        let entry = map
            .get(server_name)
            .ok_or_else(|| anyhow!("Server '{}' not connected", server_name))?;
        entry
            .client
            .clone()
            .ok_or_else(|| anyhow!("Server '{}' is still starting", server_name))?
    }; // pool read lock released before the upstream call
    let client = client_arc.lock().await;
    let result = client.call_tool_with_meta(tool_name, arguments, request_meta).await;
    drop(client);
    result
}

pub async fn call_tool(server_name: &str, tool_name: &str, arguments: Value) -> Result<ToolCallResult> {
    // Builtin "mcphub-desktop" server (RAG tools): virtual, not in the pool.
    // Route to the RAG dispatch so invoking rag_* via the Tauri call_tool
    // command / "servers" panel works instead of erroring "not connected".
    if server_name == crate::rag::service::BUILTIN_SERVER_NAME {
        // Smart Routing meta tools (smart_route_search / smart_route_describe / smart_route_call):
        // global scope, executed in-process. Checked BEFORE RAG dispatch —
        // names never collide (rag_* vs meta names).
        if crate::smart_routing::meta::is_meta_tool(tool_name) {
            // Box::pin: meta call_tool re-enters pool::call_tool for the
            // RESOLVED real tool (different name) — a legal mutual recursion
            // the compiler can't size without boxing.
            return Box::pin(crate::smart_routing::meta::call_meta_tool_builtin(
                tool_name,
                &arguments,
            ))
            .await;
        }
        let app = crate::mcp::progress::get_app_handle()
            .ok_or_else(|| anyhow!("app handle unavailable"))?;
        return crate::rag::service::call_builtin_tool(&app, tool_name, &arguments).await;
    }
    // On-demand stdio servers keep their live client in the on-demand store
    // (the pool entry is a sleeping shadow). Route there so the call lazily
    // spawns the process on first use.
    let on_demand = {
        let map = pool().read().await;
        map.get(server_name).map(|e| e.start_on_demand).unwrap_or(false)
    };
    if on_demand {
        return super::on_demand::call_tool_on_demand(server_name, tool_name, arguments).await;
    }

    let client_arc = {
        let map = pool().read().await;
        let entry = map
            .get(server_name)
            .ok_or_else(|| anyhow!("Server '{}' not connected", server_name))?;
        entry
            .client
            .clone()
            .ok_or_else(|| anyhow!("Server '{}' is still starting", server_name))?
    }; // pool read lock released before the upstream call

    log::debug!("[{}] Calling tool '{}'...", server_name, tool_name);
    let client = client_arc.lock().await;
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

// ---------------------------------------------------------------------------
// On-demand spawning: pool-placeholder status mutators.
//
// On-demand stdio servers keep their live client in `on_demand::ON_DEMAND_CLIENTS`;
// the pool entry is a "shadow" (client always None) carrying status + cached
// tools + the `start_on_demand` flag. These helpers let the on-demand module
// update the shadow's status as it wakes/sleeps/errors without exposing the
// pool's internal lock.
// ---------------------------------------------------------------------------

/// Mark an on-demand server's shadow entry as awake (just spawned). Updates
/// status to connected + caches the tool list + server version. Keeps the
/// `start_on_demand` flag set. No-op if the entry was removed (server
/// disabled/deleted while spawning).
pub(crate) async fn mark_on_demand_awake(name: &str, tools: Vec<Tool>, server_version: Option<String>) {
    let tool_count = tools.len();
    let last_connected = Some(chrono::Utc::now().to_rfc3339());
    let mut map = pool().write().await;
    if let Some(entry) = map.get_mut(name) {
        // Identity guard: only mutate entries that are on-demand shadows.
        // If the shadow was replaced by a live entry (server disabled ->
        // re-enabled as a normal always-connected server while the cold
        // spawn was in flight), stamping connected=true here would fabricate
        // a connected status backed by no client in the pool.
        if !entry.start_on_demand {
            return;
        }
        entry.tools = tools;
        entry.status.connected = true;
        entry.status.starting = false;
        entry.status.start_on_demand = true;
        entry.status.tool_count = tool_count;
        entry.status.error = None;
        entry.status.last_connected = last_connected;
        entry.status.server_version = server_version;
    }
}

/// Mark an on-demand server's shadow entry as sleeping (idle timeout or stale
/// connection). Sets connected false but KEEPS the cached tools so the server
/// stays discoverable. Clears any prior error.
pub(crate) async fn mark_on_demand_sleeping(name: &str) {
    let mut map = pool().write().await;
    if let Some(entry) = map.get_mut(name) {
        // Identity guard: a live (non-on-demand) entry must never be flipped
        // to disconnected by an on-demand idle timer firing late — that would
        // report a connected server as down.
        if !entry.start_on_demand {
            return;
        }
        entry.status.connected = false;
        entry.status.starting = false;
        entry.status.start_on_demand = true;
        entry.status.error = None;
        // tools intentionally preserved
    }
}

/// Mark an on-demand server's shadow entry with a spawn failure error. The
/// server stays sleeping (connected false) but the error is surfaced so the
/// frontend can show why a cold-start failed rather than rendering "Sleeping".
pub(crate) async fn mark_on_demand_error(name: &str, error: String) {
    let mut map = pool().write().await;
    if let Some(entry) = map.get_mut(name) {
        // Identity guard: same rationale as awake/sleeping — never stamp an
        // error onto a live entry that has a real client backing it.
        if !entry.start_on_demand {
            return;
        }
        entry.status.connected = false;
        entry.status.starting = false;
        entry.status.start_on_demand = true;
        entry.status.error = Some(error);
    }
}
