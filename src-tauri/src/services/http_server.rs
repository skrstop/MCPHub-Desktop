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
    mcp::pool,
    models::{bearer_key::BearerKey, server::Tool},
    services::{bearer_key_service, config_service, group_service, server_tool_config_service},
};
use axum::response::IntoResponse;
use axum::{
    body::Body,
    extract::Path,
    http::{header, HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        Json, Response,
    },
    routing::{get, post},
    Router,
};
use futures_util::StreamExt;
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    net::SocketAddr,
    sync::{Arc, OnceLock},
};
use tokio::{net::TcpListener, sync::Mutex};
use tower_http::cors::CorsLayer;

fn new_session_id() -> String {
    let id: u128 = rand::thread_rng().gen();
    format!("{:032x}", id)
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
                        servers.extend(g.servers);
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
                        servers.extend(g.servers);
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
    let accessible: Vec<&String> = group.servers.iter()
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
    let mut target_server: Option<String> = None;
    for server_name in &group.servers {
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

/// Resolve a scope path to the list of connected server names.
/// - "" or "$smart"              → all connected servers
/// - "$smart/{group}"            → servers in that group
/// - "{name}"                    → group by name/id, or single server
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
            return g.servers.clone();
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

/// Core MCP JSON-RPC dispatcher.
async fn dispatch_mcp(headers: HeaderMap, scope: String, body: Value) -> Response {
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

    // Notifications have no "id" — respond with 202 Accepted, no body.
    if id.is_none() && (method.starts_with("notifications/") || method == "ping") {
        return axum::http::Response::builder()
            .status(StatusCode::ACCEPTED)
            .body(Body::empty())
            .unwrap();
    }

    match method {
        "initialize" => {
            let session_id = new_session_id();
            let mut resp = jsonrpc_response(
                id,
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "MCPHub Desktop", "version": "0.12.12"}
                }),
            );
            resp.headers_mut().insert(
                "mcp-session-id",
                session_id.parse().expect("valid header value"),
            );
            resp
        }
        "ping" => jsonrpc_response(id, json!({})),
        "tools/list" => {
            let mut server_names = mcp_scope_servers(&scope).await;
            // Apply bearer key access control
            if let Some(allowed) = get_allowed_servers(bearer_key.as_ref()).await {
                server_names.retain(|s| allowed.contains(s));
            }
            // Prefix tool names with server name when multiple servers are in scope
            let use_prefix = server_names.len() > 1;
            let mut tools: Vec<Value> = Vec::new();
            for sn in &server_names {
                if let Ok(ts) = pool::list_tools_for(sn).await {
                    let filtered = server_tool_config_service::apply_tool_filters(sn, ts)
                        .await
                        .unwrap_or_else(|_| vec![]);
                    for t in &filtered {
                        let exposed_name = if use_prefix {
                            format!("{}{}{}", sn, name_sep, t.name)
                        } else {
                            t.name.clone()
                        };
                        tools.push(json!({
                            "name": exposed_name,
                            "description": t.description.as_deref().unwrap_or(""),
                            "inputSchema": t.input_schema,
                        }));
                    }
                }
            }
            jsonrpc_response(id, json!({"tools": tools}))
        }
        "tools/call" => {
            let tool_name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let mut server_names = mcp_scope_servers(&scope).await;
            // Apply bearer key access control
            if let Some(allowed) = get_allowed_servers(bearer_key.as_ref()).await {
                server_names.retain(|s| allowed.contains(s));
            }
            let use_prefix = server_names.len() > 1;

            // Resolve server + original tool name (strip nameSeparator prefix if needed)
            let mut target: Option<(String, String)> = None;
            if use_prefix {
                // Try to find a server whose prefix matches the tool_name
                for sn in &server_names {
                    let prefix = format!("{}{}", sn, name_sep);
                    if tool_name.starts_with(&prefix) {
                        let orig_name = &tool_name[prefix.len()..];
                        if let Ok(ts) = pool::list_tools_for(sn).await {
                            if ts.iter().any(|t| t.name == orig_name) {
                                target = Some((sn.clone(), orig_name.to_string()));
                                break;
                            }
                        }
                    }
                }
            }
            // Fallback: search by original name (single-server scope or unprefixed call)
            if target.is_none() {
                for sn in &server_names {
                    if let Ok(ts) = pool::list_tools_for(sn).await {
                        if ts.iter().any(|t| t.name == tool_name) {
                            target = Some((sn.clone(), tool_name.to_string()));
                            break;
                        }
                    }
                }
            }
            match target {
                None => jsonrpc_error(id, -32602, format!("Tool '{}' not found", tool_name)),
                Some((sn, orig_name)) => match pool::call_tool(&sn, &orig_name, args).await {
                    Ok(r) => jsonrpc_response(id, json!({"content": r.content, "isError": r.is_error})),
                    Err(e) => jsonrpc_error(id, -32603, e.to_string()),
                },
            }
        }
        _ => jsonrpc_error(id, -32601, "Method not found"),
    }
}

async fn mcp_root_post(headers: HeaderMap, Json(body): Json<Value>) -> Response {
    dispatch_mcp(headers, String::new(), body).await
}

async fn mcp_scope_post(
    headers: HeaderMap,
    Path(path): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    dispatch_mcp(headers, path, body).await
}

async fn mcp_root_get(headers: HeaderMap) -> Response {
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
            "version": "0.12.12",
            "transport": "MCP Streamable HTTP",
            "usage": {
                "initialize": "POST /mcp  body: {\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{},\"clientInfo\":{\"name\":\"client\",\"version\":\"1.0\"}}}",
                "tools_list": "POST /mcp  body: {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}",
                "sse_stream": "GET /mcp  Accept: text/event-stream"
            }
        })).into_response();
    }
    // SSE stream for server-initiated messages (MCP Streamable HTTP spec)
    let stream = tokio_stream::wrappers::IntervalStream::new(
        tokio::time::interval(std::time::Duration::from_secs(25)),
    )
    .map(|_| Ok::<Event, std::convert::Infallible>(Event::default().comment("keep-alive")));
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn mcp_scope_get(headers: HeaderMap, Path(_): Path<String>) -> Response {
    mcp_root_get(headers).await
}

async fn mcp_root_delete() -> StatusCode {
    StatusCode::OK
}

async fn mcp_scope_delete(Path(_): Path<String>) -> StatusCode {
    StatusCode::OK
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
// Router
// ────────────────────────────────────────────────────────────────────────────

fn build_router(body_limit_bytes: usize) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/.well-known/oauth-protected-resource", get(oauth_protected_resource))
        .route("/servers", get(list_servers))
        // Legacy REST API (moved to /rest prefix to avoid wildcard conflict)
        .route("/rest/:server/tools", get(list_server_tools))
        .route("/rest/:server/call", post(call_server_tool))
        .route("/rest/group/:group/tools", get(list_group_tools))
        .route("/rest/group/:group/call", post(call_group_tool))
        // MCP Streamable HTTP protocol (JSON-RPC 2.0)
        .route("/mcp", get(mcp_root_get).post(mcp_root_post).delete(mcp_root_delete))
        .route("/mcp/*path", get(mcp_scope_get).post(mcp_scope_post).delete(mcp_scope_delete))
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

    let addr: SocketAddr = format!("0.0.0.0:{}", port).parse()?;
    let listener = TcpListener::bind(addr).await?;
    log::info!("MCPHub HTTP server listening on http://0.0.0.0:{} (body limit: {} bytes)", port, body_limit_bytes);

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

    *guard = Some(ServerHandle { abort_tx, port, body_limit_bytes });
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
