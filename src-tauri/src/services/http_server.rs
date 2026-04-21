/// http_server — embedded Axum HTTP server exposing MCP endpoints to external AI clients.
///
/// When `system_config.expose_http = true`, this service starts an HTTP server on the
/// configured port (default 3333). External clients can connect via SSE or Streamable HTTP
/// to access all connected MCP servers.
///
/// Endpoints:
///   GET  /health                    — health check
///   GET  /servers                   — list available servers
///   POST /mcp/{server}/call         — call a tool on a specific server
///   GET  /mcp/{server}/tools        — list tools for a server
///   POST /mcp/call                  — smart call: route to best server
use crate::{
    mcp::pool,
    models::server::Tool,
    services::{bearer_key_service, config_service, group_service, server_tool_config_service},
};
use axum::{
    extract::Path,
    http::{HeaderMap, StatusCode},
    response::Json,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    net::SocketAddr,
    sync::{Arc, OnceLock},
};
use tokio::{net::TcpListener, sync::Mutex};
use tower_http::cors::CorsLayer;

// ────────────────────────────────────────────────────────────────────────────
// Global server state
// ────────────────────────────────────────────────────────────────────────────

struct ServerHandle {
    abort_tx: tokio::sync::oneshot::Sender<()>,
    port: u16,
}

static SERVER_HANDLE: OnceLock<Arc<Mutex<Option<ServerHandle>>>> = OnceLock::new();

fn handle() -> &'static Arc<Mutex<Option<ServerHandle>>> {
    SERVER_HANDLE.get_or_init(|| Arc::new(Mutex::new(None)))
}

// ────────────────────────────────────────────────────────────────────────────
// Request / Response types
// ────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct CallToolRequest {
    tool: String,
    arguments: Option<Value>,
}

#[derive(Serialize)]
struct ServerInfo {
    name: String,
    connected: bool,
    tool_count: usize,
}

#[derive(Deserialize)]
struct SmartCallRequest {
    server: Option<String>,
    group: Option<String>,
    tool: String,
    arguments: Option<Value>,
}

// ────────────────────────────────────────────────────────────────────────────
// Auth helper
// ────────────────────────────────────────────────────────────────────────────

async fn check_bearer_auth(headers: &HeaderMap) -> Result<(), (StatusCode, Json<Value>)> {
    // Dynamically read config so changes take effect without restarting the HTTP server
    let enabled = match config_service::get().await {
        Ok(c) => c
            .get("bearerKeyEnabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        Err(_) => false,
    };
    if !enabled {
        return Ok(());
    }

    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !auth.starts_with("Bearer ") {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Missing Bearer token" })),
        ));
    }
    let token = &auth[7..];
    match bearer_key_service::find_by_token(token).await {
        Ok(Some(key)) if key.enabled => Ok(()),
        _ => Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Invalid or disabled Bearer token" })),
        )),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Handlers
// ────────────────────────────────────────────────────────────────────────────

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "service": "mcphub-desktop" }))
}

async fn list_servers(
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    check_bearer_auth(&headers).await?;

    let statuses = pool::get_all_statuses().await;
    let servers: Vec<ServerInfo> = statuses
        .into_iter()
        .map(|s| ServerInfo {
            name: s.name.clone(),
            connected: s.connected,
            tool_count: s.tool_count,
        })
        .collect();
    Ok(Json(json!({ "servers": servers })))
}

async fn list_server_tools(
    headers: HeaderMap,
    Path(server_name): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    check_bearer_auth(&headers).await?;

    let tools = pool::list_tools_for(&server_name).await.map_err(|e| {
        (StatusCode::NOT_FOUND, Json(json!({ "error": e.to_string() })))
    })?;
    let tools = server_tool_config_service::apply_tool_filters(&server_name, tools)
        .await
        .unwrap_or_else(|_| vec![]);
    Ok(Json(json!({ "tools": tools })))
}

async fn call_server_tool(
    headers: HeaderMap,
    Path(server_name): Path<String>,
    Json(req): Json<CallToolRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    check_bearer_auth(&headers).await?;

    let args = req.arguments.unwrap_or(json!({}));
    match pool::call_tool(&server_name, &req.tool, args).await {
        Ok(result) => Ok(Json(json!({ "result": result.content, "is_error": result.is_error }))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )),
    }
}

async fn list_group_tools(
    headers: HeaderMap,
    Path(group_name): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    check_bearer_auth(&headers).await?;

    let group = group_service::list_all()
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?
        .into_iter()
        .find(|g| g.name == group_name)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "Group not found" })),
            )
        })?;

    let mut tools: Vec<Tool> = Vec::new();
    for server_name in &group.servers {
        if let Ok(server_tools) = pool::list_tools_for(server_name).await {
            let filtered = server_tool_config_service::apply_tool_filters(server_name, server_tools)
                .await
                .unwrap_or_else(|_| vec![]);
            tools.extend(filtered);
        }
    }
    Ok(Json(json!({ "group": group.name, "tools": tools })))
}

async fn call_group_tool(
    headers: HeaderMap,
    Path(group_name): Path<String>,
    Json(req): Json<SmartCallRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    check_bearer_auth(&headers).await?;

    // Find which server in the group has the requested tool
    let group = group_service::list_all()
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?
        .into_iter()
        .find(|g| g.name == group_name)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "Group not found" })),
            )
        })?;

    let tool_name = &req.tool;
    let mut target_server: Option<String> = None;
    for server_name in &group.servers {
        if let Ok(tools) = pool::list_tools_for(server_name).await {
            if tools.iter().any(|t| &t.name == tool_name) {
                target_server = Some(server_name.clone());
                break;
            }
        }
    }

    let server_name = target_server.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("Tool '{}' not found in group '{}'", tool_name, group_name) })),
        )
    })?;

    let args = req.arguments.unwrap_or(json!({}));
    match pool::call_tool(&server_name, tool_name, args).await {
        Ok(result) => Ok(Json(json!({ "result": result.content, "is_error": result.is_error }))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Router
// ────────────────────────────────────────────────────────────────────────────

fn build_router() -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/servers", get(list_servers))
        .route("/mcp/:server/tools", get(list_server_tools))
        .route("/mcp/:server/call", post(call_server_tool))
        .route("/mcp/group/:group/tools", get(list_group_tools))
        .route("/mcp/group/:group/call", post(call_group_tool))
        .layer(CorsLayer::permissive())
}

// ────────────────────────────────────────────────────────────────────────────
// Lifecycle
// ────────────────────────────────────────────────────────────────────────────

/// Start the HTTP server on the given port.
/// If a server is already running on a different port, it will be stopped first.
pub async fn start(port: u16) -> anyhow::Result<()> {
    let mut guard = handle().lock().await;

    // Already running on the same port — nothing to do
    if let Some(ref h) = *guard {
        if h.port == port {
            log::info!("HTTP server already running on port {}", port);
            return Ok(());
        }
        // Different port — stop old instance (Sender drop triggers graceful shutdown)
        log::info!("HTTP server port changed, restarting...");
    }

    let app = build_router();

    let addr: SocketAddr = format!("0.0.0.0:{}", port).parse()?;
    let listener = TcpListener::bind(addr).await?;
    log::info!("MCPHub HTTP server listening on http://0.0.0.0:{}", port);

    let (abort_tx, abort_rx) = tokio::sync::oneshot::channel::<()>();

    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = abort_rx.await;
            })
            .await
            .ok();
        log::info!("MCPHub HTTP server stopped");
    });

    *guard = Some(ServerHandle { abort_tx, port });
    Ok(())
}

/// Stop the HTTP server if it is running.
pub async fn stop() {
    let mut guard = handle().lock().await;
    if let Some(h) = guard.take() {
        let _ = h.abort_tx.send(());
        log::info!("MCPHub HTTP server shutdown requested");
    }
}

/// Returns the current port if the server is running.
pub async fn current_port() -> Option<u16> {
    let guard = handle().lock().await;
    guard.as_ref().map(|h| h.port)
}

/// Called at startup — reads system_config and starts the server if exposeHttp is enabled.
pub async fn maybe_start() {
    match config_service::get().await {
        Ok(config) => {
            let expose = config
                .get("exposeHttp")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if expose {
                let port = config
                    .get("httpPort")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(3333) as u16;
                if let Err(e) = start(port).await {
                    log::error!("Failed to start HTTP server: {}", e);
                }
            }
        }
        Err(e) => log::warn!("Could not read config for HTTP server startup: {}", e),
    }
}

/// Sync HTTP server state with current config.
/// Called after update_system_config — starts if exposeHttp=true, stops if false.
pub async fn sync_with_config() {
    match config_service::get().await {
        Ok(config) => {
            let expose = config
                .get("exposeHttp")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if expose {
                let port = config
                    .get("httpPort")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(3333) as u16;
                if let Err(e) = start(port).await {
                    log::error!("Failed to start HTTP server: {}", e);
                }
            } else {
                stop().await;
            }
        }
        Err(e) => log::warn!("Could not read config for HTTP server sync: {}", e),
    }
}
