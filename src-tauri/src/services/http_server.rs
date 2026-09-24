/// http_server — embedded Axum HTTP server exposing MCP endpoints to external AI clients.
///
/// When `system_config.expose_http = true`, this service starts an HTTP server on the
/// configured port (default 23333). External clients can connect via SSE or Streamable HTTP
/// to access all connected MCP servers.
///
/// Endpoints:
///   GET  /health                    — health check
///   GET  /servers                   — list available servers
///   POST /mcp/{server}/call         — call a tool on a specific server
///   GET  /mcp/{server}/tools        — list tools for a server
///   POST /mcp/call                  — smart call: route to best server
use crate::{
    mcp::{pool, session_pool},
    models::{bearer_key::BearerKey, server::Tool},
    services::{
        app_logger, bearer_key_service, config_service, group_service, log_service,
        mcp_tasks, mcp_version::{self, MethodCtx, MethodOutcome, TransportMode, VersionStrategy},
        prompt_service, resource_service, server_tool_config_service,
    },
};
use axum::response::IntoResponse;
use axum::{
    body::{to_bytes, Body},
    extract::{Path, Query},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        Json, Response,
    },
    routing::{get, post},
    Router,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::{Arc, OnceLock},
};
use tokio::{net::TcpListener, sync::{mpsc, Mutex, RwLock}};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tower_http::cors::CorsLayer;
use tauri::Emitter;

fn new_session_id() -> String {
    let id: u128 = rand::random();
    format!("{:032x}", id)
}

// ────────────────────────────────────────────────────────────────────────────
// Per-session state
// ────────────────────────────────────────────────────────────────────────────
//
// Two session-keyed stores (both follow the OnceLock<Arc<RwLock<HashMap>>>
// pattern from mcp/session_pool.rs):
//
//  • SESSION_STRATEGY — the protocol-version strategy negotiated at
//    `initialize`, looked up by `mcp-session-id` on later requests so each
//    request is shaped by the revision the client speaks.
//
//  • SSE_CHANNELS — for legacy 2024-11-05 SSE-only clients, the sender half of
//    the channel the GET /mcp SSE stream drains. A POST /mcp/message pushes
//    the JSON-RPC response down this channel as an SSE `message` event
//    (instead of returning it as the POST body). Streamable-HTTP (2025+)
//    sessions never register a channel here.

type StrategyMap = HashMap<String, &'static dyn VersionStrategy>;

static SESSION_STRATEGY: OnceLock<Arc<RwLock<StrategyMap>>> = OnceLock::new();

fn session_strategies() -> &'static Arc<RwLock<StrategyMap>> {
    SESSION_STRATEGY.get_or_init(|| Arc::new(RwLock::new(HashMap::new())))
}

async fn strategy_for_session(sid: &str) -> Option<&'static dyn VersionStrategy> {
    session_strategies().read().await.get(sid).copied()
}

async fn remember_strategy(sid: String, strategy: &'static dyn VersionStrategy) {
    session_strategies().write().await.insert(sid, strategy);
}

async fn forget_strategy(sid: &str) {
    session_strategies().write().await.remove(sid);
}

type ChannelMap = HashMap<String, mpsc::UnboundedSender<String>>;

static SSE_CHANNELS: OnceLock<Arc<RwLock<ChannelMap>>> = OnceLock::new();

fn sse_channels() -> &'static Arc<RwLock<ChannelMap>> {
    SSE_CHANNELS.get_or_init(|| Arc::new(RwLock::new(HashMap::new())))
}

async fn register_sse_channel(sid: String, tx: mpsc::UnboundedSender<String>) {
    sse_channels().write().await.insert(sid, tx);
}

async fn sse_channel_for(sid: &str) -> Option<mpsc::UnboundedSender<String>> {
    sse_channels().read().await.get(sid).cloned()
}

/// Best-effort channel teardown. Called when a push fails (client gone) and on
/// session DELETE. ponytail: leaked entries are bounded by session count; a
/// periodic sweeper could be added if the map grows.
async fn drop_sse_channel(sid: &str) {
    sse_channels().write().await.remove(sid);
    forget_strategy(sid).await;
}

fn build_resource_metadata_url(headers: &HeaderMap) -> Option<String> {
    let host = headers.get("host").and_then(|v| v.to_str().ok())?;
    Some(format!("http://{}/.well-known/oauth-protected-resource", host))
}

fn build_oauth_401(headers: &HeaderMap, reason: &str) -> Response {
    let description = if reason == "missing" {
        "No authorization provided"
    } else {
        "Invalid bearer token"
    };
    let resource_metadata_url = build_resource_metadata_url(headers);
    let mut www_auth_parts = vec![
        "error=\"invalid_token\"".to_string(),
        format!("error_description=\"{}\"", description),
    ];
    let mut body = json!({
        "error": "invalid_token",
        "error_description": description,
    });
    if let Some(ref url) = resource_metadata_url {
        www_auth_parts.push(format!("resource_metadata=\"{}\"", url));
        body["resource_metadata"] = json!(url);
    }
    let www_auth = format!("Bearer {}", www_auth_parts.join(", "));
    let b = serde_json::to_string(&body).unwrap_or_default();
    axum::http::Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(header::CONTENT_TYPE, "application/json")
        .header("www-authenticate", www_auth)
        .body(Body::from(b))
        .unwrap()
}

// ────────────────────────────────────────────────────────────────────────────
// Global server state
// ────────────────────────────────────────────────────────────────────────────

struct ServerHandle {
    abort_tx: tokio::sync::oneshot::Sender<()>,
    /// Diagnostics companion of abort_tx (see start()): fired together in
    /// stop() so the serve task can classify its own ending.
    probe_tx: tokio::sync::oneshot::Sender<()>,
    port: u16,
    body_limit_bytes: usize,
}

/// Parse a body-limit string like "1mb", "512kb", "1048576" into bytes.
/// Defaults to 1 MiB when the input is empty or unrecognisable.
pub fn parse_body_limit(s: &str) -> usize {
    let s = s.trim().to_lowercase();
    if let Some(num) = s.strip_suffix("mb") {
        if let Ok(n) = num.trim().parse::<usize>() {
            return n * 1024 * 1024;
        }
    }
    if let Some(num) = s.strip_suffix("kb") {
        if let Ok(n) = num.trim().parse::<usize>() {
            return n * 1024;
        }
    }
    if let Some(num) = s.strip_suffix('b') {
        if let Ok(n) = num.trim().parse::<usize>() {
            return n;
        }
    }
    if let Ok(n) = s.parse::<usize>() {
        return n;
    }
    1024 * 1024 // default 1 MiB
}

static SERVER_HANDLE: OnceLock<Arc<Mutex<Option<ServerHandle>>>> = OnceLock::new();

fn handle() -> &'static Arc<Mutex<Option<ServerHandle>>> {
    SERVER_HANDLE.get_or_init(|| Arc::new(Mutex::new(None)))
}

// ── status reporting ─────────────────────────────────────────────────────────
// The last start/stop outcome, stashed process-globally. `maybe_start` runs at
// app startup (lib.rs) BEFORE the webview has registered its event listener, so
// a startup bind failure would otherwise be invisible to the UI. The frontend
// fetches `current_status` on mount via the `get_http_server_status` command to
// catch that missed failure; `set_status` also emits a live `http://server-status`
// event for updates that happen after the listener is up (e.g. the user changes
// the port in Settings → `sync_with_config` → `start`).

#[derive(Clone, serde::Serialize)]
pub struct HttpServerStatus {
    pub running: bool,
    pub port: u16,
    /// Human-readable failure reason (None when running or never started). On
    /// Windows the message is worded to flag the firewall as a likely cause.
    pub error: Option<String>,
    /// Machine-readable failure category ("addrInUse" | "permissionDenied" |
    /// "addrNotAvailable" | "other") so the frontend can localize the reason
    /// text by kind instead of showing this raw (English) message. None when
    /// no error.
    #[serde(rename = "errorKind", skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    /// Short OS error string (e.g. "Address already in use (os error 98)") for
    /// the dialog's technical detail line on the "other" kind. None when no error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

static HTTP_STATUS: OnceLock<std::sync::Mutex<HttpServerStatus>> = OnceLock::new();

fn status_lock() -> &'static std::sync::Mutex<HttpServerStatus> {
    HTTP_STATUS.get_or_init(|| std::sync::Mutex::new(HttpServerStatus {
        running: false,
        port: 0,
        error: None,
        error_kind: None,
        detail: None,
    }))
}

/// Snapshot of the last start/stop outcome (backs the `get_http_server_status`
/// command — lets the frontend catch a startup failure it missed).
pub fn current_status() -> HttpServerStatus {
    status_lock()
        .lock()
        .map(|g| g.clone())
        .unwrap_or(HttpServerStatus {
            running: false,
            port: 0,
            error: None,
            error_kind: None,
            detail: None,
        })
}

/// Record the latest outcome and, if an AppHandle is stashed (it is, after
/// `mcp::progress::set_app_handle` runs at startup), emit a `http://server-status`
/// event the frontend toasts on. Best-effort: a missing handle just means no
/// live toast (the status is still queryable via `current_status`).
fn set_status(s: HttpServerStatus) {
    if let Ok(mut g) = status_lock().lock() {
        *g = s.clone();
    }
    if let Some(app) = crate::mcp::progress::get_app_handle() {
        let _ = app.emit("http://server-status", &s);
    }
}

/// Machine-readable failure category for the status payload — the frontend
/// localizes the reason text from this instead of the raw English message.
fn bind_failure_kind(e: &std::io::Error) -> &'static str {
    match e.kind() {
        std::io::ErrorKind::AddrInUse => "addrInUse",
        std::io::ErrorKind::PermissionDenied => "permissionDenied",
        std::io::ErrorKind::AddrNotAvailable => "addrNotAvailable",
        _ => "other",
    }
}

/// Build a human-readable bind-failure message. On Windows the firewall is a
/// frequent cause (the app blocked from listening, or the port reserved by a
/// firewall rule), so it is called out alongside the usual "port occupied" so
/// the user knows what to fix. The message is both logged and surfaced to the
/// UI as the `error` field of the status event.
fn bind_failure_message(port: u16, e: &std::io::Error) -> String {
    let cause = match e.kind() {
        std::io::ErrorKind::AddrInUse => {
            "The port is already in use by another application.".to_string()
        }
        std::io::ErrorKind::PermissionDenied => {
            "Permission denied — the port may be blocked by a firewall rule or require elevation.".to_string()
        }
        std::io::ErrorKind::AddrNotAvailable => {
            "The address is not available on this host.".to_string()
        }
        _ => format!("OS error: {e}"),
    };
    if cfg!(windows) {
        format!(
            "Failed to start the MCP HTTP server on port {port}. {cause} \
             On Windows this is frequently caused by Windows Defender Firewall blocking the app. \
             Try allowing MCPHub through the firewall (and opening inbound TCP {port}), free the port, \
             or change the HTTP port in Settings."
        )
    } else {
        format!(
            "Failed to start the MCP HTTP server on port {port}. {cause} \
             If the port is occupied, free it or change the HTTP port in Settings."
        )
    }
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
    #[allow(dead_code)]
    server: Option<String>,
    #[allow(dead_code)]
    group: Option<String>,
    tool: String,
    arguments: Option<Value>,
}

// ────────────────────────────────────────────────────────────────────────────
// Auth helper
// ────────────────────────────────────────────────────────────────────────────

/// Validate the bearer token in the request headers.
/// Returns `Ok(None)` when bearer auth is disabled (all access allowed),
/// `Ok(Some(key))` when auth is enabled and the token is valid,
/// `Err(response)` when auth is enabled but the token is missing or invalid.
async fn check_bearer_auth(headers: &HeaderMap) -> Result<Option<BearerKey>, Response> {
    // Dynamically read config so changes take effect without restarting the HTTP server
    let config = config_service::get().await.ok();
    let enabled = config
        .as_ref()
        .and_then(|c| {
            // UI saves under routing.enableBearerAuth; legacy path: bearerKeyEnabled
            c.get("routing")
                .and_then(|r| r.get("enableBearerAuth"))
                .and_then(|v| v.as_bool())
                .or_else(|| c.get("bearerKeyEnabled").and_then(|v| v.as_bool()))
        })
        .unwrap_or(false);
    if !enabled {
        return Ok(None);
    }

    // Use the configured header name (defaults to "authorization")
    let header_name = config
        .as_ref()
        .and_then(|c| c.get("routing"))
        .and_then(|r| r.get("bearerAuthHeaderName"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_else(|| "authorization".to_string());

    let auth = headers
        .get(header_name.as_str())
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !auth.starts_with("Bearer ") {
        return Err(build_oauth_401(headers, "missing"));
    }
    let token = &auth[7..];
    match bearer_key_service::find_by_token(token).await {
        Ok(Some(key)) if key.enabled => Ok(Some(key)),
        _ => Err(build_oauth_401(headers, "invalid")),
    }
}

/// Extract server names from a list of JsonValue (can be strings or objects with "name" field)
fn extract_server_names(servers: &[serde_json::Value]) -> Vec<String> {
    servers
        .iter()
        .filter_map(|s| {
            if let Some(name) = s.as_str() {
                Some(name.to_string())
            } else if let Some(name) = s.get("name").and_then(|n| n.as_str()) {
                Some(name.to_string())
            } else {
                None
            }
        })
        .collect()
}

/// Compute the set of server names a bearer key is allowed to access.
/// Returns `None` when there is no restriction (access_type "all" or no key present).
async fn get_allowed_servers(key: Option<&BearerKey>) -> Option<HashSet<String>> {
    let key = key?;
    match key.access_type.as_str() {
        "all" => None,
        "servers" => Some(key.allowed_servers.iter().cloned().collect()),
        "groups" => {
            let mut servers = HashSet::new();
            if let Ok(groups) = group_service::list_all().await {
                for g in groups {
                    if key.allowed_groups.contains(&g.name) {
                        servers.extend(extract_server_names(&g.servers));
                    }
                }
            }
            Some(servers)
        }
        "custom" => {
            let mut servers: HashSet<String> = key.allowed_servers.iter().cloned().collect();
            if let Ok(groups) = group_service::list_all().await {
                for g in groups {
                    if key.allowed_groups.contains(&g.name) {
                        servers.extend(extract_server_names(&g.servers));
                    }
                }
            }
            Some(servers)
        }
        _ => None,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Handlers
// ────────────────────────────────────────────────────────────────────────────

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "service": "mcphub-desktop" }))
}

async fn list_servers(headers: HeaderMap) -> Response {
    let bearer_key = match check_bearer_auth(&headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let allowed_opt = get_allowed_servers(bearer_key.as_ref()).await;
    let statuses = pool::get_all_statuses().await;
    let servers: Vec<ServerInfo> = statuses
        .into_iter()
        .filter(|s| allowed_opt.as_ref().map_or(true, |a| a.contains(&s.name)))
        .map(|s| ServerInfo {
            name: s.name.clone(),
            connected: s.connected,
            tool_count: s.tool_count,
        })
        .collect();
    Json(json!({ "servers": servers })).into_response()
}

async fn list_server_tools(
    headers: HeaderMap,
    Path(server_name): Path<String>,
) -> Response {
    let bearer_key = match check_bearer_auth(&headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    if let Some(allowed) = get_allowed_servers(bearer_key.as_ref()).await {
        if !allowed.contains(&server_name) {
            return (StatusCode::FORBIDDEN, Json(json!({ "error": "Access denied for this server" }))).into_response();
        }
    }
    let tools = match pool::list_tools_for(&server_name).await {
        Ok(t) => t,
        Err(e) => return (StatusCode::NOT_FOUND, Json(json!({ "error": e.to_string() }))).into_response(),
    };
    let tools = server_tool_config_service::apply_tool_filters(&server_name, tools)
        .await
        .unwrap_or_else(|_| vec![]);
    Json(json!({ "tools": tools })).into_response()
}

async fn call_server_tool(
    headers: HeaderMap,
    Path(server_name): Path<String>,
    Json(req): Json<CallToolRequest>,
) -> Response {
    let bearer_key = match check_bearer_auth(&headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    if let Some(allowed) = get_allowed_servers(bearer_key.as_ref()).await {
        if !allowed.contains(&server_name) {
            return (StatusCode::FORBIDDEN, Json(json!({ "error": "Access denied for this server" }))).into_response();
        }
    }
    // Disabled-tool gate (origin #1178): tools disabled via server_tool_config
    // must not remain executable through the REST endpoint.
    if let Ok(tools) = pool::list_tools_for(&server_name).await {
        let filtered = server_tool_config_service::apply_tool_filters(&server_name, tools)
            .await
            .unwrap_or_default();
        if let Some(t) = filtered.iter().find(|t| &t.name == &req.tool) {
            if !t.enabled {
                return (
                    StatusCode::FORBIDDEN,
                    Json(json!({ "error": format!("Tool '{}' is disabled", req.tool) })),
                )
                    .into_response();
            }
        }
    }
    let args = req.arguments.unwrap_or(json!({}));
    match pool::call_tool(&server_name, &req.tool, args).await {
        Ok(result) => Json(json!({ "result": result.content, "is_error": result.is_error })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

async fn list_group_tools(
    headers: HeaderMap,
    Path(group_name): Path<String>,
) -> Response {
    let bearer_key = match check_bearer_auth(&headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let groups = match group_service::list_all().await {
        Ok(g) => g,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    };
    let group = match groups.into_iter().find(|g| g.name == group_name) {
        Some(g) => g,
        None => return (StatusCode::NOT_FOUND, Json(json!({ "error": "Group not found" }))).into_response(),
    };
    // Apply bearer key access control: filter to only servers the key can access
    let allowed_opt = get_allowed_servers(bearer_key.as_ref()).await;
    let server_names = extract_server_names(&group.servers);
    let accessible: Vec<&String> = server_names.iter()
        .filter(|s| allowed_opt.as_ref().map_or(true, |a| a.contains(*s)))
        .collect();
    if accessible.is_empty() && allowed_opt.is_some() {
        return (StatusCode::FORBIDDEN, Json(json!({ "error": "Access denied for this group" }))).into_response();
    }
    let mut tools: Vec<Tool> = Vec::new();
    for server_name in &accessible {
        if let Ok(server_tools) = pool::list_tools_for(server_name).await {
            let filtered = server_tool_config_service::apply_tool_filters(server_name, server_tools)
                .await
                .unwrap_or_else(|_| vec![]);
            tools.extend(filtered);
        }
    }
    Json(json!({ "group": group.name, "tools": tools })).into_response()
}

async fn call_group_tool(
    headers: HeaderMap,
    Path(group_name): Path<String>,
    Json(req): Json<SmartCallRequest>,
) -> Response {
    let bearer_key = match check_bearer_auth(&headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let groups = match group_service::list_all().await {
        Ok(g) => g,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    };
    let group = match groups.into_iter().find(|g| g.name == group_name) {
        Some(g) => g,
        None => return (StatusCode::NOT_FOUND, Json(json!({ "error": "Group not found" }))).into_response(),
    };
    let tool_name = &req.tool;
    // Apply bearer key access control: only search servers the key can access
    let allowed_opt = get_allowed_servers(bearer_key.as_ref()).await;
    let server_names = extract_server_names(&group.servers);
    let mut target_server: Option<String> = None;
    for server_name in &server_names {
        if allowed_opt.as_ref().map_or(true, |a| a.contains(server_name)) {
            if let Ok(tools) = pool::list_tools_for(server_name).await {
                if tools.iter().any(|t| &t.name == tool_name) {
                    target_server = Some(server_name.clone());
                    break;
                }
            }
        }
    }
    let server_name = match target_server {
        Some(s) => s,
        None => return (StatusCode::NOT_FOUND, Json(json!({ "error": format!("Tool '{}' not found in group '{}'", tool_name, group_name) }))).into_response(),
    };
    // Disabled-tool gate (origin #1178): tools disabled via server_tool_config
    // must not remain executable through the REST group endpoint.
    if let Ok(tools) = pool::list_tools_for(&server_name).await {
        let filtered = server_tool_config_service::apply_tool_filters(&server_name, tools)
            .await
            .unwrap_or_default();
        if let Some(t) = filtered.iter().find(|t| &t.name == tool_name) {
            if !t.enabled {
                return (
                    StatusCode::FORBIDDEN,
                    Json(json!({ "error": format!("Tool '{}' is disabled", tool_name) })),
                )
                    .into_response();
            }
        }
    }
    let args = req.arguments.unwrap_or(json!({}));
    match pool::call_tool(&server_name, tool_name, args).await {
        Ok(result) => Json(json!({ "result": result.content, "is_error": result.is_error })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// MCP Streamable HTTP Protocol (JSON-RPC 2.0)
// ────────────────────────────────────────────────────────────────────────────

fn jsonrpc_response(id: Option<Value>, result: Value) -> Response {
    let body = serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "result": result,
        "id": id,
    }))
    .unwrap_or_default();
    axum::http::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap()
}

fn jsonrpc_error(id: Option<Value>, code: i32, message: impl Into<String>) -> Response {
    let body = serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "error": {"code": code, "message": message.into()},
        "id": id,
    }))
    .unwrap_or_default();
    axum::http::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap()
}

/// Server config with optional tool/prompt/resource filters
struct ServerFilter {
    name: String,
    tools: Option<Vec<String>>,  // None = all tools, Some = specific tools
    #[allow(dead_code)]
    prompts: Option<Vec<String>>,
    #[allow(dead_code)]
    resources: Option<Vec<String>>,
}

/// Extract server filters from group servers config
fn extract_server_filters(servers: &[serde_json::Value]) -> Vec<ServerFilter> {
    servers
        .iter()
        .filter_map(|s| {
            let (name, tools, prompts, resources) = if let Some(name) = s.as_str() {
                (name.to_string(), None, None, None)
            } else if let Some(obj) = s.as_object() {
                let name = obj.get("name")?.as_str()?.to_string();
                let tools = extract_filter_list(obj.get("tools"));
                let prompts = extract_filter_list(obj.get("prompts"));
                let resources = extract_filter_list(obj.get("resources"));
                (name, tools, prompts, resources)
            } else {
                return None;
            };
            Some(ServerFilter { name, tools, prompts, resources })
        })
        .collect()
}

/// Extract filter list from a JSON value (can be "all" or array of strings)
fn extract_filter_list(value: Option<&serde_json::Value>) -> Option<Vec<String>> {
    match value {
        Some(serde_json::Value::String(s)) if s == "all" => None,  // None means all
        Some(serde_json::Value::Array(arr)) => {
            let names: Vec<String> = arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            if names.is_empty() { None } else { Some(names) }
        }
        _ => None,  // Default to all
    }
}

/// Resolve a scope path to the list of connected server names.
/// - "" or "$smart"              → all connected servers
/// - "$smart/{group}"            → servers in that group
/// - "{name}"                    → group by name/id, or single server
#[allow(dead_code)]
async fn mcp_scope_servers(scope: &str) -> Vec<String> {
    let scope = scope.trim_start_matches('/').trim();
    if scope.is_empty() || scope == "$smart" {
        return pool::get_all_statuses()
            .await
            .into_iter()
            .filter(|s| s.connected)
            .map(|s| s.name.clone())
            .collect();
    }
    let name = scope.strip_prefix("$smart/").unwrap_or(scope);
    // Try as group (name or id)
    if let Ok(groups) = group_service::list_all().await {
        if let Some(g) = groups.iter().find(|g| g.name == name || g.id == name) {
            return extract_server_names(&g.servers);
        }
    }
    // Try as server name
    if pool::get_all_statuses()
        .await
        .iter()
        .any(|s| s.connected && s.name == name)
    {
        return vec![name.to_string()];
    }
    vec![]
}

/// Get server filters for a scope (used for tool filtering in groups)
/// Look up the tools a server exposes, for `tools/list` + `tools/call`
/// resolution. Real (custom) servers come from the MCP pool; the RAG builtin
/// server has no pool entry, so its tools come from `rag::service` instead.
/// Returns None for a disconnected/unknown server (or RAG when disabled).
async fn tools_for_server(nm: &str) -> Option<Vec<crate::models::server::Tool>> {
    if nm == crate::rag::service::BUILTIN_SERVER_NAME {
        if crate::rag::service::is_enabled() {
            Some(crate::rag::service::builtin_tools())
        } else {
            None
        }
    } else {
        pool::list_tools_for(nm).await.ok()
    }
}

async fn mcp_scope_server_filters(scope: &str) -> Vec<ServerFilter> {
    let scope = scope.trim_start_matches('/').trim();
    // The RAG builtin server filter, appended to global scope (and exposed on
    // its own single-server scope) when RAG is enabled. Treated like any server
    // by the tools/list aggregation.
    let rag_filter = || -> Option<ServerFilter> {
        if crate::rag::service::is_enabled() {
            Some(ServerFilter {
                name: crate::rag::service::BUILTIN_SERVER_NAME.to_string(),
                tools: None,
                prompts: None,
                resources: None,
            })
        } else {
            None
        }
    };
    if scope.is_empty() || scope == "$smart" {
        // No filters for global scope - all connected pool servers + the RAG
        // builtin server (when RAG is on). On-demand stdio servers that are
        // currently sleeping (connected=false, start_on_demand=true) are
        // included so their cached tools stay discoverable and a `tools/call`
        // can cold-start them.
        let mut filters: Vec<ServerFilter> = pool::get_all_statuses()
            .await
            .into_iter()
            .filter(|s| s.connected || s.start_on_demand)
            .map(|s| ServerFilter {
                name: s.name.clone(),
                tools: None,
                prompts: None,
                resources: None,
            })
            .collect();
        if let Some(rf) = rag_filter() {
            filters.push(rf);
        }
        return filters;
    }
    let name = scope.strip_prefix("$smart/").unwrap_or(scope);
    // Try as group (name or id)
    if let Ok(groups) = group_service::list_all().await {
        if let Some(g) = groups.iter().find(|g| g.name == name || g.id == name) {
            return extract_server_filters(&g.servers);
        }
    }
    // RAG builtin server accessed directly as a single-server scope.
    if name == crate::rag::service::BUILTIN_SERVER_NAME {
        if let Some(rf) = rag_filter() {
            return vec![rf];
        }
    }
    // Try as server name (no filters). Include sleeping on-demand servers so a
    // single-server scope can still cold-start them via tools/call.
    if pool::get_all_statuses()
        .await
        .iter()
        .any(|s| (s.connected || s.start_on_demand) && s.name == name)
    {
        return vec![ServerFilter {
            name: name.to_string(),
            tools: None,
            prompts: None,
            resources: None,
        }];
    }
    vec![]
}

/// None = allow all (global/unknown scope); Some(allowed) = allow only if key is in the list.
fn builtin_allowed(selection: &Option<Vec<String>>, key: &str) -> bool {
    match selection {
        None => true,
        Some(allowed) => allowed.iter().any(|s| s == key),
    }
}

/// Core MCP JSON-RPC dispatcher.
async fn dispatch_mcp(headers: HeaderMap, scope: String, body: Value, fallback_ip: Option<String>) -> Response {
    let method = body.get("method").and_then(|m| m.as_str()).unwrap_or("unknown");
    let client_ip = headers.get("x-forwarded-for")
        .or_else(|| headers.get("x-real-ip"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or(s).trim().to_string())
        .or(fallback_ip)
        .unwrap_or_else(|| "127.0.0.1".to_string());
    // Downstream session id (set by our `initialize` response). Present for all
    // per-session requests on the HTTP MCP path; used to route `tools/call` to a
    // dedicated upstream client when the target server has `perSessionClient`.
    let session_id = headers
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    log::debug!("[HTTP] MCP request: method={}, scope={}, client={}, session={}", method, scope, client_ip, session_id.as_deref().unwrap_or("none"));

    let bearer_key = match check_bearer_auth(&headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };

    // Read config once for route-enable and nameSeparator checks
    let config = config_service::get().await.ok();

    // Check route-enable flags
    let scope_clean = scope.trim_start_matches('/').trim();
    let is_global_scope = scope_clean.is_empty() || scope_clean == "$smart";
    if is_global_scope {
        let enable_global = config.as_ref()
            .and_then(|c| c.get("routing"))
            .and_then(|r| r.get("enableGlobalRoute"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        if !enable_global {
            return (StatusCode::NOT_FOUND, Json(json!({"error": "Global route is disabled"}))).into_response();
        }
    } else {
        let check_name = scope_clean.strip_prefix("$smart/").unwrap_or(scope_clean);
        let is_group = group_service::list_all().await
            .map(|gs| gs.iter().any(|g| g.name == check_name || g.id == check_name))
            .unwrap_or(false);
        if is_group {
            let enable_group = config.as_ref()
                .and_then(|c| c.get("routing"))
                .and_then(|r| r.get("enableGroupNameRoute"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            if !enable_group {
                return (StatusCode::NOT_FOUND, Json(json!({"error": "Group name route is disabled"}))).into_response();
            }
        }
    }

    // Read nameSeparator from config (default "-")
    let name_sep: String = config.as_ref()
        .and_then(|c| c.get("nameSeparator"))
        .and_then(|v| v.as_str())
        .unwrap_or("-")
        .to_string();

    let method = body.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = body.get("id").cloned();
    let params = body.get("params").cloned().unwrap_or(Value::Null);

    // Resolve this session's negotiated protocol strategy (recorded at
    // initialize). For initialize itself there is no session header yet, so
    // this defaults to 2025-03-26 and is re-negotiated inside the arm. No
    // per-version if/else here — the strategy owns all version variance.
    let strategy = match session_id.as_deref() {
        Some(sid) => strategy_for_session(sid).await.unwrap_or_else(mcp_version::default_strategy),
        None => mcp_version::default_strategy(),
    };

    // 2025-06+ spec: subsequent requests SHOULD carry `MCP-Protocol-Version`
    // with the negotiated version. If the strategy requires it and the header
    // is present but names a version we don't support, respond 400 (spec). A
    // missing header is allowed for backward compat (spec says assume
    // 2025-03-26) so existing clients like Cherry Studio, which don't send
    // it, keep working.
    if strategy.requires_version_header() && method != "initialize" {
        if let Some(req_pv) = headers
            .get("mcp-protocol-version")
            .and_then(|v| v.to_str().ok())
        {
            if !req_pv.is_empty() && !mcp_version::is_supported(req_pv) {
                let msg = format!("Unsupported MCP-Protocol-Version: {}", req_pv);
                return (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response();
            }
        }
    }

    // Notifications have no "id" — respond with 202 Accepted, no body.
    if id.is_none() && (method.starts_with("notifications/") || method == "ping") {
        return axum::http::Response::builder()
            .status(StatusCode::ACCEPTED)
            .body(Body::empty())
            .unwrap();
    }

    match method {
        "initialize" => {
            // Protocol negotiation (spec): pick the strategy for the client's
            // requested version (falls back to 2025-03-26 when unknown), record
            // it for the session, and respond with that version + its
            // capabilities.
            let client_pv = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or("");
            let strategy = mcp_version::strategy_for(client_pv);
            // Reuse an existing session id (a legacy SSE GET established one,
            // surfaced here via the mcp-session-id header) or mint a fresh one
            // for Streamable HTTP.
            let sid = session_id.clone().unwrap_or_else(new_session_id);
            remember_strategy(sid.clone(), strategy).await;

            // Record the client connection in the log panel (app_log): the
            // downstream client's identity, requested vs negotiated protocol,
            // and transport — the first thing to look at when something breaks.
            let client_name = params
                .get("clientInfo").and_then(|c| c.get("name"))
                .and_then(|n| n.as_str()).unwrap_or("unknown");
            let client_ver = params
                .get("clientInfo").and_then(|c| c.get("version"))
                .and_then(|v| v.as_str()).unwrap_or("unknown");
            // Transport reflects how the client connects at the MCP protocol
            // layer — not which radio button the user picked in their client
            // UI (Cherry Studio's "SSE" and "Streamable HTTP" send identical
            // POST requests with `Accept: application/json, text/event-stream`,
            // so the server cannot distinguish them and does not try).
            //   • POST /mcp (any Accept)  → streamable-http (2025 Streamable HTTP)
            //   • GET /mcp with stream accept and no session → legacy-sse
            //     (genuinely 2024-11-05 SSE-only client; see mcp_root_get)
            let transport = match strategy.transport() {
                TransportMode::LegacySse => "legacy-sse",
                TransportMode::StreamableHttp => "streamable-http",
            };
            let accept_hdr = headers
                .get("accept").and_then(|v| v.to_str().ok()).unwrap_or("(none)");
            let conn_msg = format!(
                "[MCP] Client connected: session={} ip={} client={}/{} proto={}→{} transport={} accept={}",
                sid, client_ip, client_name, client_ver,
                if client_pv.is_empty() { "(none)" } else { client_pv },
                strategy.version(), transport, accept_hdr
            );
            log::info!("{}", conn_msg);
            app_logger::log_to_db("info", &conn_msg);

            let mut resp = jsonrpc_response(
                id,
                json!({
                    "protocolVersion": strategy.version(),
                    "capabilities": strategy.capabilities(),
                    "serverInfo": {"name": "MCPHub Desktop", "version": env!("CARGO_PKG_VERSION")}
                }),
            );
            resp.headers_mut().insert(
                "mcp-session-id",
                sid.parse().expect("valid header value"),
            );
            resp
        }
        "ping" => jsonrpc_response(id, json!({})),
        "tools/list" => {
            let mut server_filters = mcp_scope_server_filters(&scope).await;
            // Apply bearer key access control
            if let Some(allowed) = get_allowed_servers(bearer_key.as_ref()).await {
                server_filters.retain(|s| allowed.contains(&s.name));
            }
            // Prefix tool names with server name when multiple servers are in scope
            let use_prefix = server_filters.len() > 1;
            let mut tools: Vec<Value> = Vec::new();
            for sf in &server_filters {
                // Builtin RAG server: its tools come from rag::service (no pool
                // entry). Otherwise pull from the MCP pool as usual.
                let is_builtin = sf.name == crate::rag::service::BUILTIN_SERVER_NAME;
                let ts: Vec<crate::models::server::Tool> = if is_builtin {
                    if !crate::rag::service::is_enabled() {
                        continue;
                    }
                    crate::rag::service::builtin_tools()
                } else {
                    match pool::list_tools_for(&sf.name).await {
                        Ok(ts) => ts,
                        Err(_) => continue,
                    }
                };
                let filtered = if is_builtin {
                    ts
                } else {
                    server_tool_config_service::apply_tool_filters(&sf.name, ts)
                        .await
                        .unwrap_or_else(|_| vec![])
                };
                for t in &filtered {
                    // Skip disabled tools
                    if !t.enabled {
                        continue;
                    }
                    // Apply group-level tool filter
                    if let Some(ref allowed_tools) = sf.tools {
                        if !allowed_tools.contains(&t.name) {
                            continue;
                        }
                    }
                    let exposed_name = if use_prefix {
                        format!("{}{}{}", sf.name, name_sep, t.name)
                    } else {
                        t.name.clone()
                    };
                    let mut entry = json!({
                        "name": exposed_name,
                        "description": t.description.as_deref().unwrap_or(""),
                        "inputSchema": t.input_schema,
                    });
                    // 2025 passthrough: forward upstream annotations /
                    // outputSchema only when present. The strategy then
                    // shapes the entry (e.g. 2024 strips these).
                    if let Some(a) = &t.annotations {
                        entry["annotations"] = a.clone();
                    }
                    if let Some(s) = &t.output_schema {
                        entry["outputSchema"] = s.clone();
                    }
                    tools.push(strategy.shape_tool(entry));
                }
            }
            jsonrpc_response(id, json!({"tools": tools}))
        }
        "tools/call" => {
            let tool_name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));

            let mut server_filters = mcp_scope_server_filters(&scope).await;
            // Apply bearer key access control
            if let Some(allowed) = get_allowed_servers(bearer_key.as_ref()).await {
                server_filters.retain(|s| allowed.contains(&s.name));
            }
            let use_prefix = server_filters.len() > 1;

            // Resolve server + original tool name (strip nameSeparator prefix if needed).
            // The RAG builtin server has no pool entry - its tools come from
            // rag::service::builtin_tools(), so we branch on the builtin name.
            let rag_name = crate::rag::service::BUILTIN_SERVER_NAME;
            let mut target: Option<(String, String)> = None;
            if use_prefix {
                // Try to find a server whose prefix matches the tool_name
                for sf in &server_filters {
                    let prefix = format!("{}{}", sf.name, name_sep);
                    if tool_name.starts_with(&prefix) {
                        let orig_name = &tool_name[prefix.len()..];
                        // Check if tool is allowed in group config
                        if let Some(ref allowed_tools) = sf.tools {
                            if !allowed_tools.contains(&orig_name.to_string()) {
                                continue;
                            }
                        }
                        if let Some(ts) = tools_for_server(&sf.name).await {
                            if ts.iter().any(|t| t.name == orig_name) {
                                target = Some((sf.name.clone(), orig_name.to_string()));
                                break;
                            }
                        }
                    }
                }
            }
            // Fallback: search by original name (single-server scope or unprefixed call)
            if target.is_none() {
                for sf in &server_filters {
                    // Check if tool is allowed in group config
                    if let Some(ref allowed_tools) = sf.tools {
                        if !allowed_tools.contains(&tool_name.to_string()) {
                            continue;
                        }
                    }
                    if let Some(ts) = tools_for_server(&sf.name).await {
                        if ts.iter().any(|t| t.name == tool_name) {
                            target = Some((sf.name.clone(), tool_name.to_string()));
                            break;
                        }
                    }
                }
            }
            match target {
                None => {
                    log::warn!("[HTTP] Tool '{}' not found in scope '{}'", tool_name, scope);
                    jsonrpc_error(id, -32602, format!("Tool '{}' not found", tool_name))
                }
                Some((sn, orig_name)) => {
                    // RAG builtin server: dispatch to the local rag service
                    // (no MCP pool call). Only the three RAG tools are routed.
                    if sn == rag_name {
                        // Builtin RAG server: dispatch via the shared
                        // call_builtin_tool (same path the Tauri call_tool
                        // command uses). Returns a ToolCallResult we wrap into a
                        // JSON-RPC response. call_builtin_tool guards is_enabled
                        // and "tool not found" itself.
                        let Some(app) = crate::mcp::progress::get_app_handle() else {
                            return jsonrpc_error(id, -32603, "app handle unavailable".to_string());
                        };
                        match crate::rag::service::call_builtin_tool(&app, &orig_name, &args).await {
                            Ok(result) => {
                                let resp = json!({"content": result.content, "isError": result.is_error});
                                return jsonrpc_response(id, strategy.shape_tool_call_result(resp));
                            }
                            Err(e) => return jsonrpc_error(id, -32603, e.to_string()),
                        }
                    }
                    // Check if tool is enabled
                    if let Ok(ts) = pool::list_tools_for(&sn).await {
                        let filtered = server_tool_config_service::apply_tool_filters(&sn, ts).await.unwrap_or_default();
                        if let Some(t) = filtered.iter().find(|t| t.name == orig_name) {
                            if !t.enabled {
                                log::warn!("[HTTP] Tool '{}' is disabled on server '{}'", orig_name, sn);
                                return jsonrpc_error(id, -32602, format!("Tool '{}' is disabled", orig_name));
                            }
                        }
                    }

                    log::debug!("[HTTP] Calling tool '{}' on server '{}'", orig_name, sn);
                    let start = std::time::Instant::now();
                    // Per-session upstream client isolation (origin #985): when
                    // the target server has `perSessionClient` and a downstream
                    // session id is present, route the call through a dedicated
                    // per-session client instead of the shared pool. Otherwise
                    // (no session, or shared server) use the shared pool as before.
                    let is_isolated = session_id.as_ref().is_some()
                        && pool::is_per_session_client(&sn).await;

                    // 2025-11-25 task augmentation: when the client sends a
                    // `task` field in params, wrap the call as an async task
                    // and return a CreateTaskResult immediately (status:
                    // working). The client polls via tasks/get|result. The
                    // negotiated strategy must have advertised the tasks
                    // capability (V2025_11_25 does); other versions ignore it.
                    if params.get("task").is_some() && strategy.requires_version_header() {
                        let ttl = params.get("task")
                            .and_then(|t| t.get("ttl"))
                            .and_then(|v| v.as_u64());
                        let task = mcp_tasks::create_tool_task(
                            sn.clone(), orig_name.clone(), args.clone(),
                            session_id.clone(), is_isolated, client_ip.clone(),
                            strategy, ttl,
                        ).await;
                        log::info!("[HTTP] Task-augmented tools/call: tool '{}' on '{}' → taskId {}",
                            orig_name, sn, task["taskId"]);
                        app_logger::log_to_db("info", &format!(
                            "[HTTP] Task created: tool '{}' on server '{}' task={}",
                            orig_name, sn, task["taskId"]
                        ));
                        return jsonrpc_response(id, json!({"task": task}));
                    }

                    let call_result = if is_isolated {
                        let sid = session_id.as_ref().unwrap();
                        log::info!(
                            "[HTTP] Routing tool '{}' on server '{}' through isolated session client ({})",
                            orig_name, sn, sid
                        );
                        app_logger::log_to_db(
                            "info",
                            &format!(
                                "[HTTP] Per-session isolated call: tool '{}' on server '{}' (session {})",
                                orig_name, sn, sid
                            ),
                        );
                        session_pool::call_tool_isolated(sid, &sn, &orig_name, args.clone()).await
                    } else {
                        pool::call_tool(&sn, &orig_name, args.clone()).await
                    };
                    match call_result {
                        Ok(r) => {
                            let duration_ms = start.elapsed().as_millis() as i64;
                            let status = if r.is_error { "error" } else { "success" };
                            log::info!("[HTTP] Tool '{}' call {} on server '{}' ({}ms)", orig_name, status, sn, duration_ms);

                            // Write to activity_log
                            let output = serde_json::to_value(&r).ok();
                            let _ = log_service::write_activity(
                                &sn,
                                &orig_name,
                                Some(duration_ms),
                                status,
                                Some(args),
                                output,
                                None,
                                Some(&client_ip),
                            ).await;

                            // 2025 passthrough: forward upstream
                            // structuredContent only when present; the
                            // strategy then shapes the result (e.g. 2024 strips it).
                            let mut call_resp = json!({"content": r.content, "isError": r.is_error});
                            if let Some(sc) = &r.structured_content {
                                call_resp["structuredContent"] = sc.clone();
                            }
                            jsonrpc_response(id, strategy.shape_tool_call_result(call_resp))
                        }
                        Err(e) => {
                            let duration_ms = start.elapsed().as_millis() as i64;
                            let err_msg = e.to_string();
                            log::error!("[HTTP] Tool '{}' call failed on server '{}' ({}ms): {}", orig_name, sn, duration_ms, err_msg);

                            // Write to activity_log
                            let _ = log_service::write_activity(
                                &sn,
                                &orig_name,
                                Some(duration_ms),
                                "error",
                                Some(args),
                                None,
                                Some(&err_msg),
                                Some(&client_ip),
                            ).await;

                            jsonrpc_error(id, -32603, err_msg)
                        }
                    }
                }
            }
        }
        "prompts/list" => {
            // Builtin prompts are carried by the "mcphub-desktop" builtin server.
            // Its per-server `prompts` selection (None = all, Some = list) in the
            // scope's group config governs which are exposed. If the builtin
            // server isn't in scope, expose none.
            let filters = mcp_scope_server_filters(&scope).await;
            let prompt_sel = filters
                .iter()
                .find(|f| f.name == crate::rag::service::BUILTIN_SERVER_NAME)
                .and_then(|f| f.prompts.clone());
            let prompts = prompt_service::list_all().await.unwrap_or_default();
            let list: Vec<Value> = prompts.into_iter().filter(|p| {
                p.enabled && builtin_allowed(&prompt_sel, &p.name)
            }).map(|p| {
                let args: Vec<Value> = p.arguments.into_iter().map(|a| json!({
                    "name": a.name,
                    "description": a.description.unwrap_or_default(),
                    "required": a.required,
                })).collect();
                json!({
                    "name": p.name,
                    "title": p.title,
                    "description": p.description.unwrap_or_default(),
                    "arguments": args,
                })
            }).collect();
            jsonrpc_response(id, json!({"prompts": list}))
        }
        "prompts/get" => {
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let filters = mcp_scope_server_filters(&scope).await;
            let prompt_sel = filters
                .iter()
                .find(|f| f.name == crate::rag::service::BUILTIN_SERVER_NAME)
                .and_then(|f| f.prompts.clone());
            let prompt = match prompt_service::list_all().await {
                Ok(ps) => ps.into_iter().find(|p| p.enabled && p.name == name
                    && builtin_allowed(&prompt_sel, &p.name)),
                Err(_) => None,
            };
            match prompt {
                Some(p) => {
                    let text = prompt_service::render_template(&p.template, &args);
                    jsonrpc_response(id, json!({
                        "title": p.title,
                        "description": p.description.unwrap_or_default(),
                        "messages": [{
                            "role": "user",
                            "content": {"type": "text", "text": text}
                        }]
                    }))
                }
                None => jsonrpc_error(id, -32602, format!("Prompt '{}' not found", name)),
            }
        }
        "resources/list" => {
            let filters = mcp_scope_server_filters(&scope).await;
            let resource_sel = filters
                .iter()
                .find(|f| f.name == crate::rag::service::BUILTIN_SERVER_NAME)
                .and_then(|f| f.resources.clone());
            let resources = resource_service::list_all().await.unwrap_or_default();
            let list: Vec<Value> = resources.into_iter().filter(|r| {
                r.enabled && builtin_allowed(&resource_sel, &r.uri)
            }).map(|r| json!({
                "uri": r.uri,
                "name": r.name.unwrap_or_default(),
                "description": r.description.unwrap_or_default(),
                "mimeType": r.mime_type,
            })).collect();
            jsonrpc_response(id, json!({"resources": list}))
        }
        "resources/read" => {
            let uri = params.get("uri").and_then(|u| u.as_str()).unwrap_or("");
            let filters = mcp_scope_server_filters(&scope).await;
            let resource_sel = filters
                .iter()
                .find(|f| f.name == crate::rag::service::BUILTIN_SERVER_NAME)
                .and_then(|f| f.resources.clone());
            let resource = match resource_service::list_all().await {
                Ok(rs) => rs.into_iter().find(|r| r.enabled && r.uri == uri
                    && builtin_allowed(&resource_sel, &r.uri)),
                Err(_) => None,
            };
            match resource {
                Some(r) => jsonrpc_response(id, json!({
                    "contents": [{
                        "uri": r.uri,
                        "mimeType": r.mime_type,
                        "text": r.content,
                    }]
                })),
                None => jsonrpc_error(id, -32602, format!("Resource '{}' not found", uri)),
            }
        }
        "tasks/get" => {
            // 2025-11-25: return the current Task snapshot (client polls).
            let task_id = params.get("taskId").and_then(|t| t.as_str()).unwrap_or("");
            match mcp_tasks::get(task_id).await {
                Some(t) => jsonrpc_response(id, json!({"task": t})),
                None => jsonrpc_error(id, -32602, format!("Task '{}' not found", task_id)),
            }
        }
        "tasks/result" => {
            // 2025-11-25: the stored CallToolResult (with _meta.related-task)
            // when terminal, else the task snapshot for continued polling.
            let task_id = params.get("taskId").and_then(|t| t.as_str()).unwrap_or("");
            match mcp_tasks::result(task_id).await {
                Ok(r) => jsonrpc_response(id, r),
                Err((code, msg)) => jsonrpc_error(id, code, msg),
            }
        }
        "tasks/list" => {
            // 2025-11-25: list all tasks (no pagination; desktop-scale).
            let list = mcp_tasks::list_all().await;
            jsonrpc_response(id, list)
        }
        "tasks/cancel" => {
            // 2025-11-25: mark the task cancelled (terminal).
            let task_id = params.get("taskId").and_then(|t| t.as_str()).unwrap_or("");
            match mcp_tasks::cancel(task_id).await {
                Ok(t) => jsonrpc_response(id, json!({"task": t})),
                Err((code, msg)) => jsonrpc_error(id, code, msg),
            }
        }
        _ => {
            // Hand version-specific methods (e.g. 2025-11 tasks/*) to the
            // strategy; fall through to -32601 when it does not claim it.
            let ctx = MethodCtx {
                scope: scope.clone(),
                session_id: session_id.clone(),
                params: params.clone(),
                name_sep: name_sep.clone(),
                client_ip: client_ip.clone(),
            };
            match strategy.handle_extra_method(method, &ctx) {
                Some(MethodOutcome::Result(v)) => jsonrpc_response(id, v),
                Some(MethodOutcome::Error(code, msg)) => jsonrpc_error(id, code, msg),
                None => jsonrpc_error(id, -32601, "Method not found"),
            }
        }
    }
}

async fn mcp_root_post(headers: HeaderMap, Json(body): Json<Value>) -> Response {
    let socket_ip = headers.get("host")
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.split(':').next())
        .map(|s| s.to_string());
    log_mcp_request("POST", "/mcp", &headers);
    dispatch_mcp(headers, String::new(), body, socket_ip).await
}

async fn mcp_scope_post(
    headers: HeaderMap,
    Path(path): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let socket_ip = headers.get("host")
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.split(':').next())
        .map(|s| s.to_string());
    log_mcp_request("POST", &format!("/mcp/{}", path), &headers);
    dispatch_mcp(headers, path, body, socket_ip).await
}

/// Query params for the legacy 2024-11-05 SSE message endpoint.
/// The session id is in the `endpoint` event URI the server sent on the GET
/// SSE stream (2024 transport has no mcp-session-id header).
#[derive(Deserialize)]
struct MessageQuery {
    #[serde(rename = "sessionId", default)]
    session_id: Option<String>,
}

/// Debug helper: log every inbound MCP HTTP request (method, path, accept,
/// session) to the log panel so a client's real transport can be identified
/// without guessing from the initialize message alone.
fn log_mcp_request(method: &str, path: &str, headers: &HeaderMap) {
    let accept = headers
        .get("accept").and_then(|v| v.to_str().ok()).unwrap_or("(none)");
    let sid = headers
        .get("mcp-session-id").and_then(|v| v.to_str().ok()).unwrap_or("(none)");
    let msg = format!(
        "[MCP] Request: {} {} accept={} session={}", method, path, accept, sid
    );
    log::info!("{}", msg);
    app_logger::log_to_db("debug", &msg);
}

/// Legacy 2024-11-05 SSE transport: POST endpoint the client sends requests
/// to. The JSON-RPC response does NOT come back on this POST — it is pushed
/// down the session's SSE stream as a `message` event. The POST itself
/// returns 202 Accepted.
async fn mcp_message_post(
    headers: HeaderMap,
    Query(q): Query<MessageQuery>,
    Json(body): Json<Value>,
) -> Response {
    let socket_ip = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.split(':').next())
        .map(|s| s.to_string());
    // Surface the query session id as the mcp-session-id header so
    // dispatch_mcp resolves this legacy session's negotiated strategy
    // (V2024) and per-session routing works.
    let mut headers = headers;
    if let Some(ref sid) = q.session_id {
        if let Ok(v) = HeaderValue::from_str(sid) {
            headers.insert("mcp-session-id", v);
        }
    }
    let resp = dispatch_mcp(headers, String::new(), body, socket_ip).await;
    // Collect the JSON-RPC body dispatch produced and push it onto the SSE
    // channel; empty body (notifications → 202) is skipped.
    let bytes = match to_bytes(resp.into_body(), 16 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::ACCEPTED, Body::empty()).into_response(),
    };
    if let Some(ref sid) = q.session_id {
        if !bytes.is_empty() {
            let msg = String::from_utf8_lossy(&bytes).to_string();
            if let Some(tx) = sse_channel_for(sid).await {
                if tx.send(msg).is_err() {
                    // SSE stream already gone — drop this session's channel.
                    drop_sse_channel(sid).await;
                }
            }
        }
    }
    (StatusCode::ACCEPTED, Body::empty()).into_response()
}

async fn mcp_root_get(headers: HeaderMap) -> Response {
    log_mcp_request("GET", "/mcp", &headers);
    if let Err(r) = check_bearer_auth(&headers).await {
        return r;
    }
    // If the client doesn't request SSE (e.g. browser), return a friendly JSON info response
    let accept = headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !accept.contains("text/event-stream") {
        return Json(json!({
            "service": "MCPHub Desktop",
            "version": env!("CARGO_PKG_VERSION"),
            "transport": "MCP Streamable HTTP",
            "usage": {
                "initialize": "POST /mcp  body: {\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},\"clientInfo\":{\"name\":\"client\",\"version\":\"1.0\"}}}",
                "tools_list": "POST /mcp  body: {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}",
                "sse_stream": "GET /mcp  Accept: text/event-stream"
            }
        })).into_response();
    }

    // An existing session (mcp-session-id header) doing a GET means a 2025
    // Streamable HTTP client opening the server-push stream — keep-alive only.
    if extract_session_id(&headers).is_some() {
        let stream = tokio_stream::wrappers::IntervalStream::new(
            tokio::time::interval(std::time::Duration::from_secs(25)),
        )
        .map(|_| Ok::<Event, std::convert::Infallible>(Event::default().comment("keep-alive")));
        return Sse::new(stream)
            .keep_alive(KeepAlive::default())
            .into_response();
    }

    // No session header + SSE accept → legacy 2024-11-05 HTTP+SSE transport.
    // Establish a session bound to the V2024 strategy + an SSE channel, send
    // the `endpoint` event telling the client where to POST, then stream
    // `message` events carrying the JSON-RPC responses that /mcp/message
    // pushes for this session.
    let sid = new_session_id();
    let strategy = mcp_version::strategy_for("2024-11-05");
    remember_strategy(sid.clone(), strategy).await;
    let (tx, rx) = mpsc::unbounded_channel::<String>();
    register_sse_channel(sid.clone(), tx).await;

    // Record the legacy SSE connection in the log panel. Only a genuine
    // 2024-11-05 SSE-only client reaches here (no session header, stream
    // accept on GET). Note: clients that ALSO POST initialize (like Cherry
    // Studio) will produce a second "Client connected" line via the POST path
    // — that's expected; this line reflects the legacy GET stream itself.
    let conn_msg = format!(
        "[MCP] Client connected: session={} ip={} client=(legacy-sse-stream) proto=2024-11-05→{} transport=legacy-sse",
        sid,
        headers.get("x-forwarded-for").or_else(|| headers.get("x-real-ip"))
            .and_then(|v| v.to_str().ok()).and_then(|s| Some(s.split(',').next().unwrap_or(s).trim().to_string()))
            .unwrap_or_else(|| "127.0.0.1".to_string()),
        strategy.version()
    );
    log::info!("{}", conn_msg);
    app_logger::log_to_db("info", &conn_msg);

    let endpoint_uri = format!("/mcp/message?sessionId={}", sid);
    let endpoint_ev: Result<Event, std::convert::Infallible> =
        Ok(Event::default().event("endpoint").data(endpoint_uri));
    // Diagnostics: log when the legacy SSE stream ENDS (client gone / channel
    // dropped) - previously there was no trace of a session vanishing.
    let watch_sid = sid.clone();
    let message_stream = UnboundedReceiverStream::new(rx)
        .map(|s| Ok::<Event, std::convert::Infallible>(Event::default().event("message").data(s)))
        .chain(futures_util::stream::unfold((), move |()| {
            let watch_sid = watch_sid.clone();
            async move {
                let line = format!(
                    "[http-server-watch] legacy SSE stream ended: session={} (client disconnected or channel dropped)",
                    watch_sid
                );
                log::info!("{}", line);
                app_logger::log_to_db("info", &line);
                None
            }
        }));
    let stream = futures_util::stream::iter([endpoint_ev]).chain(message_stream);
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn mcp_scope_get(headers: HeaderMap, Path(_): Path<String>) -> Response {
    mcp_root_get(headers).await
}

async fn mcp_root_delete(headers: HeaderMap) -> StatusCode {
    if let Some(sid) = extract_session_id(&headers) {
        log::info!("[HTTP] DELETE /mcp — cleaning up session {}", sid);
        app_logger::log_to_db(
            "info",
            &format!("[HTTP] Session end (DELETE /mcp), cleaning up isolated clients: {}", sid),
        );
        // Tear down any per-session isolated upstream clients for this session.
        session_pool::cleanup_session(&sid).await;
        // Drop this session's SSE channel (legacy transport) + negotiated strategy.
        drop_sse_channel(&sid).await;
    } else {
        log::debug!("[HTTP] DELETE /mcp — no mcp-session-id header, nothing to clean");
    }
    StatusCode::OK
}

async fn mcp_scope_delete(headers: HeaderMap, Path(path): Path<String>) -> StatusCode {
    if let Some(sid) = extract_session_id(&headers) {
        log::info!("[HTTP] DELETE /mcp/{} — cleaning up session {}", path, sid);
        app_logger::log_to_db(
            "info",
            &format!(
                "[HTTP] Session end (DELETE /mcp/{}), cleaning up isolated clients: {}",
                path, sid
            ),
        );
        session_pool::cleanup_session(&sid).await;
        drop_sse_channel(&sid).await;
    } else {
        log::debug!("[HTTP] DELETE /mcp/{} — no mcp-session-id header, nothing to clean", path);
    }
    StatusCode::OK
}

/// Extract the `mcp-session-id` header (trimmed, non-empty) if present.
fn extract_session_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

async fn oauth_protected_resource(headers: HeaderMap) -> Response {
    let base_url = build_resource_metadata_url(&headers)
        .map(|url| url.replace("/.well-known/oauth-protected-resource", ""))
        .unwrap_or_else(|| "http://localhost:23333".to_string());
    Json(json!({
        "resource": base_url,
        "authorization_servers": [base_url],
        "scopes_supported": ["read", "write"],
        "bearer_methods_supported": ["header"],
    })).into_response()
}

// ────────────────────────────────────────────────────────────────────────────
// OpenAPI-compatible endpoints (origin /api/*: spec generation + tool execution)
//
// Mirrors origin's openApiController/openApiGeneratorService so OpenWebUI and
// other OpenAPI clients can browse/call MCP tools:
//   GET  /api/openapi.json|.yaml                  — spec for all connected servers
//   GET  /api/{name}/openapi.json|.yaml           — spec scoped to a group or single server
//   GET|POST /api/tools/{server}/{tool}           — execute a tool (global scope)
//   GET|POST /api/{name}/tools/{server}/{tool}    — execute a tool (group/server scope)
// All routes go through the same bearer-key gate as /rest/* and /mcp/*.
// ────────────────────────────────────────────────────────────────────────────

/// Read nameSeparator from system config (default "-"); same source as the
/// /mcp dispatch path.
async fn name_separator() -> String {
    config_service::get()
        .await
        .ok()
        .and_then(|c| c.get("nameSeparator").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .unwrap_or_else(|| "-".to_string())
}

/// One tool collected for spec generation / execution: (server name, bare tool
/// name with the server prefix stripped, full Tool with enabled/description
/// overrides applied).
struct OpenApiToolRef {
    server: String,
    bare_name: String,
    tool: Tool,
}

/// Collect the tools visible under `scope` (None = global "/"), mirroring the
/// /mcp scope rules: group tool allow-lists applied, disabled tools skipped
/// (unless `include_disabled`), server-prefixed runtime names reduced to bare.
async fn collect_openapi_tools(scope: Option<&str>, include_disabled: bool) -> Vec<OpenApiToolRef> {
    let sfs = mcp_scope_server_filters(scope.unwrap_or("/")).await;
    let name_sep = name_separator().await;
    let mut out = Vec::new();
    for sf in sfs {
        let ts = match tools_for_server(&sf.name).await {
            Some(t) => t,
            None => continue,
        };
        // Group allow-list: sf.tools holds bare tool names (same as tools/call).
        let ts: Vec<Tool> = match &sf.tools {
            Some(allowed) => ts
                .into_iter()
                .filter(|t| {
                    let bare = t
                        .name
                        .strip_prefix(&format!("{}{}", sf.name, name_sep))
                        .unwrap_or(&t.name);
                    allowed.contains(&bare.to_string())
                })
                .collect(),
            None => ts,
        };
        let ts = server_tool_config_service::apply_tool_filters(&sf.name, ts)
            .await
            .unwrap_or_default();
        for t in ts {
            if !include_disabled && !t.enabled {
                continue;
            }
            let prefix = format!("{}{}", sf.name, name_sep);
            let bare_name = t.name.strip_prefix(&prefix).unwrap_or(&t.name).to_string();
            out.push(OpenApiToolRef {
                server: sf.name.clone(),
                bare_name,
                tool: t,
            });
        }
    }
    out
}

/// Decide the operation shape from a tool's inputSchema, mirroring origin:
/// pure primitive params (no object/array/string) and ≤10 properties → GET
/// query parameters; anything else → POST JSON request body.
fn tool_schema_shape(tool: &Tool) -> (Option<Vec<serde_json::Value>>, Option<serde_json::Value>) {
    let schema = &tool.input_schema;
    if !schema.is_object() {
        return (None, None);
    }
    let (Some(properties), Some(props_obj)) = (
        schema.get("properties"),
        schema.get("properties").and_then(|p| p.as_object()),
    ) else {
        return (None, None);
    };
    if props_obj.is_empty() {
        return (None, None);
    }
    let required: Vec<&str> = schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    let has_complex = props_obj.values().any(|prop: &serde_json::Value| {
        matches!(
            prop.get("type").and_then(|t| t.as_str()),
            Some("object") | Some("array") | Some("string")
        )
    });
    if !has_complex && props_obj.len() <= 10 {
        let mut parameters: Vec<serde_json::Value> = Vec::new();
        for (name, prop) in props_obj.iter() {
            let mut schema_obj = serde_json::Map::new();
            schema_obj.insert(
                "type".to_string(),
                prop.get("type").cloned().unwrap_or(json!("string")),
            );
            if let Some(e) = prop.get("enum") {
                schema_obj.insert("enum".to_string(), e.clone());
            }
            if let Some(d) = prop.get("default") {
                schema_obj.insert("default".to_string(), d.clone());
            }
            if let Some(f) = prop.get("format") {
                schema_obj.insert("format".to_string(), f.clone());
            }
            parameters.push(json!({
                "name": name,
                "in": "query",
                "required": required.contains(&name.as_str()),
                "description": prop.get("description").cloned().unwrap_or(json!(format!("Parameter {name}"))),
                "schema": serde_json::Value::Object(schema_obj),
            }));
        }
        (Some(parameters), None)
    } else {
        let mut body_schema = serde_json::Map::new();
        body_schema.insert("type".to_string(), json!("object"));
        body_schema.insert("properties".to_string(), properties.clone());
        if !required.is_empty() {
            body_schema.insert("required".to_string(), json!(required));
        }
        let request_body = json!({
            "required": !required.is_empty(),
            "content": {
                "application/json": {
                    "schema": serde_json::Value::Object(body_schema),
                }
            }
        });
        (None, Some(request_body))
    }
}

/// Build an OpenAPI 3.0.3 document from the collected tools. `base_url` is the
/// public origin (scheme://host[:port]) — the spec's server URL appends `/api`.
fn build_openapi_spec(
    title: &str,
    description: &str,
    version: &str,
    base_url: &str,
    tools: Vec<OpenApiToolRef>,
) -> serde_json::Value {
    let mut paths = serde_json::Map::new();
    let mut tags: Vec<serde_json::Value> = Vec::new();
    let mut seen_servers: Vec<String> = Vec::new();
    // OpenAPI requires unique operationIds; two servers can expose the same
    // bare tool name (e.g. two filesystem servers), so dedupe with the server
    // name as the disambiguator.
    let mut seen_operation_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for t in &tools {
        if !seen_servers.contains(&t.server) {
            seen_servers.push(t.server.clone());
            tags.push(json!({
                "name": t.server,
                "description": format!("Tools from {} server", t.server),
            }));
        }
        let (parameters, request_body) = tool_schema_shape(&t.tool);
        let operation_id = if seen_operation_ids.contains(&t.bare_name) {
            format!("{}_{}", t.server.replace(['-', '.', '/'], "_"), t.bare_name)
        } else {
            t.bare_name.clone()
        };
        seen_operation_ids.insert(operation_id.clone());
        let path_name = format!(
            "/tools/{}/{}",
            urlencode_component(&t.server),
            urlencode_component(&t.bare_name)
        );
        let mut operation = json!({
            "summary": t.tool.description.clone().unwrap_or_else(|| format!("Execute {} tool", t.bare_name)),
            "description": t.tool.description.clone().unwrap_or_else(|| format!("Execute the {} tool from {} server", t.bare_name, t.server)),
            "operationId": operation_id,
            "tags": [t.server],
            "responses": {
                "200": {"description": "Successful tool execution", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ToolResponse"}}}},
                "400": {"description": "Bad request - invalid parameters", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ErrorResponse"}}}},
                "500": {"description": "Internal server error", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ErrorResponse"}}}},
            }
        });
        if let Some(params) = parameters {
            operation["parameters"] = json!(params);
        }
        if let Some(body) = request_body {
            operation["requestBody"] = body;
        }
        let entry = paths
            .entry(path_name)
            .or_insert_with(|| json!({}));
        let method = if operation.get("requestBody").is_some() {
            "post"
        } else {
            "get"
        };
        entry[method] = operation;
    }
    json!({
        "openapi": "3.0.3",
        "info": {
            "title": title,
            "description": description,
            "version": version,
            "contact": {"name": "MCPHub Desktop", "url": "https://github.com/skrstop/MCPHub-Desktop"},
        },
        "servers": [{"url": format!("{base_url}/api"), "description": "MCPHub Desktop API Server"}],
        "paths": paths,
        "components": {
            "schemas": {
                "ToolResponse": {
                    "type": "object",
                    "properties": {
                        "content": {"type": "array", "items": {"type": "object", "properties": {"type": {"type": "string"}, "text": {"type": "string"}}}},
                        "isError": {"type": "boolean"},
                    }
                },
                "ErrorResponse": {
                    "type": "object",
                    "properties": {"error": {"type": "string"}, "message": {"type": "string"}}
                },
            },
            "securitySchemes": {
                "bearerAuth": {"type": "http", "scheme": "bearer"}
            }
        },
        "security": [{"bearerAuth": []}],
        "tags": tags,
    })
}

/// Minimal percent-encoding for path segments in the generated spec (keeps
/// unreserved characters; everything else %XX — matches encodeURIComponent for
/// the characters that matter in server/tool names).
fn urlencode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Public base URL for the spec's `servers` entry: prefer the request Host
/// header (reverse-proxy friendly), fall back to localhost:{running port}.
/// Always http:// — the desktop HTTP listener has no TLS; a TLS-terminating
/// proxy should set serverUrl=... explicitly (origin parity query param).
fn openapi_base_url(headers: &HeaderMap) -> String {
    if let Some(host) = headers.get("host").and_then(|v| v.to_str().ok()) {
        if !host.is_empty() {
            return format!("http://{host}");
        }
    }
    let port = current_status().port;
    format!("http://localhost:{}", if port > 0 { port } else { 23333 })
}

/// Origin-parity spec options parsed from the query string
/// (`?title=&description=&version=&serverUrl=&includeDisabled=`).
struct OpenApiSpecOptions {
    title: Option<String>,
    description: Option<String>,
    version: Option<String>,
    server_url: Option<String>,
    include_disabled: bool,
}

impl OpenApiSpecOptions {
    fn from_query(q: &std::collections::HashMap<String, String>) -> Self {
        let get = |k: &str| q.get(k).filter(|v| !v.is_empty()).cloned();
        Self {
            title: get("title"),
            description: get("description"),
            version: get("version"),
            server_url: get("serverUrl"),
            include_disabled: q.get("includeDisabled").map(|v| v == "true").unwrap_or(false),
        }
    }
}

fn openapi_spec_response(
    headers: &HeaderMap,
    opts: &OpenApiSpecOptions,
    default_title: &str,
    default_description: &str,
    tools: Vec<OpenApiToolRef>,
) -> Response {
    let base = opts
        .server_url
        .clone()
        .unwrap_or_else(|| openapi_base_url(headers));
    let spec = build_openapi_spec(
        opts.title.as_deref().unwrap_or(default_title),
        opts.description.as_deref().unwrap_or(default_description),
        opts.version.as_deref().unwrap_or("1.0.0"),
        &base,
        tools,
    );
    // YAML output is not generated (no serializer dependency); .yaml paths
    // serve the same JSON document, which every OpenAPI client accepts.
    (
        [(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")],
        Json(spec),
    )
        .into_response()
}

async fn openapi_full_spec(
    headers: HeaderMap,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let bearer_key = match check_bearer_auth(&headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let opts = OpenApiSpecOptions::from_query(&q);
    // `?group=` filter (origin parity): scope to a group; an unknown group
    // yields an EMPTY spec (no single-server fallback on this param — the
    // fallback only applies to the /api/{name}/openapi.json path form).
    let scope = match q.get("group") {
        Some(g) if !g.is_empty() => {
            let is_group = group_service::list_all()
                .await
                .ok()
                .map(|gs| gs.iter().any(|x| x.name == *g || x.id == *g))
                .unwrap_or(false);
            if !is_group {
                return openapi_spec_response(&headers, &opts, "MCPHub Desktop API", "", Vec::new());
            }
            Some(g.clone())
        }
        _ => None,
    };
    let mut tools = collect_openapi_tools(scope.as_deref(), opts.include_disabled).await;
    // `?servers=a,b` filter (origin parity): restrict to the listed servers.
    if let Some(servers) = q.get("servers").filter(|s| !s.is_empty()) {
        let wanted: Vec<&str> = servers.split(',').map(|s| s.trim()).collect();
        tools.retain(|t| wanted.contains(&t.server.as_str()));
    }
    if let Some(allowed) = get_allowed_servers(bearer_key.as_ref()).await {
        tools.retain(|t| allowed.contains(&t.server));
    }
    openapi_spec_response(
        &headers,
        &opts,
        "MCPHub Desktop API",
        "OpenAPI specification for MCP tools managed by MCPHub Desktop. Enables integration with OpenWebUI and other OpenAPI-compatible systems.",
        tools,
    )
}

async fn openapi_named_spec(
    headers: HeaderMap,
    Path(name): Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let bearer_key = match check_bearer_auth(&headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let opts = OpenApiSpecOptions::from_query(&q);
    // Group name first; mcp_scope_server_filters falls back to a single-server
    // scope (and the RAG builtin) when `name` is not a group.
    let group = group_service::list_all()
        .await
        .ok()
        .and_then(|gs| gs.into_iter().find(|g| g.name == name || g.id == name));
    let (scope, display, is_group) = match &group {
        Some(g) => (g.name.clone(), g.name.clone(), true),
        None => (name.clone(), name.clone(), false),
    };
    let (default_title, default_description) = if is_group {
        (
            format!("{display} Group MCP API"),
            format!("OpenAPI specification for {display} group tools"),
        )
    } else {
        (
            format!("{display} MCP API"),
            format!("OpenAPI specification for {display} MCP server tools"),
        )
    };
    let mut tools = collect_openapi_tools(Some(&scope), opts.include_disabled).await;
    if let Some(allowed) = get_allowed_servers(bearer_key.as_ref()).await {
        tools.retain(|t| allowed.contains(&t.server));
    }
    if tools.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": "Server not found",
                "message": format!("Server '{name}' is not connected or does not exist"),
            })),
        )
            .into_response();
    }
    openapi_spec_response(&headers, &opts, &default_title, &default_description, tools)
}

/// Origin /api/openapi/servers parity: connected server names.
async fn openapi_servers_list(headers: HeaderMap) -> Response {
    let bearer_key = match check_bearer_auth(&headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let mut names: Vec<String> = pool::get_all_statuses()
        .await
        .into_iter()
        .filter(|s| s.connected)
        .map(|s| s.name)
        .collect();
    if let Some(allowed) = get_allowed_servers(bearer_key.as_ref()).await {
        names.retain(|n| allowed.contains(n));
    }
    names.sort();
    Json(json!({ "success": true, "data": names })).into_response()
}

/// Origin /api/openapi/stats parity: connected/tool totals + per-server breakdown.
async fn openapi_stats(headers: HeaderMap) -> Response {
    let bearer_key = match check_bearer_auth(&headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let mut statuses = pool::get_all_statuses().await;
    if let Some(allowed) = get_allowed_servers(bearer_key.as_ref()).await {
        statuses.retain(|s| allowed.contains(&s.name));
    }
    let breakdown: Vec<serde_json::Value> = statuses
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "toolCount": s.tool_count,
                "status": if s.connected { "connected" }
                    else if s.starting { "connecting" }
                    else if s.start_on_demand { "sleeping" }
                    else { "disconnected" },
            })
        })
        .collect();
    let total_tools: usize = statuses.iter().filter(|s| s.connected).map(|s| s.tool_count).sum();
    Json(json!({
        "success": true,
        "data": {
            "totalServers": statuses.iter().filter(|s| s.connected).count(),
            "totalTools": total_tools,
            "serverBreakdown": breakdown,
        }
    }))
    .into_response()
}

/// Coerce string query-parameter values to the types declared in the tool's
/// inputSchema (origin convertParametersToTypes equivalent).
fn coerce_query_args(query: std::collections::HashMap<String, String>, tool: &Tool) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    let props = tool
        .input_schema
        .get("properties")
        .and_then(|p| p.as_object());
    for (k, v) in query {
        let ty = props
            .and_then(|p| p.get(&k))
            .and_then(|p| p.get("type"))
            .and_then(|t| t.as_str());
        let value = match ty {
            Some("number") => v.parse::<f64>().map(|n| json!(n)).unwrap_or(json!(v)),
            Some("integer") => v.parse::<i64>().map(|n| json!(n)).unwrap_or(json!(v)),
            Some("boolean") => match v.to_lowercase().as_str() {
                "true" => json!(true),
                "false" => json!(false),
                _ => json!(v),
            },
            _ => json!(v),
        };
        obj.insert(k, value);
    }
    serde_json::Value::Object(obj)
}

/// Shared execution path for the four /api tools routes. `scope` None = global.
/// Resolves the tool by bare or server-prefixed name (origin semantics), gates
/// disabled tools, calls the shared pool, and writes an activity log entry.
async fn execute_openapi_impl(
    scope: Option<String>,
    server_name: String,
    tool_name: String,
    args: serde_json::Value,
    headers: &HeaderMap,
    source_ip: Option<&str>,
) -> Response {
    let bearer_key = match check_bearer_auth(headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    if let Some(allowed) = get_allowed_servers(bearer_key.as_ref()).await {
        if !allowed.contains(&server_name) {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "Access denied for this server" })),
            )
                .into_response();
        }
    }
    // Scoped variant: verify the server is actually reachable under the scope
    // (a group's server list, or the named server itself).
    if let Some(ref sc) = scope {
        let sfs = mcp_scope_server_filters(sc).await;
        if !sfs.iter().any(|f| f.name == server_name) {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": format!("Server '{server_name}' not found in scope '{sc}'") })),
            )
                .into_response();
        }
    }
    let tools = match pool::list_tools_for(&server_name).await {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    };
    let name_sep = name_separator().await;
    let prefixed = format!("{}{}", server_name, name_sep);
    let actual_name = match tools.iter().find(|t| t.name == tool_name) {
        Some(_) => tool_name.clone(),
        None => {
            if tools.iter().any(|t| t.name == format!("{prefixed}{tool_name}")) {
                format!("{prefixed}{tool_name}")
            } else {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({ "error": format!("Tool '{tool_name}' not found on server '{server_name}'") })),
                )
                    .into_response();
            }
        }
    };
    // Disabled-tool gate (origin #1178 parity with /rest and /mcp).
    let filtered = server_tool_config_service::apply_tool_filters(&server_name, tools)
        .await
        .unwrap_or_default();
    if let Some(t) = filtered.iter().find(|t| t.name == actual_name) {
        if !t.enabled {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": format!("Tool '{}' is disabled", tool_name) })),
            )
                .into_response();
        }
    }
    let start = std::time::Instant::now();
    let result = pool::call_tool(&server_name, &actual_name, args.clone()).await;
    let duration_ms = start.elapsed().as_millis() as i64;
    match result {
        Ok(r) => {
            let status = if r.is_error { "error" } else { "success" };
            log::info!("[OpenAPI] Tool '{}' call {} on server '{}' ({}ms)", actual_name, status, server_name, duration_ms);
            let output = serde_json::to_value(&r).ok();
            let _ = log_service::write_activity(
                &server_name,
                &actual_name,
                Some(duration_ms),
                status,
                Some(args),
                output,
                None,
                source_ip,
            )
            .await;
            Json(json!({ "content": r.content, "isError": r.is_error })).into_response()
        }
        Err(e) => {
            let _ = log_service::write_activity(
                &server_name,
                &actual_name,
                Some(duration_ms),
                "error",
                Some(args),
                None,
                Some(&e.to_string()),
                source_ip,
            )
            .await;
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Failed to execute tool", "message": e.to_string() })),
            )
                .into_response()
        }
    }
}

fn client_ip_of(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty())
        })
}

async fn openapi_exec_global_get(
    headers: HeaderMap,
    Path((server, tool)): Path<(String, String)>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let ip = client_ip_of(&headers);
    let args = match pool::list_tools_for(&server).await {
        Ok(ts) => {
            let name_sep = name_separator().await;
            let prefixed = format!("{}{}", server, name_sep);
            let actual = ts.iter().find(|t| t.name == tool)
                .or_else(|| ts.iter().find(|t| t.name == format!("{prefixed}{tool}")));
            match actual {
                Some(t) => coerce_query_args(query, t),
                None => json!({}),
            }
        }
        Err(_) => json!({}),
    };
    execute_openapi_impl(None, server, tool, args, &headers, ip.as_deref()).await
}

async fn openapi_exec_global_post(
    headers: HeaderMap,
    Path((server, tool)): Path<(String, String)>,
    body: Option<Json<serde_json::Value>>,
) -> Response {
    let ip = client_ip_of(&headers);
    let args = body.and_then(|Json(v)| v.as_object().cloned()).unwrap_or_default();
    execute_openapi_impl(None, server, tool, serde_json::Value::Object(args), &headers, ip.as_deref()).await
}

async fn openapi_exec_scoped_get(
    headers: HeaderMap,
    Path((scope, server, tool)): Path<(String, String, String)>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let ip = client_ip_of(&headers);
    let args = match pool::list_tools_for(&server).await {
        Ok(ts) => {
            let name_sep = name_separator().await;
            let prefixed = format!("{}{}", server, name_sep);
            let actual = ts.iter().find(|t| t.name == tool)
                .or_else(|| ts.iter().find(|t| t.name == format!("{prefixed}{tool}")));
            match actual {
                Some(t) => coerce_query_args(query, t),
                None => json!({}),
            }
        }
        Err(_) => json!({}),
    };
    execute_openapi_impl(Some(scope), server, tool, args, &headers, ip.as_deref()).await
}

async fn openapi_exec_scoped_post(
    headers: HeaderMap,
    Path((scope, server, tool)): Path<(String, String, String)>,
    body: Option<Json<serde_json::Value>>,
) -> Response {
    let ip = client_ip_of(&headers);
    let args = body.and_then(|Json(v)| v.as_object().cloned()).unwrap_or_default();
    execute_openapi_impl(Some(scope), server, tool, serde_json::Value::Object(args), &headers, ip.as_deref()).await
}

// ────────────────────────────────────────────────────────────────────────────
// Router
// ────────────────────────────────────────────────────────────────────────────

fn build_router(body_limit_bytes: usize) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/.well-known/oauth-protected-resource", get(oauth_protected_resource))
        .route("/servers", get(list_servers))
        // Legacy REST API (moved to /rest prefix to avoid wildcard conflict)
        .route("/rest/{server}/tools", get(list_server_tools))
        .route("/rest/{server}/call", post(call_server_tool))
        .route("/rest/group/{group}/tools", get(list_group_tools))
        .route("/rest/group/{group}/call", post(call_group_tool))
        // OpenAPI-compatible endpoints (spec generation + tool execution).
        // Literal routes win over the {name} params; .yaml paths serve the same
        // JSON document (no YAML serializer dependency).
        .route("/api/openapi.json", get(openapi_full_spec))
        .route("/api/openapi.yaml", get(openapi_full_spec))
        .route("/api/openapi/servers", get(openapi_servers_list))
        .route("/api/openapi/stats", get(openapi_stats))
        .route("/api/{name}/openapi.json", get(openapi_named_spec))
        .route("/api/{name}/openapi.yaml", get(openapi_named_spec))
        .route(
            "/api/tools/{server}/{tool}",
            get(openapi_exec_global_get).post(openapi_exec_global_post),
        )
        .route(
            "/api/{name}/tools/{server}/{tool}",
            get(openapi_exec_scoped_get).post(openapi_exec_scoped_post),
        )
        // MCP Streamable HTTP protocol (JSON-RPC 2.0)
        .route("/mcp", get(mcp_root_get).post(mcp_root_post).delete(mcp_root_delete))
        // Legacy 2024-11-05 SSE transport: POST target named in the `endpoint`
        // event. Static route wins over the /mcp/*path wildcard below.
        .route("/mcp/message", post(mcp_message_post))
        .route("/mcp/{*path}", get(mcp_scope_get).post(mcp_scope_post).delete(mcp_scope_delete))
        .layer(axum::extract::DefaultBodyLimit::max(body_limit_bytes))
        .layer(CorsLayer::permissive())
}

// ────────────────────────────────────────────────────────────────────────────
// Lifecycle
// ────────────────────────────────────────────────────────────────────────────

/// Start the HTTP server on the given port with the given body limit.
/// If a server is already running on the same port and limit — nothing to do.
/// Otherwise the old instance is stopped and a new one started.
pub async fn start(port: u16, body_limit_bytes: usize) -> anyhow::Result<()> {
    let mut guard = handle().lock().await;

    // Already running with the same port and body limit — nothing to do
    if let Some(ref h) = *guard {
        if h.port == port && h.body_limit_bytes == body_limit_bytes {
            log::info!("HTTP server already running on port {}", port);
            return Ok(());
        }
        // Port or body limit changed — stop old instance
        log::info!("HTTP server config changed, restarting...");
    }

    let app = build_router(body_limit_bytes);

    // Start the tasks TTL sweeper (drops expired 2025-11-25 tasks).
    mcp_tasks::spawn_ttl_sweeper();

    // Check TRUST_PROXY environment variable
    let trust_proxy = std::env::var("TRUST_PROXY").unwrap_or_default().to_lowercase();
    let trust_proxy = trust_proxy == "true" || trust_proxy == "1" || trust_proxy == "yes";

    let addr: SocketAddr = format!("0.0.0.0:{}", port).parse()?;
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            let err_msg = bind_failure_message(port, &e);
            log::error!("{}", err_msg);
            app_logger::log_to_db("error", &err_msg);
            // Surface to the UI: emit a status event (live toast) + stash it so
            // the frontend can fetch it on mount if it missed this (startup race).
            set_status(HttpServerStatus {
                running: false,
                port,
                error: Some(err_msg.clone()),
                error_kind: Some(bind_failure_kind(&e).to_string()),
                detail: Some(format!("{e}")),
            });
            return Err(anyhow::anyhow!(err_msg));
        }
    };
    let http_msg = format!("MCPHub HTTP server listening on http://0.0.0.0:{} (body limit: {} bytes, trust_proxy: {})", port, body_limit_bytes, trust_proxy);
    log::info!("{}", http_msg);
    app_logger::log_to_db("info", &http_msg);

    // On Windows, external clients are commonly blocked by Windows Defender
    // Firewall even though the bind succeeded (loopback works, 0.0.0.0 inbound
    // doesn't). Log a proactive hint so "started but unreachable" shows up in the
    // logs, not just "listening" with no clue why clients can't connect.
    #[cfg(windows)]
    {
        let fw_hint = format!(
            "If external clients cannot connect on port {p}, allow this app through Windows Defender Firewall \
             (inbound TCP {p}). Loopback (127.0.0.1) is unaffected.",
            p = port
        );
        log::info!("[firewall] {fw_hint}");
        app_logger::log_to_db("info", &fw_hint);
    }

    set_status(HttpServerStatus {
        running: true,
        port,
        error: None,
        error_kind: None,
        detail: None,
    });

    let (abort_tx, abort_rx) = tokio::sync::oneshot::channel::<()>();
    // Diagnostics probe (2026-08-27 "HTTP 服务静默挂掉" investigation): a second
    // channel fired alongside abort_tx in stop(). abort_rx is consumed by the
    // graceful-shutdown future and can't be probed after the fact; this probe
    // lets the serve task classify its own ending as intentional vs unexpected.
    let (probe_tx, mut abort_rx_probe) = tokio::sync::oneshot::channel::<()>();

    tokio::spawn(async move {
        // FIX (2026-08-27): an earlier attempt to bound graceful shutdown
        // (mirroring upstream #1042) wrapped tokio::time::timeout around the
        // WHOLE serve future - which killed the server exactly 10s after
        // startup: the timeout elapsed, the serve future was dropped (listener
        // closed, fds released) while the status still said running, and
        // nothing restarted it. Symptom: external clients can't connect
        // anymore, lsof shows no listener, app process alive.
        //
        // Correct structure: serve runs UNBOUNDED; with_graceful_shutdown fires
        // when abort_rx completes (stop()/config restart). The outcome is then
        // classified for the [http-server-watch] diagnostics trail - including
        // panics, which tokio otherwise swallows silently at the task boundary.
        let serve = axum::serve(listener, app).with_graceful_shutdown(async {
            let _ = abort_rx.await;
        });
        let serve_fut = std::future::IntoFuture::into_future(serve);
        let outcome = match futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(serve_fut)).await {
            Ok(Ok(())) => "clean stop".to_string(),
            Ok(Err(e)) => format!("accept-loop error: {e}"),
            Err(panic) => format!(
                "PANIC: {}",
                panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "(non-string panic payload)".to_string())
            ),
        };
        //   Ok(())            -> stop() explicitly fired the probe
        //   Err(Disconnected) -> ServerHandle dropped = start() replacing it
        //                        (port/body-limit config restart) - intentional
        //   Err(Empty)        -> nobody asked; serve died on its own
        let stopped_intentionally =
            !matches!(abort_rx_probe.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty));
        let line = format!(
            "[http-server-watch] serve task ended ({}): {}",
            if stopped_intentionally { "intentional stop" } else { "UNEXPECTED - serve died without a stop request" },
            outcome
        );
        log::warn!("{}", line);
        app_logger::log_to_db("warn", &line);
        log::info!("MCPHub HTTP server stopped");
    });

    *guard = Some(ServerHandle { abort_tx, probe_tx, port, body_limit_bytes });
    Ok(())
}

/// Stop the HTTP server if it is running.
pub async fn stop() {
    let mut guard = handle().lock().await;
    if let Some(h) = guard.take() {
        let _ = h.abort_tx.send(());
        let _ = h.probe_tx.send(());
        log::info!("MCPHub HTTP server shutdown requested");
        set_status(HttpServerStatus {
            running: false,
            port: h.port,
            error: None,
            error_kind: None,
            detail: None,
        });
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
                .unwrap_or(true);
            if expose {
                let port = config
                    .get("httpPort")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(23333) as u16;
                let body_limit_str = config
                    .get("routing")
                    .and_then(|r| r.get("jsonBodyLimit"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("1mb");
                let body_limit_bytes = parse_body_limit(body_limit_str);
                if let Err(e) = start(port, body_limit_bytes).await {
                    let err_msg = format!("Failed to start HTTP server on port {}: {}", port, e);
                    log::error!("{}", err_msg);
                    app_logger::log_to_db("error", &err_msg);
                }
            }
        }
        Err(e) => {
            let warn_msg = format!("Could not read config for HTTP server startup: {}", e);
            log::warn!("{}", warn_msg);
            app_logger::log_to_db("warn", &warn_msg);
        }
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
                .unwrap_or(true);
            if expose {
                let port = config
                    .get("httpPort")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(23333) as u16;
                let body_limit_str = config
                    .get("routing")
                    .and_then(|r| r.get("jsonBodyLimit"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("1mb");
                let body_limit_bytes = parse_body_limit(body_limit_str);
                if let Err(e) = start(port, body_limit_bytes).await {
                    log::error!("Failed to start HTTP server: {}", e);
                }
            } else {
                stop().await;
            }
        }
        Err(e) => log::warn!("Could not read config for HTTP server sync: {}", e),
    }
}

#[cfg(test)]
mod openapi_tests {
    use super::*;

    /// matchit panics at Router build time on conflicting routes; the /api
    /// literal-vs-param mix ("/api/openapi.json" vs "/api/{name}/openapi.json",
    /// "/api/tools/{server}/{tool}" vs "/api/{name}/tools/{server}/{tool}") must
    /// coexist with static segments winning. Build the router here so a conflict
    /// surfaces as a test failure instead of an app-startup panic.
    #[test]
    fn router_builds_with_openapi_routes() {
        let _ = build_router(1024 * 1024);
    }

    #[test]
    fn coerce_query_args_converts_schema_types() {
        let tool = Tool {
            name: "t".to_string(),
            description: None,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "n": {"type": "number"},
                    "i": {"type": "integer"},
                    "b": {"type": "boolean"},
                    "s": {"type": "string"}
                }
            }),
            server_name: "srv".to_string(),
            enabled: true,
            annotations: None,
            output_schema: None,
        };
        let mut q = std::collections::HashMap::new();
        q.insert("n".to_string(), "1.5".to_string());
        q.insert("i".to_string(), "42".to_string());
        q.insert("b".to_string(), "TRUE".to_string());
        q.insert("s".to_string(), "hello".to_string());
        q.insert("junk_n".to_string(), "NaN!".to_string());
        let out = coerce_query_args(q, &tool);
        assert_eq!(out["n"], json!(1.5));
        assert_eq!(out["i"], json!(42));
        assert_eq!(out["b"], json!(true));
        assert_eq!(out["s"], json!("hello"));
        // Unparseable numbers fall back to the raw string (origin parity).
        assert_eq!(out["junk_n"], json!("NaN!"));
        // Unknown keys pass through as strings.
        let mut q2 = std::collections::HashMap::new();
        q2.insert("unknown".to_string(), "x".to_string());
        let out2 = coerce_query_args(q2, &tool);
        assert_eq!(out2["unknown"], json!("x"));
    }

    #[test]
    fn spec_options_parse_origin_query_params() {
        let mut q = std::collections::HashMap::new();
        q.insert("title".to_string(), "Custom".to_string());
        q.insert("serverUrl".to_string(), "https://proxy.example".to_string());
        q.insert("includeDisabled".to_string(), "true".to_string());
        let o = OpenApiSpecOptions::from_query(&q);
        assert_eq!(o.title.as_deref(), Some("Custom"));
        assert_eq!(o.server_url.as_deref(), Some("https://proxy.example"));
        assert!(o.include_disabled);
        assert!(o.description.is_none());
        assert!(o.version.is_none());
        // includeDisabled=anything-else is false (origin compares === 'true').
        q.insert("includeDisabled".to_string(), "1".to_string());
        assert!(!OpenApiSpecOptions::from_query(&q).include_disabled);
    }

    #[test]
    fn spec_shape_routes_simple_tools_to_get_and_complex_to_post() {
        let mk = |schema: serde_json::Value| Tool {
            name: "echo".to_string(),
            description: None,
            input_schema: schema,
            server_name: "test".to_string(),
            enabled: true,
            annotations: None,
            output_schema: None,
        };
        // Pure number/boolean params → GET query parameters.
        let simple = mk(json!({
            "type": "object",
            "properties": {
                "n": {"type": "number"},
                "b": {"type": "boolean"}
            },
            "required": ["n"]
        }));
        let (params, body) = tool_schema_shape(&simple);
        assert!(params.is_some() && body.is_none(), "simple tool should use query params");
        assert_eq!(params.as_ref().unwrap().len(), 2);

        // A string prop counts as complex (origin parity) → POST body.
        let complex = mk(json!({
            "type": "object",
            "properties": {"s": {"type": "string"}}
        }));
        let (params, body) = tool_schema_shape(&complex);
        assert!(params.is_none() && body.is_some(), "string prop must route to requestBody");

        // No schema → neither.
        let bare = mk(json!({}));
        let (params, body) = tool_schema_shape(&bare);
        assert!(params.is_none() && body.is_none());
    }

    #[test]
    fn spec_operation_ids_are_unique_across_servers() {
        let mk_tool = |name: &str| Tool {
            name: name.to_string(),
            description: None,
            input_schema: json!({"type": "object", "properties": {"q": {"type": "number"}}}),
            server_name: "test".to_string(),
            enabled: true,
            annotations: None,
            output_schema: None,
        };
        let tools = vec![
            OpenApiToolRef { server: "fs-a".into(), bare_name: "read".into(), tool: mk_tool("fs-a-read") },
            OpenApiToolRef { server: "fs-b".into(), bare_name: "read".into(), tool: mk_tool("fs-b-read") },
            OpenApiToolRef { server: "fs-a".into(), bare_name: "list".into(), tool: mk_tool("fs-a-list") },
        ];
        let spec = build_openapi_spec("t", "d", "1.0.0", "http://localhost:1", tools);
        let paths = spec.get("paths").and_then(|p| p.as_object()).unwrap();
        assert_eq!(paths.len(), 3, "three tools across two servers -> three paths");
        let mut ids = Vec::new();
        for (path, ops) in paths {
            for (method, op) in ops.as_object().unwrap() {
                ids.push((path.clone(), method.to_string(),
                    op.get("operationId").and_then(|v| v.as_str()).unwrap().to_string()));
            }
        }
        let mut unique = std::collections::HashSet::new();
        for (_, _, id) in &ids {
            assert!(unique.insert(id.clone()), "duplicate operationId: {id}");
        }
        assert!(ids.iter().any(|(_, m, _)| m == "get"), "simple tools are GET");
    }

    #[test]
    fn urlencode_keeps_unreserved_and_escapes_slash() {
        assert_eq!(urlencode_component("fs-a_tool.1~"), "fs-a_tool.1~");
        assert_eq!(urlencode_component("a/b c"), "a%2Fb%20c");
        assert_eq!(urlencode_component("中文"), "%E4%B8%AD%E6%96%87");
    }
}
