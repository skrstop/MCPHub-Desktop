/// http_server — embedded Axum HTTP server exposing MCP endpoints to external AI clients.
///
/// When `system_config.expose_http = true`, this service starts an HTTP server on the
/// configured port (default 23333). External clients can connect via SSE or Streamable HTTP
/// to access all connected MCP servers.
///
/// Endpoints:
///   GET  /health                    — health check
///   GET  /servers                   — list available servers
///   /mcp, /mcp/{scope}              — MCP Streamable HTTP (JSON-RPC via rmcp;
///                                     scope = "" | $smart | {group} | {server})
///   /rest/... , /api/...            — REST & OpenAPI-compatible surfaces
use crate::{
    mcp::pool,
    models::{bearer_key::BearerKey, server::Tool},
    services::{
        app_logger, bearer_key_service, config_service, group_service, log_service,
        mcp_tasks, server_tool_config_service,
    },
};
use axum::response::IntoResponse;
use axum::{
    body::Body,
    extract::{Path, Query},
    http::{header, HeaderMap, StatusCode},
    response::{
        Json, Response,
    },
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    net::SocketAddr,
    sync::{Arc, OnceLock},
};
use tokio::{net::TcpListener, sync::Mutex};
use tower_http::cors::CorsLayer;
use tauri::Emitter;

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
    // www_auth embeds a client-influenced Host value — never let a bad header
    // value panic the worker.
    let resp = axum::http::Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(header::CONTENT_TYPE, "application/json");
    let resp = match axum::http::HeaderValue::from_str(&www_auth) {
        Ok(v) => resp.header("www-authenticate", v),
        Err(_) => resp,
    };
    resp.body(Body::from(b)).unwrap_or_else(|_| {
        axum::http::Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(Body::from(String::from("{\"error\":\"invalid_token\"}")))
            .expect("static fallback response is valid")
    })
}

// ────────────────────────────────────────────────────────────────────────────
// Global server state
// ────────────────────────────────────────────────────────────────────────────

struct ServerHandle {
    /// SDK cancellation token (rmcp_service config): cancelled in stop() so
    /// in-flight SSE/streaming handlers terminate with the listener instead
    /// of relying on body-drop (documented SDK shutdown mechanism).
    sdk_cancel: tokio_util::sync::CancellationToken,
    abort_tx: tokio::sync::oneshot::Sender<()>,
    /// Shutdown sender for the loopback serve task (separate oneshot —
    /// receivers are single-consumer).
    abort_tx_lb: tokio::sync::oneshot::Sender<()>,
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
            return n.saturating_mul(1024 * 1024);
        }
    }
    if let Some(num) = s.strip_suffix("kb") {
        if let Ok(n) = num.trim().parse::<usize>() {
            return n.saturating_mul(1024);
        }
    }
    if let Some(num) = s.strip_suffix('b') {
        if let Ok(n) = num.trim().parse::<usize>() {
            return n.saturating_mul(1);
        }
    }
    if let Ok(n) = s.parse::<usize>() {
        return n;
    }
    1024 * 1024 // default 1 MiB
}

static SERVER_HANDLE: OnceLock<Arc<Mutex<Option<ServerHandle>>>> = OnceLock::new();
/// Monotonic generation of HTTP server instances. Incremented by every
/// successful start(); lets the watch task detect that the handle it is about
/// to clear still belongs to ITS instance (a newer start() may have raced it).
static HTTP_START_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Snapshot of the active server's body_limit_bytes — the leniency middleware
/// reads it per-request WITHOUT taking the global SERVER_HANDLE mutex (start()
/// holds that mutex across bind-probe retries, stalling every in-flight /mcp
/// POST for up to ~10s otherwise).
static BODY_LIMIT_SNAPSHOT: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(8 * 1024 * 1024);

/// Shared LocalSessionManager so the idle reaper can enumerate/terminate
/// sessions the SDK would otherwise keep forever (clients that never send
/// DELETE — killed process, closed tab — leak isolated upstream clients).
fn shared_session_manager() -> &'static std::sync::Arc<rmcp::transport::streamable_http_server::session::local::LocalSessionManager> {
    static MGR: OnceLock<std::sync::Arc<rmcp::transport::streamable_http_server::session::local::LocalSessionManager>> = OnceLock::new();
    MGR.get_or_init(Default::default)
}

/// Last time each mcp-session-id was seen on a request (idle-reaper input).
/// Cap-free: entries are removed by the reaper when the session dies.
static SESSION_LAST_SEEN: OnceLock<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>> = OnceLock::new();

fn session_last_seen() -> &'static std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>> {
    SESSION_LAST_SEEN.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Sessions idle longer than this are reaped. The SDK worker does idle-exit
/// after 5min (SessionConfig::keep_alive default, local.rs) but the session
/// ENTRY stays in the manager's map and our upstream isolation state
/// (mcp/session_pool) is never cleaned by the SDK — hence this reaper.
const SESSION_IDLE_REAP_SECS: u64 = 30 * 60;

fn spawn_session_reaper() {
    // Idempotent: start() runs on every restart (port/limit change) — without
    // this guard each restart spawns another 60s loop (unbounded over a
    // long-lived process).
    static REAPER_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if REAPER_STARTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            let now = std::time::Instant::now();
            let expired: Vec<String> = {
                let map = session_last_seen().lock().unwrap_or_else(|p| p.into_inner());
                map.iter()
                    .filter(|(_, t)| now.duration_since(**t).as_secs() >= SESSION_IDLE_REAP_SECS)
                    .map(|(k, _)| k.clone())
                    .collect()
            };
            if expired.is_empty() {
                continue;
            }
            let mgr = shared_session_manager();
            for sid in expired {
                // Only reap sessions the SDK still holds (an unknown sid was
                // already terminated — just drop the timestamp).
                let alive = mgr.sessions.read().await.keys().any(|k| k.as_ref() == sid.as_str());
                if alive {
                    // Reap via the pub sessions map (same steps as the SDK's
                    // SessionManager::close_session — remove + handle.close).
                    // Arc<LocalSessionManager> does NOT implement the
                    // SessionManager trait (SDK impl is on the bare type), so
                    // trait close_session is unreachable from a shared Arc.
                    let handle = {
                        let mut sessions = mgr.sessions.write().await;
                        // Re-check last-seen under the same write lock: a
                        // request between the expired snapshot and here must
                        // keep the session alive (touch/reap race).
                        let fresh = session_last_seen()
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .get(&sid)
                            .map(|t| now.duration_since(*t).as_secs() < SESSION_IDLE_REAP_SECS)
                            .unwrap_or(true); // no stamp = unknown activity, keep
                        if fresh {
                            None
                        } else {
                            sessions.remove(sid.as_str())
                        }
                    };
                    if let Some(handle) = handle {
                        if let Err(e) = handle.close().await {
                            log::debug!("[session-reaper] close {sid}: {e}");
                        }
                    }
                    crate::mcp::session_pool::cleanup_session(&sid).await;
                    log::info!("[session-reaper] reaped idle session {sid}");
                }
                session_last_seen().lock().unwrap_or_else(|p| p.into_inner()).remove(&sid);
            }
        }
    });
}

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
    /// Loopback-hijack warning (server IS running, but 127.0.0.1:<port> answers
    /// with a foreign /health — another app bound localhost on the same port,
    /// so local clients never reach us). Emitted as part of the status event
    /// so the frontend can toast it; does NOT flip `running` to false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

static HTTP_STATUS: OnceLock<std::sync::Mutex<HttpServerStatus>> = OnceLock::new();

fn status_lock() -> &'static std::sync::Mutex<HttpServerStatus> {
    HTTP_STATUS.get_or_init(|| std::sync::Mutex::new(HttpServerStatus {
        running: false,
        port: 0,
        error: None,
        error_kind: None,
        detail: None,
        warning: None,
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
                warning: None,
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
pub(crate) async fn check_bearer_auth(headers: &HeaderMap) -> Result<Option<BearerKey>, Response> {
    // Dynamically read config so changes take effect without restarting the HTTP server.
    // Read failure must FAIL CLOSED: a transient DB error must not bypass bearer auth.
    let config = match config_service::get().await {
        Ok(c) => c,
        Err(e) => {
            log::warn!("[http] config read failed during bearer auth check: {e}; failing closed (401)");
            return Err(build_oauth_401(headers, "invalid"));
        }
    };
    let enabled = config
        .get("routing")
        // UI saves under routing.enableBearerAuth; legacy path: bearerKeyEnabled
        .and_then(|r| r.get("enableBearerAuth"))
        .and_then(|v| v.as_bool())
        .or_else(|| config.get("bearerKeyEnabled").and_then(|v| v.as_bool()))
        .unwrap_or(false);
    if !enabled {
        return Ok(None);
    }

    // Use the configured header name (defaults to "authorization")
    let header_name = config
        .get("routing")
        .and_then(|r| r.get("bearerAuthHeaderName"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_else(|| "authorization".to_string());

    let auth = headers
        .get(header_name.as_str())
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !auth.starts_with("Bearer ")
        && !auth.get(..7).map(|p| p.eq_ignore_ascii_case("bearer ")).unwrap_or(false)
    {
        return Err(build_oauth_401(headers, "missing"));
    }
    // Case-insensitive scheme per OAuth 2.0 (RFC 6750); the token starts
    // after the first space.
    let token = auth.split_once(' ').map(|(_, t)| t).unwrap_or(&auth[7..]);
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
pub(crate) async fn get_allowed_servers(key: Option<&BearerKey>) -> Option<HashSet<String>> {
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
        // Legacy rows may carry an empty access_type — treat as unrestricted
        // (pre-fix behaviour) so existing keys keep working.
        "" => None,
        // Unknown non-empty access_type: fail CLOSED. The previous `_ => None`
        // made any future/unknown enum value silently unrestricted — a
        // fail-open security smell.
        other => {
            log::warn!(
                "[bearer] unknown access_type '{}' on key '{}', denying all servers (fail-closed)",
                other, key.name
            );
            Some(HashSet::new())
        }
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
    // Smart Routing meta tools are exclusive to the $smart access points —
    // their names/schemas must not leak via the single-server REST listing
    // either (parity with list_group_tools and the /mcp bridge surface).
    let tools = if server_name == crate::rag::service::BUILTIN_SERVER_NAME {
        tools
            .into_iter()
            .filter(|t| !crate::smart_routing::meta::is_meta_tool(&t.name))
            .collect()
    } else {
        tools
    };
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
    // Meta tools are exclusive to $smart — never resolvable via the plain
    // single-server REST call path even when the target is the builtin.
    if server_name == crate::rag::service::BUILTIN_SERVER_NAME
        && crate::smart_routing::meta::is_meta_tool(&req.tool)
    {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("Tool '{}' not found on server '{}'", req.tool, server_name) })),
        )
            .into_response();
    }
    // Tool existence + disabled-tool gate (origin #1178): tools disabled via
    // server_tool_config must not remain executable through the REST endpoint,
    // and unknown tools answer 404 instead of surfacing as a 500 from the
    // pool call.
    let _known_enabled = match pool::list_tools_for(&server_name).await {
        Ok(tools) => {
            // apply_tool_filters failure must FAIL CLOSED: an empty filter
            // list would silently skip the disabled-tool gate (review round
            // 10; mirrors the bearer config fail-closed policy).
            let filtered = match server_tool_config_service::apply_tool_filters(&server_name, tools)
                .await
            {
                Ok(f) => f,
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "error": format!("tool gate check failed: {e}") })),
                    )
                        .into_response();
                }
            };
            match filtered.iter().find(|t| &t.name == &req.tool) {
                Some(t) if !t.enabled => {
                    return (
                        StatusCode::FORBIDDEN,
                        Json(json!({ "error": format!("Tool '{}' is disabled", req.tool) })),
                    )
                        .into_response();
                }
                Some(_) => true,
                // Tool absent from the pool's (possibly cached) list — covers
                // on-demand sleeping servers too; genuinely unknown tools
                // answer 404 via the flag below.
                None => false,
            }
        }
        // Pool unreachable: let the call below report the real error.
        Err(_) => true,
    };
    let args = req.arguments.unwrap_or(json!({}));
    match crate::mcp::time::timeout_tool_call(pool::call_tool(&server_name, &req.tool, args)).await {
        Ok(result) => {
            // Omit structuredContent/_meta when absent — `_meta: null` is not
            // a spec-valid shape (MCP requires object when present).
            let mut body = json!({
                "result": result.content,
                "is_error": result.is_error,
            });
            if let Some(sc) = result.structured_content {
                body["structuredContent"] = sc;
            }
            if let Some(m) = result.raw_meta {
                body["_meta"] = m;
            }
            Json(body).into_response()
        }
        Err(e) => {
            let msg = e.to_string();
            let lower = msg.to_lowercase();
            // 404 only when the call error itself indicates an unknown tool:
            // `!known_enabled` alone must not force 404 — the pool's tool
            // list can be stale (on-demand sleeping server), and a real
            // failure (upstream down, timeout during wake-up) would then be
            // mislabeled Not Found instead of surfacing its true error.
            let not_found = msg.contains("[tool-not-found]")
                // Prose fallback restored: REST 404 semantics for unknown
                // tools depend on it (rmcp_gap tests assert 404). False-
                // positive risk (arbitrary upstream text containing both
                // fragments → 404 instead of 500) is acceptable vs. breaking
                // established 404 behavior.
                || (msg.contains("Tool '") && msg.contains("' not found"))
                || lower.contains("no such tool");
            let status = if not_found {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, Json(json!({ "error": msg }))).into_response()
        }
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
    // Per-server tool allow-list: group members may carry `tools` filters
    // (same semantics as the /mcp/{group} bridge surface and the spec
    // generation) — the REST listing must not leak tools the group allow-list
    // hides. Empty allow-list = fail-closed (deny all for that server).
    let group_filters = mcp_scope_server_filters(&format!("/mcp/{group_name}")).await;
    for server_name in &accessible {
        if let Ok(server_tools) = pool::list_tools_for(server_name).await {
            let mut filtered = server_tool_config_service::apply_tool_filters(server_name, server_tools)
                .await
                .unwrap_or_else(|_| vec![]);
            // Smart Routing meta tools are exclusive to the $smart access
            // points (same rule as mcp_scope_server_filters) — a group that
            // includes the builtin server must not expose them via REST.
            if server_name.as_str() == crate::rag::service::BUILTIN_SERVER_NAME {
                filtered.retain(|t| !crate::smart_routing::meta::is_meta_tool(&t.name));
            }
            if let Some(sf) = group_filters.iter().find(|f| f.name == server_name.as_str()) {
                if let Some(allowed) = &sf.tools {
                    filtered.retain(|t| allowed.iter().any(|a| a == &t.name));
                }
            }
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
            // Meta tools are exclusive to $smart — never resolvable via a
            // REST group scope, even when the group includes the builtin.
            if server_name.as_str() == crate::rag::service::BUILTIN_SERVER_NAME
                && crate::smart_routing::meta::is_meta_tool(tool_name)
            {
                continue;
            }
            if let Ok(tools) = pool::list_tools_for(server_name).await {
                if tools.iter().any(|t| &t.name == tool_name) {
                    // Enforce the group's per-server tool allow-list (parity
                    // with /mcp/{group} resolve_target): Some(list) = allow-list
                    // (empty = deny all), None = all tools.
                    if let Some(sf) = extract_server_filters(&group.servers)
                        .into_iter()
                        .find(|f| &f.name == server_name)
                    {
                        if let Some(allowed) = sf.tools {
                            if !allowed.iter().any(|t| t.as_str() == tool_name) {
                                continue;
                            }
                        }
                    }
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
        // Fail closed on filter-read failure (review round 10) — see
        // call_server_tool for the rationale.
        let filtered = match server_tool_config_service::apply_tool_filters(&server_name, tools)
            .await
        {
            Ok(f) => f,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": format!("tool gate check failed: {e}") })),
                )
                    .into_response();
            }
        };
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
    match crate::mcp::time::timeout_tool_call(pool::call_tool(&server_name, tool_name, args)).await {
        Ok(result) => {
            let mut body = json!({
                "result": result.content,
                "is_error": result.is_error,
            });
            if let Some(sc) = result.structured_content {
                body["structuredContent"] = sc;
            }
            if let Some(m) = result.raw_meta {
                body["_meta"] = m;
            }
            Json(body).into_response()
        }
        Err(e) => {
            // Same 404-on-unknown-tool semantics as call_server_tool — the
            // group path previously answered 500 for the same error shapes
            // (review round 10, surface consistency).
            let msg = e.to_string();
            let status = if tool_call_error_is_not_found(&msg) {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, Json(json!({ "error": msg }))).into_response()
        }
    }
}

/// Whether a tool-call error message indicates an unknown tool (REST 404
/// semantics). Shared by call_server_tool and call_group_tool.
fn tool_call_error_is_not_found(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    msg.contains("[tool-not-found]")
        || (msg.contains("Tool '") && msg.contains("' not found"))
        || lower.contains("no such tool")
}

// ────────────────────────────────────────────────────────────────────────────
// MCP Streamable HTTP Protocol (JSON-RPC 2.0)
// ────────────────────────────────────────────────────────────────────────────


/// Server config with optional tool/prompt/resource filters
pub(crate) struct ServerFilter {
    pub(crate) name: String,
    pub(crate) tools: Option<Vec<String>>,  // None = all tools, Some = specific tools
    pub(crate) prompts: Option<Vec<String>>,
    pub(crate) resources: Option<Vec<String>>,
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
                // Empty array = fail-closed (expose nothing), matching origin
                // mcpService (`tools !== 'all' && Array.isArray(tools)` → empty
                // allow-list). Collapsing [] to None would expose ALL tools.
                .collect::<Vec<_>>();
            Some(names)
        }
        _ => None,  // Default to all
    }
}


/// Get server filters for a scope (used for tool filtering in groups)
/// Look up the tools a server exposes, for `tools/list` + `tools/call`
/// resolution. Real (custom) servers come from the MCP pool; the RAG builtin
/// server has no pool entry, so its tools come from `rag::service` instead.
/// Returns None for a disconnected/unknown server (or RAG when disabled).
async fn tools_for_server(nm: &str) -> Option<Vec<crate::models::server::Tool>> {
    if nm == crate::rag::service::BUILTIN_SERVER_NAME {
        // HTTP (non-$smart) exposure: RAG tools only. The smart meta tools
        // are exclusive to the $smart access points — their tools/list builds
        // them dynamically via build_meta_tools, and letting them leak into
        // /mcp|/api listings would duplicate (and mis-scope) them.
        if crate::rag::service::is_enabled() {
            Some(crate::rag::service::builtin_tools())
        } else {
            None
        }
    } else {
        pool::list_tools_for(nm).await.ok()
    }
}

pub(crate) async fn mcp_scope_server_filters(scope: &str) -> Vec<ServerFilter> {
    let scope = scope.trim_start_matches('/').trim().trim_end_matches('/');
    // The RAG builtin server filter, appended to global scope (and exposed on
    // its own single-server scope) when RAG is enabled. Treated like any server
    // by the tools/list aggregation.
    let rag_filter = move || -> Option<ServerFilter> {
        // Builtin server surfaces on non-$smart HTTP paths only when RAG is
        // on (its rag_* tools). The smart meta tools never leak here — they
        // are exclusive to the $smart access points.
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
            let mut filters = extract_server_filters(&g.servers);
            // A group can include the builtin "mcphub-desktop" server, whose
            // stored member tool list was snapshotted at group-edit time and
            // may contain smart meta tool names. Non-$smart scopes must NEVER
            // expose them — drop here (single source of truth for the filter).
            if let Some(bf) = filters.iter_mut().find(|f| f.name == crate::rag::service::BUILTIN_SERVER_NAME) {
                if let Some(ref mut tools) = bf.tools {
                    tools.retain(|t| !crate::smart_routing::meta::is_meta_tool(t));
                }
            }
            return filters;
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
pub(crate) fn builtin_allowed(selection: &Option<Vec<String>>, key: &str) -> bool {
    match selection {
        None => true,
        Some(allowed) => allowed.iter().any(|s| s == key),
    }
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
pub(crate) async fn name_separator() -> String {
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
async fn collect_openapi_tools(
    scope: Option<&str>,
    include_disabled: bool,
    allowed_servers: Option<&std::collections::HashSet<String>>,
) -> Vec<OpenApiToolRef> {
    let mut sfs = mcp_scope_server_filters(scope.unwrap_or("/")).await;
    // Bearer allow-list pre-filter BEFORE any tools_for_server call: the pool
    // list cold-starts sleeping on-demand servers, so collecting tools for a
    // server this key cannot access would wake processes it must never touch
    // (review round 9 F-5).
    if let Some(allowed) = allowed_servers {
        sfs.retain(|sf| allowed.contains(&sf.name));
    }
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
                    // Pool caches BARE upstream tool names (prefixing happens
                    // at the bridge/spec boundary) — never strip a runtime
                    // prefix here: a bare name that happens to start with
                    // "{server}{sep}" (e.g. server "fs-a", tool "fs-a-read")
                    // would be mis-stripped and hidden by the allow-list.
                    allowed.contains(&t.name)
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
            // Bare name straight from the pool cache (see note above: never
            // strip a runtime prefix — bare names may legitimately start with
            // the prefix string).
            let bare_name = t.name.clone();
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
        // operationId uniqueness with suffix escalation: `{server}_{bare}`
        // disambiguation can itself collide (server `fs-a` vs `fs.a`, or a
        // bare tool literally named like the disambiguated form) — OpenAPI
        // code generators choke on duplicates (review round 10).
        let mut operation_id = if seen_operation_ids.contains(&t.bare_name) {
            format!("{}_{}", t.server.replace(['-', '.', '/'], "_"), t.bare_name)
        } else {
            t.bare_name.clone()
        };
        let mut n = 2;
        while seen_operation_ids.contains(&operation_id) {
            operation_id = format!("{operation_id}_{n}");
            n += 1;
        }
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
    let allowed_set = get_allowed_servers(bearer_key.as_ref()).await;
    let mut tools = collect_openapi_tools(scope.as_deref(), opts.include_disabled, allowed_set.as_ref()).await;
    // `?servers=a,b` filter (origin parity): restrict to the listed servers.
    if let Some(servers) = q.get("servers").filter(|s| !s.is_empty()) {
        let wanted: Vec<&str> = servers.split(',').map(|s| s.trim()).collect();
        tools.retain(|t| wanted.contains(&t.server.as_str()));
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
    let allowed_set = get_allowed_servers(bearer_key.as_ref()).await;
    let tools = collect_openapi_tools(Some(&scope), opts.include_disabled, allowed_set.as_ref()).await;
    if tools.is_empty() {
        // Distinguish 403 (this key has an allow-list and the server is not in
        // it) from 404 (unrestricted key — empty means the server doesn't
        // exist / is disconnected / exposes no tools), matching
        // /rest/{server}/tools (404 for missing) and execute_openapi_impl.
        // For GROUP scopes the allow-list contains SERVER names, never group
        // names — a direct `contains(&scope)` is always false and mislabels
        // an authorized-but-empty group as 403. Decide by intersecting the
        // group's member server names (extract_server_filters handles both
        // string[] and GroupServerConfig[] members) with the allow-list.
        let restricted = if is_group {
            // Empty-member group: `all()` on an empty iterator is vacuously
            // true — a legitimate (if empty) group must not read as 403.
            let restricted = match get_allowed_servers(bearer_key.as_ref()).await {
                Some(a) => match group.as_ref().map(|g| {
                    extract_server_filters(&g.servers)
                        .into_iter()
                        .map(|f| f.name)
                        .collect::<Vec<String>>()
                }) {
                    Some(members) if members.is_empty() => false,
                    Some(members) => members.iter().all(|m| !a.contains(m)),
                    None => false,
                },
                None => false,
            };
            restricted
        } else {
            get_allowed_servers(bearer_key.as_ref())
                .await
                .map(|a| !a.contains(&scope))
                .unwrap_or(false)
        };
        if restricted {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({
                    "error": "Access denied",
                    "message": format!("Server '{name}' is not exposed to this key"),
                })),
            )
                .into_response();
        }
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": "Not found",
                "message": format!("Server '{name}' not found or exposes no OpenAPI tools"),
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
        // Sleeping on-demand servers are included so the discovery list
        // matches what the spec endpoints expose (their cached tools are
        // listed, and a fetch wakes them).
        .filter(|s| s.connected || s.start_on_demand)
        .map(|s| s.name)
        .collect();
    if let Some(allowed) = get_allowed_servers(bearer_key.as_ref()).await {
        names.retain(|n| allowed.contains(n));
    }
    // The spec endpoints include the RAG builtin server's tools — the
    // discovery lists must match or clients can't see what they can call.
    if crate::rag::service::is_enabled() {
        let builtin_ok = get_allowed_servers(bearer_key.as_ref())
            .await
            .map(|a| a.contains(crate::rag::service::BUILTIN_SERVER_NAME))
            .unwrap_or(true);
        if builtin_ok && !names.iter().any(|n| n == crate::rag::service::BUILTIN_SERVER_NAME) {
            names.push(crate::rag::service::BUILTIN_SERVER_NAME.to_string());
        }
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
    let mut total_tools: usize = statuses.iter().filter(|s| s.connected || s.start_on_demand).map(|s| s.tool_count).sum();
    let mut breakdown: Vec<serde_json::Value> = statuses
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
    // Spec endpoints expose the RAG builtin server — keep stats in sync.
    if crate::rag::service::is_enabled() {
        let builtin_ok = get_allowed_servers(bearer_key.as_ref())
            .await
            .map(|a| a.contains(crate::rag::service::BUILTIN_SERVER_NAME))
            .unwrap_or(true);
        if builtin_ok {
            let n = crate::rag::service::builtin_server_tools().await.len();
            total_tools += n;
            breakdown.push(json!({
                "name": crate::rag::service::BUILTIN_SERVER_NAME,
                "toolCount": n,
                "status": "connected",
            }));
        }
    }
    Json(json!({
        "success": true,
        "data": {
            "totalServers": statuses.iter().filter(|s| s.connected || s.start_on_demand).count(),
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
    // Meta tools are exclusive to $smart — never resolvable via the plain
    // single-server REST call path even when the target is the builtin.
    if server_name == crate::rag::service::BUILTIN_SERVER_NAME
        && crate::smart_routing::meta::is_meta_tool(&tool_name)
    {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("Tool '{tool_name}' not found on server '{server_name}'") })),
        )
            .into_response();
    }
    // Scoped variant: verify the server is actually reachable under the scope
    // (a group's server list, or the named server itself) — AND enforce the
    // scope's tool allow-list here: /api/{group}/tools/{server}/{tool} must
    // not reach tools the group allow-list hides (parity with the /mcp/{group}
    // resolve_target gate and the spec-generation filter).
    if let Some(ref sc) = scope {
        let sfs = mcp_scope_server_filters(sc).await;
        let sf = sfs.iter().find(|f| f.name == server_name);
        if sf.is_none() {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": format!("Server '{server_name}' not found in scope '{sc}'") })),
            )
                .into_response();
        }
        if let Some(sf) = sf {
            // sf.tools is Some(allow-list); empty = deny all (M7 fail-closed).
            if let Some(allowed) = &sf.tools {
                // Pool names are bare (see collect_openapi_tools note) —
                // compare as-is, no runtime-prefix stripping.
                if !allowed.iter().any(|t| *t == tool_name) {
                    return (
                        StatusCode::FORBIDDEN,
                        Json(json!({ "error": format!("Tool '{tool_name}' is not allowed by scope '{sc}'") })),
                    )
                        .into_response();
                }
            }
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
    // Fail closed on filter-read failure (review round 10).
    let filtered = match server_tool_config_service::apply_tool_filters(&server_name, tools).await
    {
        Ok(f) => f,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("tool gate check failed: {e}") })),
            )
                .into_response();
        }
    };
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
    let result = crate::mcp::time::timeout_tool_call(pool::call_tool(&server_name, &actual_name, args.clone())).await;
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
            let mut body = json!({
                "content": r.content,
                "isError": r.is_error,
            });
            if let Some(sc) = r.structured_content {
                body["structuredContent"] = sc;
            }
            if let Some(m) = r.raw_meta {
                body["_meta"] = m;
            }
            Json(body).into_response()
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

pub(crate) fn client_ip_of(headers: &HeaderMap) -> Option<String> {
    // XFF/X-Real-IP are client-spoofable. Only honor them when the operator
    // opted in via TRUST_PROXY (the env var previously only decorated the
    // startup log). There is NO socket-peer fallback: the router is not built
    // with `into_make_service_with_connect_info`, so without a trusted proxy
    // header the source_ip stays None (spoofing-proof by construction).
    // Header-based spoofing must not poison activity_log.source_ip.
    let trust_proxy = std::env::var("TRUST_PROXY")
        .map(|v| {
            let v = v.to_lowercase();
            v == "true" || v == "1" || v == "yes"
        })
        .unwrap_or(false);
    if trust_proxy {
        if let Some(ip) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split(',').next())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            return Some(ip);
        }
        if let Some(ip) = headers
            .get("x-real-ip")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
        {
            return Some(ip);
        }
    }
    None
}

async fn openapi_exec_global_get(
    headers: HeaderMap,
    Path((server, tool)): Path<(String, String)>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let ip = client_ip_of(&headers);
    // Bearer check FIRST: pool::list_tools_for below can COLD-START a
    // sleeping on-demand stdio server (spawning its child process). Doing
    // that before auth would let unauthenticated requests churn gateway
    // child processes (resource exhaustion) even with bearer auth enabled.
    // execute_openapi_impl re-checks; this early check is fail-closed too.
    // Allow-list pre-check BEFORE list_tools_for: a valid-but-restricted key
    // must not cold-start servers outside its allow-list (review round 10,
    // same rationale as the collect_openapi_tools prefilter). execute_openapi_impl
    // re-checks authoritatively.
    let bearer_key_pre = match check_bearer_auth(&headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    if let Some(allowed) = get_allowed_servers(bearer_key_pre.as_ref()).await {
        if !allowed.contains(&server) {
            return (
                axum::http::StatusCode::FORBIDDEN,
                axum::Json(json!({ "error": "Access denied for this server" })),
            )
                .into_response();
        }
    }
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
    // Same early bearer gate as openapi_exec_global_get (cold-start guard),
    // plus the allow-list pre-check (review round 10).
    let bearer_key_pre = match check_bearer_auth(&headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    if let Some(allowed) = get_allowed_servers(bearer_key_pre.as_ref()).await {
        if !allowed.contains(&server) {
            return (
                axum::http::StatusCode::FORBIDDEN,
                axum::Json(json!({ "error": "Access denied for this server" })),
            )
                .into_response();
        }
    }
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

/// Shared rmcp `StreamableHttpService` factory for the `/mcp` access point.
/// One `LocalSessionManager` per mount is intentional: sessions do not span
/// scopes (each `/mcp/{scope}` path is an independent endpoint for clients).
fn rmcp_service(
    sdk_cancel: tokio_util::sync::CancellationToken,
) -> rmcp::transport::streamable_http_server::tower::StreamableHttpService<
    super::rmcp_bridge::HubBridge,
    rmcp::transport::streamable_http_server::session::local::LocalSessionManager,
> {
    let mut config = rmcp::transport::streamable_http_server::tower::StreamableHttpServerConfig::default();
    // Simple request-response calls return `application/json` instead of an
    // SSE stream: simplified clients (e.g. codemoss-ide) JSON.parse the body
    // and choke on the SSE keepalive frame ("data: \nid:"). Calls carrying a
    // progressToken still fall back to SSE (rmcp built-in behaviour).
    config.json_response = true;
    // The hub binds 0.0.0.0 and serves LAN/remote MCP clients (bearer keys
    // gate access). The SDK's default allowed_hosts (loopback only) 403s any
    // non-loopback Host header — empirically verified — which breaks every
    // remote client before authentication. Host validation is a browser
    // DNS-rebinding defence; MCP clients are not browsers and the endpoint is
    // authenticated, so disable it (parity with origin's Express, which never
    // validated Host).
    let mut config = config.disable_allowed_hosts();
    // Align with the leniency middleware's parse cap (8MB floor): the SDK
    // default is 4MiB, which would make 4–8MB POSTs fully buffer + (re)parse
    // in the middleware only to be rejected 413 by the SDK afterwards. Read
    // the SAME snapshot the leniency middleware uses so a user-configured
    // limit above 64MB is not buffered by the middleware then 413'd here.
    config.max_request_body_bytes =
        std::sync::atomic::AtomicUsize::load(&BODY_LIMIT_SNAPSHOT, std::sync::atomic::Ordering::Relaxed)
            .max(64 * 1024 * 1024);
    config.cancellation_token = sdk_cancel;
    rmcp::transport::streamable_http_server::tower::StreamableHttpService::new(
        || Ok(super::rmcp_bridge::HubBridge::new()),
        shared_session_manager().clone(),
        config,
    )
}

/// System switch: strict protocol validation for `/mcp` requests.
/// `true` = rmcp rejects malformed headers/_meta (spec-exact behaviour);
/// `false` (default) = the leniency middleware normalizes legacy-client
/// requests before they reach rmcp (many MCP clients lag the new specs).
/// Version used to upgrade session-less bare requests in leniency mode.
/// Deliberately a legacy version: the upgraded request keeps legacy semantics
/// (no CacheableResult) in the bridge gating.
const UPGRADE_VERSION: &str = "2025-11-25";
const PV_KEY: &str = "io.modelcontextprotocol/protocolVersion";

/// The registered protocol versions (mirrors rmcp `ProtocolVersion::KNOWN_VERSIONS`).
const KNOWN_PROTOCOL_VERSIONS: [&str; 5] = [
    "2024-11-05",
    "2025-03-26",
    "2025-06-18",
    "2025-11-25",
    "2026-07-28",
];

fn is_known_protocol_version(v: &str) -> bool {
    KNOWN_PROTOCOL_VERSIONS.contains(&v)
}

pub(crate) async fn is_mcp_strict_validation_enabled() -> bool {
    crate::services::config_service::get()
        .await
        .ok()
        .and_then(|c| c.get("mcp").and_then(|m| m.get("strictValidation")).and_then(|v| v.as_bool()))
        .unwrap_or(false)
}

/// Leniency normalization for POST requests (strict mode OFF only).
/// - Accept header missing either of the required media types -> add it
///   (rmcp otherwise answers 406 before even reading the body).
/// - `MCP-Protocol-Version >= 2026` with `_meta` missing the required
///   client metadata -> inject minimal defaults (rmcp answers -32602
///   otherwise; older clients legitimately send bare protocolVersion).
/// - `_meta.protocolVersion` without the header -> add the header.
/// - Invalid-encoding version header -> strip instead of 400.
async fn mcp_leniency_middleware(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    // Path-gated to /mcp*: this middleware rewrites JSON-RPC envelope noise
    // (injects `jsonrpc`/`params`/`_meta` keys into the body). Applied to the
    // whole router it used to inject a bogus `jsonrpc` key into REST tool
    // bodies (/api/tools/... passes the body verbatim as tool arguments) and
    // force-rewrite Accept on unrelated GETs.
    {
        let p = req.uri().path();
        if p != "/mcp" && !p.starts_with("/mcp/") {
            return next.run(req).await;
        }
    }
    if req.method() == axum::http::Method::GET {
        // Lenient GET (SSE stream open): a missing/deficient Accept header
        // otherwise answers rmcp's spec-correct 406 — the stream open does
        // not affect tool calls, so supply the media type the endpoint needs.
        let strict = is_mcp_strict_validation_enabled().await;
        if !strict {
            let mut sse_ok = false;
            for v in req.headers().get_all(axum::http::header::ACCEPT) {
                if let Ok(s) = v.to_str() {
                    if s.to_ascii_lowercase().contains("text/event-stream") {
                        sse_ok = true;
                    }
                }
            }
            let mut req = req;
            if !sse_ok {
                req.headers_mut().insert(
                    axum::http::header::ACCEPT,
                    axum::http::HeaderValue::from_static("text/event-stream"),
                );
            }
            return next.run(req).await;
        }
        return next.run(req).await;
    }
    if req.method() != axum::http::Method::POST {
        return next.run(req).await;
    }
    let strict = is_mcp_strict_validation_enabled().await;
    // Accept-header normalization (header-level, no body parse needed).
    // Strict mode keeps rmcp's spec-correct 406 for bad Accept headers.
    let mut accept_ok = false;
    for v in req.headers().get_all(axum::http::header::ACCEPT) {
        if let Ok(s) = v.to_str() {
            let sl = s.to_ascii_lowercase();
            if sl.contains("application/json") && sl.contains("text/event-stream") {
                accept_ok = true;
            }
        }
    }
    let mut req = req;
    if !strict {
        if !accept_ok {
            req.headers_mut().insert(
                axum::http::header::ACCEPT,
                axum::http::HeaderValue::from_static("application/json, text/event-stream"),
            );
        }
        // Missing Content-Type hits axum's 415 before any handler runs; a
        // JSON-RPC POST without it is still unambiguously JSON — supply it.
        if req.headers().get(axum::http::header::CONTENT_TYPE).is_none() {
            req.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderValue::from_static("application/json"),
            );
        }
    }

    // Body-level normalization: parse JSON-RPC body, patch _meta / headers.
    // Session-less non-initialize requests also need the upgrade path
    // (rmcp answers 422 "expect initialize request" for them otherwise).
    // Parsing always runs (bounded) because the SEP-2663 task marker below
    // must be surfaced in BOTH modes (strict mode only skips the fixes).
    let (mut parts, body) = req.into_parts();
    // Parse bound: the router's configured jsonBodyLimit is the real cap
    // (DefaultBodyLimit) — it may exceed 8MB, so the middleware must never
    // reject/lower a body the router would accept. Read the same config with
    // an 8MB floor and a sanity ceiling; on a read failure fall back to
    // 8MB (previous behavior). The ceiling must never be BELOW the router's
    // DefaultBodyLimit for the same config — otherwise the middleware would
    // 413 bodies the router itself would accept (review round 9 F-4: the old
    // hardcoded 64MB ceiling rejected user-configured limits above 64MB).
    let parse_cap = {
        let cfg_val = crate::services::config_service::get().await.ok();
        let parsed = cfg_val
            .as_ref()
            .and_then(|c| c.get("routing").and_then(|r| r.get("jsonBodyLimit")).and_then(|v| v.as_str()))
            .map(parse_body_limit);
        let router_limit =
            std::sync::atomic::AtomicUsize::load(&BODY_LIMIT_SNAPSHOT, std::sync::atomic::Ordering::Relaxed);
        parsed
            .unwrap_or(8 * 1024 * 1024)
            .clamp(8 * 1024 * 1024, (64 * 1024 * 1024).max(router_limit))
    };
    let bytes = match axum::body::to_bytes(body, parse_cap).await {
        Ok(b) => b,
        Err(_) => {
            // Over the limit: the body has already been consumed by this read
            // attempt, so the router's DefaultBodyLimit can no longer produce
            // its (accurate) 413 — answering directly instead of forwarding an
            // empty body that would only surface as a confusing 400 JSON
            // parse error (review round 8, 2026-10-04).
            let res = axum::http::Response::builder()
                .status(axum::http::StatusCode::PAYLOAD_TOO_LARGE)
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(
                    "{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32700,\"message\":\"Payload too large\"}}",
                ))
                .unwrap();
            return res;
        }
    };
    // Strip any client-supplied task marker BEFORE parsing: the bridge must
    // only ever see a value derived from the request body, not one a client
    // forged directly into a header — including the non-JSON early-return
    // path below, which would otherwise pass the forged header through.
    parts.headers.remove("x-mcphub-task-requested");
    let mut value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => {
            // Not JSON — pass through untouched (rmcp will handle the error).
            let req = axum::extract::Request::from_parts(
                parts,
                axum::body::Body::from(bytes),
            );
            return next.run(req).await;
        }
    };
    let mut changed = false;

    // SEP-2663 task marker (BOTH modes): rmcp's typed CallToolRequestParams
    // has no `task` field, so a client-directed `params.task` would be
    // silently dropped and the call executed synchronously. Surface it as an
    // internal header the bridge reads to materialize a task instead.
    if value.get("method").and_then(|m| m.as_str()) == Some("tools/call") {
        if let Some(task_obj) = value.pointer("/params/task").filter(|t| t.is_object()) {
            if let Ok(hv) = axum::http::HeaderValue::from_str(&task_obj.to_string()) {
                parts.headers.insert("x-mcphub-task-requested", hv);
                changed = true;
            }
        }
    }

    // Strict mode: no leniency fixes — rmcp enforces the spec natively. The
    // task marker above is a lossless translation, not a validation bypass.
    if strict {
        return next.run(into_request(parts, &bytes, &value, changed)).await;
    }

    let header_version = {
        // Strip invalid/unknown version headers FIRST (leniency): rmcp answers
        // 400 "Unsupported MCP-Protocol-Version" for unknown values, yet the
        // header adds nothing a session handshake has not already negotiated.
        // Applies to the whole request (the early-return branch below also
        // benefits). Invalid ENCODING is stripped here too.
        let raw = parts.headers.get("mcp-protocol-version").cloned();
        let invalid = raw
            .as_ref()
            .map(|v| {
                v.to_str()
                    .map(|s| !is_known_protocol_version(s))
                    .unwrap_or(true)
            })
            .unwrap_or(false);
        if invalid {
            parts.headers.remove("mcp-protocol-version");
            changed = true;
            None
        } else {
            raw.map(|v| v.to_str().ok().map(|s| s.to_string())).flatten()
        }
    };
    let has_session = parts
        .headers
        .get("mcp-session-id")
        .map(|v| !v.is_empty())
        .unwrap_or(false);

    let req_method = value.get("method").and_then(|m| m.as_str()).map(|s| s.to_string());
    let has_id = value.get("id").is_some();
    // Lenient normalization of `jsonrpc` envelope noise: a missing or wrong
    // version field has zero effect on tool-call semantics but rmcp's typed
    // deserializer rejects the whole body (415). Normalize single objects.
    // Batch arrays are removed since 2025-11-25 — left untouched.
    if let Some(obj) = value.as_object_mut() {
        let jr = obj.get("jsonrpc").and_then(|v| v.as_str());
        if jr != Some("2.0") {
            obj.insert("jsonrpc".to_string(), json!("2.0"));
            changed = true;
        }
    }
    // Lenient normalization of `jsonrpc` envelope noise: a missing or wrong
    // A JSON-RPC request may legally omit `params`; rmcp answers 422 for a
    // session-less non-initialize call though. Materialize an empty params
    // object only when the bare-upgrade path below would otherwise fire.
    if let Some(obj) = value.as_object_mut() {
        let params_missing_or_not_obj = !obj.get("params").map(|p| p.is_object()).unwrap_or(false);
        let is_initialize = obj.get("method").and_then(|m| m.as_str()) == Some("initialize");
        if params_missing_or_not_obj && has_id && !is_initialize && !has_session {
            obj.insert("params".to_string(), json!({}));
            changed = true;
        }
    }
    // (See bare-upgrade below for why.) Rewrite done before `params` borrows
    // `value` mutably.
    let id_was_null = value
        .get("id")
        .map(|v| v.is_null())
        .unwrap_or(false);
    // Rewrite BEFORE `params` borrows `value`: rmcp deserializes `id: null`
    // as a Notification (serde Option<id> + None) and rejects notifications
    // outside an initialized session — so a lenient request with `"id": null`
    // would 422 no matter what _meta we inject. Rewrite null → 0 so the call
    // is processable (lenient philosophy: don't block a usable tool call on
    // an id the client didn't care about). Applies to any session-less
    // non-initialize request regardless of _meta state, since rmcp treats
    // id=null as a notification in all of them.
    if id_was_null && !has_session {
        let is_initialize = value.get("method").and_then(|m| m.as_str()) == Some("initialize");
        if !is_initialize {
            if let Some(obj) = value.as_object_mut() {
                obj.insert("id".to_string(), json!(0));
                changed = true;
            }
        }
    }
    // Blank mcp-session-id strip — pure header op, applied to EVERY request
    // shape (notifications without params included): rmcp takes the session
    // branch whenever the header exists, and restore("") is always 404.
    // Must run before the params-dependent section below so a params-less
    // notification also lands on the stateless/session-negotiated path.
    if let Some(empty_hdr) = parts.headers.get("mcp-session-id").cloned() {
        if empty_hdr.is_empty() {
            parts.headers.remove("mcp-session-id");
            changed = true;
        }
    }
    if let Some(params) = value.get_mut("params").and_then(|p| p.as_object_mut()) {
        // Only materialize _meta when an injection will actually happen;
        // otherwise legacy bodies would gain a pointless empty _meta.
        // Lexicographic `>=` alone misjudges "9999-01-01" as modern — require
        // membership in the known version set for every gate below.
        let header_ge_2026 = header_version
            .as_deref()
            .map(|v| !v.is_empty() && is_known_protocol_version(v) && v >= "2026-07-28")
            .unwrap_or(false);
        let header_missing = header_version.is_none();
        let existing_meta_has_pv = params
            .get("_meta")
            .and_then(|m| m.get("io.modelcontextprotocol/protocolVersion"))
            .and_then(|v| v.as_str())
            .is_some();
        // Stateless upgrade: a session-less Request (initialize excluded) would
        // hit rmcp's 422 "expect initialize request". Any client that skips the
        // handshake just wants a tool call — upgrade it to a modern stateless
        // request on the negotiated path. Use the legacy version so the
        // CacheableResult gating still treats the session as legacy.
        let is_initialize = req_method.as_deref() == Some("initialize");
        // Bare = no session, has id, not initialize, and no negotiated version
        // in _meta. An existing `_meta` without a protocolVersion (e.g. `{}`,
        // or carrying only progressToken) still counts as bare — the upgrade
        // only inserts the missing version/metadata keys.
        let is_bare_request = !has_session && has_id && !is_initialize && !existing_meta_has_pv;
        // Bare upgrade version: a client that DID send a known >= 2026 header
        // but omitted _meta should keep its declared version — downgrading it
        // to the legacy UPGRADE_VERSION would silently switch it to legacy
        // semantics (no ttlMs/cacheScope, no SEP-2322 resultType channel).
        let upgrade_version: String = if header_ge_2026 {
            header_version.clone().unwrap_or_else(|| UPGRADE_VERSION.to_string())
        } else {
            UPGRADE_VERSION.to_string()
        };
        if is_bare_request {
            // `_meta` may be a non-object (e.g. a string) from a sloppy
            // client; overwrite rather than unwrap — the bare upgrade owns
            // the metadata shape.
            if !params.get("_meta").map(|v| v.is_object()).unwrap_or(false) {
                params.insert("_meta".to_string(), serde_json::json!({}));
                changed = true;
            }
            let m = params
                .entry("_meta")
                .or_insert_with(|| {
                    changed = true;
                    serde_json::json!({})
                })
                .as_object_mut()
                .expect("_meta guaranteed object above");
            if !existing_meta_has_pv {
                m.insert(PV_KEY.to_string(), json!(upgrade_version));
                changed = true;
            }
            m.entry("io.modelcontextprotocol/clientInfo".to_string())
                .or_insert_with(|| {
                    changed = true;
                    serde_json::json!({"name":"unknown-client","version":"0.0.0"})
                });
            m.entry("io.modelcontextprotocol/clientCapabilities".to_string())
                .or_insert_with(|| {
                    changed = true;
                    serde_json::json!({})
                });
            // The injected _meta version must match the header (rmcp
            // header-mismatch check): override any older header too.
            {
                let mismatch = parts
                    .headers
                    .get("mcp-protocol-version")
                    .and_then(|v| v.to_str().ok())
                    .map(|v| v != upgrade_version)
                    .unwrap_or(true);
                if mismatch {
                    if let Ok(hv) = axum::http::HeaderValue::from_str(&upgrade_version) {
                        parts.headers.insert("mcp-protocol-version", hv);
                        changed = true;
                    }
                }
            }
        } else if !header_ge_2026 && !(header_missing && existing_meta_has_pv) {
            return next.run(into_request(parts, &bytes, &value, changed)).await;
        }
        let meta = params.entry("_meta").or_insert_with(|| {
            changed = true;
            serde_json::json!({})
        });
        // Non-object `_meta` (e.g. a bare string) with a 2026 header would
        // otherwise skip every injection below (as_object_mut() == None) and
        // hit rmcp as an unparseable request — normalize it first, same as
        // the bare branch.
        if !meta.is_object() {
            *meta = serde_json::json!({});
            changed = true;
        }
        if let Some(m) = meta.as_object_mut() {
            // Header >= 2026 without full client metadata: inject defaults.
            if header_version.as_deref().unwrap_or("").chars().all(|c| c.is_ascii()) {
                let needs = header_version
                    .as_deref()
                    .map(|v| !v.is_empty() && v >= "2026-07-28")
                    .unwrap_or(false);
                if needs {
                    // rmcp's modern path validates _meta.protocolVersion too —
                    // a session'd legacy client declaring the 2026 header with
                    // no/empty _meta otherwise 400s ("missing protocolVersion").
                    // Echo the header into _meta so the declaration is coherent.
                    if !m.contains_key("io.modelcontextprotocol/protocolVersion") {
                        if let Some(hv) = header_version.as_deref() {
                            if is_known_protocol_version(hv) {
                                m.insert(
                                    "io.modelcontextprotocol/protocolVersion".to_string(),
                                    serde_json::json!(hv),
                                );
                                changed = true;
                            }
                        }
                    }
                    for (k, dv) in [
                        ("io.modelcontextprotocol/clientInfo",
                         serde_json::json!({"name":"unknown-client","version":"0.0.0"})),
                        ("io.modelcontextprotocol/clientCapabilities", serde_json::json!({})),
                    ] {
                        if !m.contains_key(k) {
                            m.insert(k.to_string(), dv);
                            changed = true;
                        }
                    }
                }
            }
            // _meta.protocolVersion present but header missing -> inject header.
            // Inject the meta value VERBATIM (even when unknown): rmcp's modern
            // path requires header == _meta.protocolVersion and then produces
            // the structured -32022 UnsupportedProtocolVersionError for unknown
            // versions. Withholding the injection here would instead yield a
            // plain 400 "requires MCP-Protocol-Version header".
            if header_version.is_none() {
                if let Some(pv) = m
                    .get("io.modelcontextprotocol/protocolVersion")
                    .and_then(|v| v.as_str())
                {
                    if let Ok(hv) = axum::http::HeaderValue::from_str(pv) {
                        parts.headers.insert("mcp-protocol-version", hv);
                        changed = true;
                    }
                    // A _meta-declared >= 2026 version requires the full
                    // modern request metadata (protocolVersion AND
                    // clientCapabilities) — rmcp rejects with -32602
                    // otherwise. Inject the same defaults the header-driven
                    // path above uses.
                    if is_known_protocol_version(pv) && pv >= "2026-07-28" {
                        for (k, dv) in [
                            ("io.modelcontextprotocol/clientInfo",
                             serde_json::json!({"name":"unknown-client","version":"0.0.0"})),
                            ("io.modelcontextprotocol/clientCapabilities", serde_json::json!({})),
                        ] {
                            if !m.contains_key(k) {
                                m.insert(k.to_string(), dv);
                                changed = true;
                            }
                        }
                    }
                }
            }
            // SEP-2243 (protocol >= 2025-06-18): Mcp-Method/Mcp-Name required —
            // derive them from the body for clients that omit them. Evaluate
            // AFTER header injection so a _meta-derived version also applies.
            // Gate on "header exists" rather than version range: unknown
            // header values (e.g. a _meta-derived "2099-01-01") must still
            // carry the derived headers or rmcp's SEP-2243 check answers a
            // plain 400 before the structured -32022 version error.
            let needs_sep2243 = parts
                .headers
                .get("mcp-protocol-version")
                .and_then(|v| v.to_str().ok())
                .map(|v| !v.is_empty())
                .unwrap_or(false);
            if needs_sep2243 {
                let method = value.get("method").and_then(|m| m.as_str()).unwrap_or("");
                if !method.is_empty() {
                    // Lenient philosophy = "trust the body": a client may carry
                    // a STALE/WRONG Mcp-Method from a previous request — rmcp
                    // validates it against the body and answers 400. Overwrite
                    // whenever it differs from the body-derived value.
                    let method_header_wrong = parts
                        .headers
                        .get("mcp-method")
                        .and_then(|v| v.to_str().ok())
                        .map(|existing| existing != method)
                        .unwrap_or(true);
                    if method_header_wrong {
                        if let Ok(hv) = axum::http::HeaderValue::from_str(method) {
                            parts.headers.insert("mcp-method", hv);
                            changed = true;
                        }
                    }
                    // SEP-2243 names (mirror rmcp mcp_headers.rs tables):
                    // NAME_FROM_NAME = tools/call | prompts/get → params.name;
                    // NAME_FROM_URI = resources/read|subscribe|unsubscribe → params.uri;
                    // tasks/* → params.taskId. Without Mcp-Name the SDK rejects
                    // these methods with 400 when the request declares >= 2026-07-28
                    // (tower.rs validate_standard_headers gate = STANDARD_HEADERS).
                    let name_source = if method == "tools/call" || method == "prompts/get" {
                        value
                            .get("params")
                            .and_then(|p| p.get("name"))
                            .and_then(|n| n.as_str())
                    } else if method == "resources/read"
                        || method == "resources/subscribe"
                        || method == "resources/unsubscribe"
                    {
                        value
                            .get("params")
                            .and_then(|p| p.get("uri"))
                            .and_then(|n| n.as_str())
                    } else if method.starts_with("tasks/") {
                        // NOTE: an earlier draft had a duplicate `tasks/`
                        // branch reading `params.name` first, which made this
                        // `params.taskId` branch unreachable — tasks/get and
                        // tasks/cancel (whose params carry taskId, not name)
                        // then missed the Mcp-Name injection and the SDK
                        // rejected them with 400. (Found by R-round E2E.)
                        value
                            .get("params")
                            .and_then(|p| p.get("taskId"))
                            .and_then(|n| n.as_str())
                    } else {
                        None
                    };
                    if let Some(tn) = name_source.filter(|s| !s.is_empty()) {
                        // rmcp validates Mcp-Name against params.name; non-ASCII
                        // values cannot travel as raw header bytes but rmcp
                        // natively accepts the `=?base64?<b64>?=` wrapper
                        // (see rmcp mcp_headers::encode_header_value).
                        // Same "trust the body" rule as Mcp-Method: a stale or
                        // wrong Mcp-Name is overwritten with the derived value.
                        let hv = if tn.is_ascii() {
                            axum::http::HeaderValue::from_str(tn).ok()
                        } else {
                            use base64::Engine as _;
                            axum::http::HeaderValue::from_str(&format!(
                                "=?base64?{}?=",
                                base64::engine::general_purpose::STANDARD.encode(tn)
                            ))
                            .ok()
                        };
                        if let Some(hv) = hv {
                            let wrong_or_missing = parts
                                .headers
                                .get("mcp-name")
                                .map(|existing| existing != &hv)
                                .unwrap_or(true);
                            if wrong_or_missing {
                                parts.headers.insert("mcp-name", hv);
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
    }

    // (Unknown/invalid-encoding version headers were already stripped above.)

    if !changed {
        return next.run(into_request(parts, &bytes, &value, false)).await;
    }
    next.run(into_request(parts, &bytes, &value, true)).await
}

/// Reassemble a request from parsed parts; when anything changed, the body is
/// re-serialized (with a corrected Content-Length), otherwise the original
/// bytes pass through untouched.
fn into_request(
    parts: axum::http::request::Parts,
    bytes: &[u8],
    value: &serde_json::Value,
    changed: bool,
) -> axum::extract::Request {
    if !changed {
        return axum::extract::Request::from_parts(parts, axum::body::Body::from(bytes.to_vec()));
    }
    let new_bytes = serde_json::to_vec(value).unwrap_or_else(|_| bytes.to_vec());
    let len = new_bytes.len();
    let mut req = axum::extract::Request::from_parts(parts, axum::body::Body::from(new_bytes));
    if let Ok(l) = len.to_string().parse() {
        req.headers_mut().insert(axum::http::header::CONTENT_LENGTH, l);
    }
    req
}

/// Pre-extraction bearer gate for POST bodies on `/rest*` and `/api*`. Axum
/// runs Json extractors (buffering up to 64MB) BEFORE the handler body, so
/// the in-handler `check_bearer_auth` ran after the memory was spent — N
/// unauthenticated POSTs could buffer N×64MB. Gate POSTs at the middleware
/// layer like `/mcp*`; GETs keep their in-handler pre-checks.
async fn rest_post_bearer_middleware(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if req.method() != axum::http::Method::POST {
        return next.run(req).await;
    }
    let path = req.uri().path();
    if !path.starts_with("/rest/") && !path.starts_with("/api/") {
        return next.run(req).await;
    }
    let headers = req.headers().clone();
    match check_bearer_auth(&headers).await {
        Ok(_) => next.run(req).await,
        Err(resp) => resp,
    }
}

/// Bearer-key gate for the rmcp `/mcp` routes. The rmcp service bypasses our
/// per-request dispatch, so auth must be enforced here — otherwise
/// `enableBearerAuth` would not protect `initialize`/`ping`/`notifications`.
/// Mirrors the old `dispatch_mcp` behaviour: disabled -> pass through; enabled
/// -> missing/invalid token gets the OAuth-style 401 response.
///
/// Path-gated to `/mcp*` only: `Router::layer` applies to every route in the
/// router, and all other endpoints (`/health`, `/rest/*`, `/api/*`,
/// `/.well-known/*`) either self-gate via `check_bearer_auth` or are
/// deliberately public (health probe, OAuth metadata). Gating them here too
/// broke `loopback_ok()` (GET /health without a token → 401 → false-positive
/// loopback-hijack dialogs on every startup/watch tick).
async fn mcp_bearer_middleware(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let path = req.uri().path();
    if path != "/mcp" && !path.starts_with("/mcp/") {
        return next.run(req).await;
    }
    let headers = req.headers().clone();
    match check_bearer_auth(&headers).await {
        Ok(_) => next.run(req).await,
        Err(resp) => resp,
    }
}

/// Restores the two pre-rmcp exposure switches that the SDK migration dropped
/// (`routing.enableGlobalRoute` / `routing.enableGroupNameRoute`): root `/mcp`
/// (and `$smart`) answers 404 when the global route is disabled, group-name
/// scopes answer 404 when group routes are disabled. Semantics copied from the
/// retired `dispatch_mcp` (commit 47fcde8^) — 404 body included.
async fn mcp_route_gate_middleware(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let path = req.uri().path().split('?').next().unwrap_or("");
    let mcp_path = path == "/mcp" || path.starts_with("/mcp/");
    if mcp_path {
        // strip_prefix (not trim_start_matches) so "/mcp/mcp" is not double-
        // stripped — mirrors rmcp_bridge::scope_from_ctx exactly. Decode the
        // whole remainder first: "%24smart" / "$smart%2Fgroup" must classify
        // the same as their decoded forms (R118).
        let remainder = path.strip_prefix("/mcp").unwrap_or("").trim_start_matches('/');
        let mut scope_clean = percent_encoding::percent_decode_str(remainder.trim())
            .decode_utf8_lossy()
            .into_owned();
        // Trailing slashes must be trimmed here too: bridge scope_from_ctx
        // normalizes `$smart/` -> `$smart` and serves the full global surface,
        // so an untrimmed `$smart/` would skip the enableGlobalRoute check.
        scope_clean = scope_clean.trim_end_matches('/').to_string();
        let is_global_scope = scope_clean.is_empty() || scope_clean == "$smart";
        // Read failure must FAIL CLOSED (404) like check_bearer_auth — a
        // transient DB error must not re-expose scopes the operator disabled.
        let config = match config_service::get().await {
            Ok(c) => Some(c),
            Err(e) => {
                log::warn!("[http] config read failed during route gate: {e}; failing closed (404)");
                None
            }
        };
        if config.is_none() {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "Route gate unavailable"})),
            )
                .into_response();
        }
        let routing_flag = |key: &str, default: bool| -> bool {
            config
                .as_ref()
                .and_then(|c| c.get("routing"))
                .and_then(|r| r.get(key))
                .and_then(|v| v.as_bool())
                .unwrap_or(default)
        };
        if is_global_scope {
            if !routing_flag("enableGlobalRoute", true) {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error": "Global route is disabled"})),
                )
                    .into_response();
            }
        } else {
            // scope_clean is already percent-decoded above (R118 alignment).
            // Trim trailing slashes to match mcp_scope_server_filters — the
            // bridge tolerates `GroupA/`, and an untrimmed name here would
            // skip the enableGroupNameRoute check for that form.
            let decoded_name = scope_clean
                .strip_prefix("$smart/")
                .unwrap_or(&scope_clean)
                .trim()
                .trim_end_matches('/')
                .to_string();
            // Only the *name* match is gated by enableGroupNameRoute (matches
            // origin getGroupByIdOrName semantics); id access stays available.
            let is_group_by_name = group_service::list_all()
                .await
                .map(|gs| gs.iter().any(|g| g.name == decoded_name))
                .unwrap_or(false);
            if is_group_by_name && !routing_flag("enableGroupNameRoute", true) {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error": "Group name route is disabled"})),
                )
                    .into_response();
            }
        }
    }
    next.run(req).await
}

fn build_router(body_limit_bytes: usize, sdk_cancel: tokio_util::sync::CancellationToken) -> Router {
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
        // Smart Routing REST surface (origin /api/$smart parity): the meta
        // tools as plain REST endpoints + their OpenAPI spec (for OpenWebUI
        // and friends). Literal routes win over the {name} params.
        .route("/api/$smart/openapi.json", get(smart_openapi_spec))
        .route("/api/$smart/openapi.yaml", get(smart_openapi_spec))
        .route("/api/$smart/search", post(smart_rest_search).get(smart_rest_search))
        .route("/api/$smart/describe", post(smart_rest_describe).get(smart_rest_describe))
        .route("/api/$smart/call", post(smart_rest_call))
        .route(
            "/api/tools/{server}/{tool}",
            get(openapi_exec_global_get).post(openapi_exec_global_post),
        )
        .route(
            "/api/{name}/tools/{server}/{tool}",
            get(openapi_exec_scoped_get).post(openapi_exec_scoped_post),
        )
        // MCP Streamable HTTP protocol — served by the official rmcp SDK
        // (StreamableHttpService). Scope ("" / $smart / group / server) is
        // resolved per-request from the request path inside HubBridge.
        // The legacy 2024-11-05 dual-endpoint transport (GET /mcp endpoint
        // event + POST /mcp/message) is retired — rmcp deliberately does not
        // implement it and our logs show no real client traffic (AGENTS.md
        // §3.9.2).
        .route_service("/mcp", rmcp_service(sdk_cancel.clone()))
        .route_service("/mcp/{*path}", rmcp_service(sdk_cancel.clone()))
        .layer(axum::middleware::from_fn(mcp_route_gate_middleware))
        .layer(axum::middleware::from_fn(mcp_session_cleanup_middleware))
        .layer(axum::middleware::from_fn(mcp_leniency_middleware))
        // REST/OpenAPI POST bodies must be auth-gated BEFORE Json extraction
        // buffers them (up to 64MB each) — same rationale as /mcp below.
        .layer(axum::middleware::from_fn(rest_post_bearer_middleware))
        // Bearer LAST = outermost: auth must run before leniency spends parse
        // budget (up to 64MB) on unauthenticated requests.
        .layer(axum::middleware::from_fn(mcp_bearer_middleware))
        .layer(axum::extract::DefaultBodyLimit::max(body_limit_bytes))
        .layer(CorsLayer::permissive())
}

/// rmcp owns the `/mcp` DELETE (session termination) internally, so the
/// migration lost the pre-rmcp hook that reaped per-session isolated upstream
/// clients. This layer restores it: for a DELETE on any /mcp scope it reads
/// `mcp-session-id` up front and, after rmcp terminates the session, reaps
/// that session's isolated clients (`mcp/session_pool::cleanup_session` —
/// no-op when none exist). Fire-and-forget so the response is never delayed.
async fn mcp_session_cleanup_middleware(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let is_delete = req.method() == axum::http::Method::DELETE;
    let path_is_mcp = req
        .uri()
        .path()
        .split('?')
        .next()
        .map(|p| p == "/mcp" || p.starts_with("/mcp/"))
        .unwrap_or(false);
    // Idle-reaper input: ANY /mcp request carrying a session id refreshes its
    // last-seen stamp (not just DELETE — an active session must never age out).
    if path_is_mcp {
        if let Some(sid_seen) = req.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()).filter(|s| !s.is_empty()) {
            session_last_seen().lock().unwrap_or_else(|p| p.into_inner()).insert(sid_seen.to_string(), std::time::Instant::now());
        }
    }
    let sid = if is_delete && path_is_mcp {
        req.headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
    } else {
        None
    };
    let resp = next.run(req).await;
    if let Some(sid) = sid {
        tokio::spawn(async move {
            crate::mcp::session_pool::cleanup_session(&sid).await;
        });
    }
    resp
}

// ────────────────────────────────────────────────────────────────────────────
// Lifecycle
// ────────────────────────────────────────────────────────────────────────────

/// Loopback-hijack watch: while our server is running, periodically GET
/// 127.0.0.1:{port}/health and verify the responder is us. A foreign /health
/// means another app bound the loopback address (possible ANY time — e.g.
/// Cherry Studio starting after us and grabbing localhost thanks to
/// SO_REUSEADDR allowing wildcard+specific coexistence). Emits a warning
/// only on STATE CHANGE (clean -> hijacked, or hijacked -> clean) so the
/// dialog isn't re-raised every tick, and a later recovery clears it.
static LOOPBACK_HIJACK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static LOOPBACK_WATCH_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

async fn loopback_ok(port: u16) -> bool {
    let url = format!("http://127.0.0.1:{port}/health");
    match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => match c.get(&url).send().await {
            Ok(r) => match r.text().await {
                Ok(body) => body.contains("mcphub-desktop"),
                Err(_) => false,
            },
            // Unreachable loopback is a different problem (firewall);
            // not a hijack — stay silent.
            Err(_) => true,
        },
        Err(_) => true,
    }
}

/// Spawn the one-shot check after bind + the persistent 30s watch task.
/// The watch is started once per process; each `start()` (bind success or
/// already-running re-entry) also runs an immediate check so the failure
/// dialog's "retry start" re-verifies the loopback right away.
fn spawn_loopback_check(port: u16) {
    // One-shot, 500ms after bind — covers the startup window.
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if !loopback_ok(port).await {
            // Same stale-stop guard as the persistent watch below: stop() may
            // complete during the 500ms window (and a third party may own the
            // port) — report_loopback_hijack would unconditionally write a
            // fake running:true for a server that no longer exists.
            if current_port().await != Some(port) {
                log::info!("[http] loopback probe skipped: server stopped during probe window");
                return;
            }
            // Seed the flag so the first watch tick doesn't re-report the
            // same state change a second time (report is state-change driven).
            LOOPBACK_HIJACK.store(true, std::sync::atomic::Ordering::SeqCst);
            report_loopback_hijack(port, true);
        }
    });
    // Persistent watch — covers a squatter binding AFTER us (the one-shot
    // would have passed and never re-run). Re-reads the live port each tick
    // so a port-config change (start() restart branch) doesn't leave the
    // watch pinned to the old port (false hijack alerts on the stale port,
    // silence on the real one).
    if !LOOPBACK_WATCH_STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                let Some(current) = current_port().await else {
                    // Server intentionally stopped — watch stays alive but quiet.
                    // Clear the flag WITHOUT calling report_loopback_hijack(false):
                    // that helper unconditionally writes running:true, which would
                    // resurrect a fake "running" status for a stopped server.
                    let prev = LOOPBACK_HIJACK.swap(false, std::sync::atomic::Ordering::SeqCst);
                    if prev {
                        log::info!("[http] loopback flag cleared (server stopped)");
                    }
                    continue;
                };
                let hijacked = !loopback_ok(current).await;
                let prev = LOOPBACK_HIJACK.swap(hijacked, std::sync::atomic::Ordering::SeqCst);
                if hijacked != prev {
                    // Re-check the port still belongs to a live server: stop()
                    // may complete between current_port() and the probe, and
                    // report_loopback_hijack writes running:true — which would
                    // resurrect a fake "running" status for a stopped server.
                    if current_port().await == Some(current) {
                        report_loopback_hijack(current, hijacked);
                    } else {
                        LOOPBACK_HIJACK.store(false, std::sync::atomic::Ordering::SeqCst);
                    }
                }
            }
        });
    }
}

/// Persist + emit the loopback state transition so the frontend dialog opens
/// (hijack) or clears (recovered). Persisted via set_status — NOT a bare
/// emit — because events fired before the webview mounts its listener are
/// lost; the mount-time `get_http_server_status` fetch is the recovery path.
fn report_loopback_hijack(port: u16, hijacked: bool) {
    let warning = if hijacked {
        Some(format!(
            "localhost:{port} is served by ANOTHER application (loopback hijack). External clients via this machine's IP still reach MCPHub Desktop, but local clients using localhost will get wrong responses. Change the HTTP port in Settings → Routing, or stop the other app."
        ))
    } else {
        None
    };
    if hijacked {
        log::warn!("[http] {}", warning.as_deref().unwrap_or_default());
        app_logger::log_to_db("warn", &format!("[http] {}", warning.as_deref().unwrap_or_default()));
    } else {
        log::info!("[http] loopback recovered on port {port}");
        app_logger::log_to_db("info", &format!("[http] loopback recovered on port {port}"));
    }
    set_status(HttpServerStatus {
        running: true,
        port,
        error: None,
        error_kind: None,
        detail: None,
        warning,
    });
}

/// Start the HTTP server on the given port with the given body limit.
/// If a server is already running on the same port and limit — nothing to do.
/// Otherwise the old instance is stopped and a new one started.
pub async fn start(port: u16, body_limit_bytes: usize) -> anyhow::Result<()> {
    // Publish the limit before binding so the leniency middleware's per-request
    // snapshot read is current from the first request.
    std::sync::atomic::AtomicUsize::store(
        &BODY_LIMIT_SNAPSHOT,
        body_limit_bytes,
        std::sync::atomic::Ordering::Relaxed,
    );
    let mut guard = handle().lock().await;

    // Already running with the same port and body limit — nothing to do
    if let Some(ref h) = *guard {
        if h.port == port && h.body_limit_bytes == body_limit_bytes {
            log::info!("HTTP server already running on port {}", port);
            // Re-verify the loopback: the "retry start" button in the
            // failure dialog lands here when the server is still up (bind
            // was never the problem — e.g. loopback hijack). Without this
            // re-check the dialog closes on success while localhost stays
            // hijacked, and no future start() would re-raise it.
            spawn_loopback_check(port);
            return Ok(());
        }
        // Port or body limit changed — stop old instance
        log::info!("HTTP server config changed, restarting...");
    }
    // Tear the old instance down BEFORE binding the new one (restart branch;
    // no-op on first start): the guard replacement at the bottom of start()
    // only drops the old handle after the new bind has already succeeded, so
    // without this the restart branch races EADDRINUSE (Linux/Windows:
    // restart fails and status reports stopped while the old server keeps
    // serving) or a double-listen window (macOS SO_REUSEADDR). Graceful
    // shutdown is async — yield so the old accept loops release the port
    // before we re-bind.
    let old_port = guard.as_ref().map(|h| h.port);
    if let Some(h) = guard.take() {
        let _ = h.abort_tx.send(());
        let _ = h.abort_tx_lb.send(());
        let _ = h.probe_tx.send(());
        h.sdk_cancel.cancel();
        log::info!("HTTP server old instance stopped for restart (port {})", h.port);
    }
    // Wait for the old instance to actually release the port. A fixed 100ms
    // sleep is not enough: graceful shutdown waits for in-flight requests
    // (tool calls run up to 600s), so binding immediately would EADDRINUSE
    // while the old instance keeps serving under a "stopped" status. Poll
    // with a probe bind until the port is free (bounded), then bind for real.
    // Only relevant when the port is UNCHANGED — a new port has no bind
    // conflict with the old instance. On success the probe listener is KEPT
    // and reused as the real listener: dropping it would reopen a TOCTOU
    // window where a third party grabs the port between probe and re-bind
    // (review round 8, 2026-10-04).
    let mut reused_listener: Option<TcpListener> = None;
    if old_port == Some(port) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if std::time::Instant::now() > deadline {
                log::warn!("[http] restart: port {port} still busy after 5s, attempting bind anyway");
                break;
            }
            match tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
                Ok(l) => {
                    reused_listener = Some(l);
                    break;
                }
                Err(_) => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    continue;
                }
            }
        }
    }

    let sdk_cancel = tokio_util::sync::CancellationToken::new();
    let app = build_router(body_limit_bytes, sdk_cancel.clone());

    // Start the tasks TTL sweeper (drops expired 2025-11-25 tasks).
    mcp_tasks::spawn_ttl_sweeper();
    // Start the idle-session reaper (SDK keeps sessions forever otherwise).
    spawn_session_reaper();

    // Check TRUST_PROXY environment variable
    let trust_proxy = std::env::var("TRUST_PROXY").unwrap_or_default().to_lowercase();
    let trust_proxy = trust_proxy == "true" || trust_proxy == "1" || trust_proxy == "yes";

    let addr: SocketAddr = format!("0.0.0.0:{}", port).parse()?;
    let listener = match reused_listener {
        Some(l) => l,
        None => match TcpListener::bind(addr).await {
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
                    warning: None,
                });
                return Err(anyhow::anyhow!(err_msg));
            }
        },
    };
    // Explicit loopback bind (macOS only): the wildcard 0.0.0.0 bind leaves the
    // specific-address slot free, and SO_REUSEADDR (Node/Electron sets it by
    // default) lets another app bind 127.0.0.1:<port> AFTER us and silently
    // hijack every localhost client (longest-prefix match). Binding the
    // loopback OURSELVES closes that slot — but wildcard+specific coexistence
    // is a BSD/macOS peculiarity (SO_REUSEADDR semantics): on Linux the kernel
    // rejects the second bind with EADDRINUSE while a wildcard socket is
    // LISTENing (needs SO_REUSEPORT), and Windows std/tokio deliberately does
    // NOT set SO_REUSEADDR (its semantics allow port hijacking). Dual-binding
    // unconditionally would make the HTTP server fail to start on those
    // platforms entirely, so the anti-hijack bind is macOS-only; elsewhere the
    // loopback-watch probe below is the mitigation for the (BSD-specific)
    // squatting scenario it was designed for.
    #[cfg(target_os = "macos")]
    let loopback_listener: Option<TcpListener> = {
        let loopback_addr: SocketAddr = format!("127.0.0.1:{}", port).parse()?;
        // Bounded retry: during a same-port restart the old instance's
        // loopback listener drains in parallel with the wildcard (same 3s
        // graceful shutdown) and may still be LISTENing when we get here — a
        // single immediate bind would EADDRINUSE and take the whole restart
        // down (review round 9 F-2). Mirrors the wildcard probe-wait.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match TcpListener::bind(loopback_addr).await {
                Ok(l) => {
                    break Some(l);
                }
                Err(e) => {
                    if std::time::Instant::now() > deadline {
                        let err_msg = format!(
                            "Failed to bind loopback 127.0.0.1:{} while 0.0.0.0:{} succeeded — another process is squatting on the local address. Local clients would be hijacked; refusing to start exposed. ({e})",
                            port, port
                        );
                        log::error!("[http] {err_msg}");
                        app_logger::log_to_db("error", &err_msg);
                        set_status(HttpServerStatus {
                            running: false,
                            port,
                            error: Some(err_msg.clone()),
                            error_kind: Some("loopbackOccupied".to_string()),
                            detail: Some(format!("{e}")),
                            warning: None,
                        });
                        // Release the wildcard listener we just bound before failing.
                        drop(listener);
                        return Err(anyhow::anyhow!(err_msg));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
        }
    };
    #[cfg(not(target_os = "macos"))]
    let loopback_listener: Option<TcpListener> = None;
    let http_msg = format!("MCPHub HTTP server listening on http://0.0.0.0:{}{} (body limit: {} bytes, trust_proxy: {})", port, if loopback_listener.is_some() { " + http://127.0.0.1:".to_string() + &port.to_string() } else { String::new() }, body_limit_bytes, trust_proxy);
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
        warning: None,
    });

    // Loopback self-check (runs after the accept loop is up): hit
    // http://127.0.0.1:<port>/health and verify the responder is THIS app.
    // Scenario: another app already bound 127.0.0.1:<port> — on macOS a
    // 0.0.0.0 bind still succeeds, so there is no bind error, but every
    // `localhost:` client is served by the squatter and never reaches us
    // (silent breakage). Warn via a status event carrying `warning`.
    spawn_loopback_check(port);

    let (abort_tx, abort_rx) = tokio::sync::oneshot::channel::<()>();
    // Separate shutdown receiver for the loopback serve task: a oneshot
    // receiver is consumed by whoever awaits it, so each listener gets its
    // own cloned sender side.
    let (abort_tx_lb, abort_rx_lb) = tokio::sync::oneshot::channel::<()>();
    // Diagnostics probe (2026-08-27 "HTTP 服务静默挂掉" investigation): a second
    // channel fired alongside abort_tx in stop(). abort_rx is consumed by the
    // graceful-shutdown future and can't be probed after the fact; this probe
    // lets the serve task classify its own ending as intentional vs unexpected.
    let (probe_tx, mut abort_rx_probe) = tokio::sync::oneshot::channel::<()>();
    let gen = HTTP_START_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let watch_port = port;

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
        //
        // FIX (2026-10-04, review round 8): axum's graceful shutdown waits for
        // ALL in-flight connections to end — an open SSE stream (GET /mcp
        // server-push, subscriptions/listen) never ends on its own, so a
        // restart with any live SSE client hung forever, then failed
        // EADDRINUSE and left the server offline entirely. Graceful shutdown
        // is therefore BOUNDED: once the stop signal fires, in-flight requests
        // get a grace period, after which the serve future is dropped
        // forcibly (connections cut). Normal (non-SSE) requests complete well
        // within the grace.
        const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(3);
        // Separate Notify per listener: each graceful-shutdown future signals
        // only its own watch select, and `notify_one` (permit-storage) is used
        // instead of `notify_waiters` — a stop() racing the task's first poll
        // would otherwise lose the wakeup (waiter not yet registered) and the
        // 3s forced-abort guard would never fire (review round 9 F-3).
        let shutdown_notify = std::sync::Arc::new(tokio::sync::Notify::new());
        let lb_shutdown_notify = std::sync::Arc::new(tokio::sync::Notify::new());
        // Fires when the WILDCARD serve ends, so the loopback serve winds down
        // too (review round 9 F-1: previously the watch task awaited the
        // loopback task UNBOUNDED before classification — on Linux/Windows
        // that await deadlocked forever and the serveDied self-heal never ran;
        // on macOS a wildcard-only death hung the same way).
        let (lb_kill_tx, lb_kill_rx) = tokio::sync::oneshot::channel::<()>();
        let lb_serve = tokio::spawn({
            let app = app.clone();
            async move {
                use std::future::IntoFuture;
                if let Some(ll) = loopback_listener {
                    let notify2 = lb_shutdown_notify.clone();
                    let notify3 = notify2.clone();
                    let fut = std::pin::pin!(
                        axum::serve(ll, app)
                            .with_graceful_shutdown(async move {
                                tokio::select! {
                                    _ = abort_rx_lb => {},
                                    _ = lb_kill_rx => {},
                                }
                                notify3.notify_one();
                            })
                            .into_future()
                    );
                    let mut fut = fut;
                    tokio::select! {
                        res = &mut fut => res,
                        _ = notify2.notified() => {
                            match tokio::time::timeout(SHUTDOWN_GRACE, &mut fut).await {
                                Ok(res) => res,
                                Err(_) => {
                                    log::warn!("[http-server-watch] loopback: grace elapsed, forcing shutdown (in-flight SSE streams cut)");
                                    Ok(())
                                }
                            }
                        }
                    }
                } else {
                    // No loopback listener on this platform: nothing to serve,
                    // return immediately so the watch task's await below never
                    // blocks (review round 9 F-1).
                    Ok(())
                }
            }
        });
        let serve = axum::serve(listener, app).with_graceful_shutdown({
            let notify = shutdown_notify.clone();
            async move {
                let _ = abort_rx.await;
                notify.notify_one();
            }
        });
        let serve_fut = std::future::IntoFuture::into_future(serve);
        let mut guarded = Box::pin(futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(serve_fut)));
        let mut forced_abort = false;
        let wildcard_res = tokio::select! {
            res = &mut guarded => res,
            _ = shutdown_notify.notified() => {
                match tokio::time::timeout(SHUTDOWN_GRACE, &mut guarded).await {
                    Ok(res) => res,
                    Err(_) => {
                        forced_abort = true;
                        Ok(Ok(()))
                    }
                }
            }
        };
        let lb_res = {
            // Wind the loopback serve down and wait BOUNDED: the classification
            // and serveDied self-heal below must never depend on the loopback
            // task (review round 9 F-1).
            let _ = lb_kill_tx.send(());
            match tokio::time::timeout(std::time::Duration::from_secs(5), lb_serve).await {
                Ok(joined) => joined
                    .unwrap_or(Err(std::io::Error::other("loopback serve task panicked"))),
                Err(_) => Err(std::io::Error::other("loopback shutdown timed out")),
            }
        };
        let forced_note = if forced_abort { " [FORCED after grace - in-flight SSE streams cut]" } else { "" };
        let outcome = format!(
            "wildcard={}; loopback={}{}",
            match &wildcard_res {
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
            },
            match &lb_res {
                Ok(()) => "clean stop".to_string(),
                Err(e) => format!("accept-loop error: {e}"),
            },
            forced_note
        );
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
        // FIX (2026-10-04, review round 8): when the serve task died on its own
        // (panic / accept-loop error), SERVER_HANDLE stayed Some forever, so
        // every subsequent start()/sync_with_config hit the "already running"
        // no-op branch — the server could never self-heal. Clear the handle
        // (guarded by a generation counter so a NEWER start() instance that
        // raced us is not clobbered) and mark the status stopped.
        if !stopped_intentionally {
            if HTTP_START_GEN.load(std::sync::atomic::Ordering::SeqCst) == gen {
                let mut g = handle().lock().await;
                if HTTP_START_GEN.load(std::sync::atomic::Ordering::SeqCst) == gen {
                    *g = None;
                    let msg = "HTTP serve task died unexpectedly; handle cleared so it can restart".to_string();
                    log::error!("[http-server-watch] {msg}");
                    app_logger::log_to_db("error", &msg);
                    set_status(HttpServerStatus {
                        running: false,
                        port: watch_port,
                        error: Some(msg),
                        error_kind: Some("serveDied".to_string()),
                        detail: Some(outcome),
                        warning: None,
                    });
                }
            }
        }
        log::info!("MCPHub HTTP server stopped");
    });

    *guard = Some(ServerHandle { sdk_cancel, abort_tx, abort_tx_lb, probe_tx, port, body_limit_bytes });
    Ok(())
}

/// Stop the HTTP server if it is running.
pub async fn stop() {
    let mut guard = handle().lock().await;
    if let Some(h) = guard.take() {
        let _ = h.abort_tx.send(());
        let _ = h.abort_tx_lb.send(());
        let _ = h.probe_tx.send(());
        h.sdk_cancel.cancel();
        log::info!("MCPHub HTTP server shutdown requested");
        set_status(HttpServerStatus {
            running: false,
            port: h.port,
            error: None,
            error_kind: None,
            detail: None,
                warning: None,
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
                let raw_port = config
                    .get("httpPort")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(23333);
                // Validate instead of cast-truncating: 70000 as u16 = 4464
                // silently binds a different port; 0 binds ephemeral while
                // the status/UI would report port 0.
                if raw_port == 0 || raw_port > 65535 {
                    let msg = format!(
                        "Invalid httpPort {} in config — must be 1-65535; HTTP server not started",
                        raw_port
                    );
                    log::error!("{}", msg);
                    app_logger::log_to_db("error", &msg);
                    return;
                }
                let port = raw_port as u16;
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
                let raw_port = config
                    .get("httpPort")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(23333);
                // Validate instead of cast-truncating: 70000 as u16 = 4464
                // silently binds a different port; 0 binds ephemeral while
                // the status/UI would report port 0.
                if raw_port == 0 || raw_port > 65535 {
                    let msg = format!(
                        "Invalid httpPort {} in config — must be 1-65535; HTTP server not started",
                        raw_port
                    );
                    log::error!("{}", msg);
                    app_logger::log_to_db("error", &msg);
                    return;
                }
                let port = raw_port as u16;
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
        let _ = build_router(1024 * 1024, Default::default());
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
            title: None,
            execution: None,
            icons: None,
            meta: None,
            description_overridden: false,
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
            title: None,
            execution: None,
            icons: None,
            meta: None,
            description_overridden: false,
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
            title: None,
            execution: None,
            icons: None,
            meta: None,
            description_overridden: false,
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

// ── Smart Routing REST surface (/api/$smart/*) ──────────────────────────────

/// OpenAPI spec for the three smart-routing meta endpoints (for OpenWebUI
/// and other REST clients that can't speak MCP). Same bearer gate.
async fn smart_openapi_spec(headers: HeaderMap) -> Response {
    if let Err(r) = check_bearer_auth(&headers).await {
        return r;
    }
    // Same gate as the execution endpoints: importing a spec into a client
    // while Smart Routing is off would only produce opaque call failures —
    // surface the enable hint right at the spec fetch instead.
    if let Some(msg) = crate::smart_routing::meta::not_ready_message().await {
        return (StatusCode::FORBIDDEN, Json(json!({"error": msg}))).into_response();
    }
    let base = openapi_base_url(&headers);
    let url = |p: &str| format!("{base}/api/$smart{p}");
    let spec = json!({
        "openapi": "3.0.3",
        "info": {
            "title": "MCPHub Desktop Smart Routing API",
            "description": "AI-powered tool discovery over all connected MCP servers. Search for tools semantically, describe their schemas, and execute them.",
            "version": env!("CARGO_PKG_VERSION")
        },
        "servers": [{"url": format!("{base}/api/$smart")}],
        "paths": {
            "/search": {
                "post": {
                    "operationId": "smart_route_search",
                    "summary": "Search for relevant tools by natural-language query",
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {
                        "type": "object",
                        "properties": {
                            "query": {"type": "string", "description": "What you want to accomplish"},
                            "limit": {"type": "integer", "description": "Max results (default 10)"}
                        },
                        "required": ["query"]
                    }}}},
                    "responses": {"200": {"description": "Ranked tools with descriptions"}}
                }
            },
            "/describe": {
                "post": {
                    "operationId": "smart_route_describe",
                    "summary": "Get the full input schema for a specific tool",
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {
                        "type": "object",
                        "properties": {"toolName": {"type": "string"}},
                        "required": ["toolName"]
                    }}}},
                    "responses": {"200": {"description": "Tool info with full inputSchema"}}
                }
            },
            "/call": {
                "post": {
                    "operationId": "smart_route_call",
                    "summary": "Execute a tool by name with the given arguments",
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {
                        "type": "object",
                        "properties": {
                            "toolName": {"type": "string"},
                            "arguments": {"type": "object", "additionalProperties": true}
                        },
                        "required": ["toolName"]
                    }}}},
                    "responses": {"200": {"description": "Tool execution result (content/isError/structuredContent/_meta)"}}
                }
            }
        },
        "_comment_url_search": url("/search"),
        "_comment_url_describe": url("/describe"),
        "_comment_url_call": url("/call"),
    });
    (StatusCode::OK, Json(spec)).into_response()
}

/// Shared REST body for search/describe: accepts both JSON body and query
/// params (GET convenience).
fn smart_rest_params(
    body: Option<Json<serde_json::Value>>,
    q: Option<std::collections::HashMap<String, String>>,
) -> serde_json::Value {
    if let Some(Json(v)) = body {
        return v;
    }
    let mut m = serde_json::Map::new();
    if let Some(q) = q {
        for (k, v) in q {
            // Numeric query params (e.g. ?limit=5) must arrive as JSON numbers
            // — meta.rs reads them with as_u64(), which returns None for a
            // string and would silently fall back to the default. Only coerce
            // KNOWN numeric keys: a bare u64 parse would turn a numeric tool
            // name / query (e.g. ?toolName=12345) into a JSON number and break
            // as_str() reads downstream (review round 8, 2026-10-04).
            let val = if k == "limit" {
                v.parse::<u64>().map(Value::from).unwrap_or_else(|_| Value::String(v))
            } else {
                Value::String(v)
            };
            m.insert(k, val);
        }
    }
    Value::Object(m)
}

async fn smart_allowed_from_bearer(
    bearer_key: Option<&BearerKey>,
    scope_allowed: Option<Vec<String>>,
) -> Option<Vec<String>> {
    match (scope_allowed, get_allowed_servers(bearer_key).await) {
        (Some(list), Some(keys)) => Some(list.into_iter().filter(|s| keys.contains(s)).collect()),
        (Some(list), None) => Some(list),
        (None, Some(keys)) => Some(keys.into_iter().collect()),
        (None, None) => None,
    }
}

async fn smart_rest_search(
    headers: HeaderMap,
    Query(q): Query<std::collections::HashMap<String, String>>,
    body: Option<Json<serde_json::Value>>,
) -> Response {
    let bearer_key = match check_bearer_auth(&headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let gate = crate::smart_routing::meta::not_ready_message().await;
    if let Some(msg) = gate {
        return (StatusCode::FORBIDDEN, Json(json!({"error": msg}))).into_response();
    }
    let p = smart_rest_params(body, Some(q));
    let query = p.get("query").and_then(|v| v.as_str()).unwrap_or("");
    let limit = p.get("limit").cloned().unwrap_or(json!(10));
    let (_, _, scope_allowed) = crate::smart_routing::meta::compute_scope("$smart").await;
    let allowed = smart_allowed_from_bearer(bearer_key.as_ref(), scope_allowed).await;
    // Root $smart scope has no group tool whitelist — gate applies only to
    // /$smart/{group} (bridge handles that path).
    match crate::smart_routing::meta::handle_search_tools(query, limit, allowed, None).await {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response(),
    }
}

async fn smart_rest_describe(
    headers: HeaderMap,
    Query(q): Query<std::collections::HashMap<String, String>>,
    body: Option<Json<serde_json::Value>>,
) -> Response {
    let bearer_key = match check_bearer_auth(&headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let gate = crate::smart_routing::meta::not_ready_message().await;
    if let Some(msg) = gate {
        return (StatusCode::FORBIDDEN, Json(json!({"error": msg}))).into_response();
    }
    let p = smart_rest_params(body, Some(q));
    let tool_name = p.get("toolName").and_then(|v| v.as_str()).unwrap_or("");
    let (_, _, scope_allowed) = crate::smart_routing::meta::compute_scope("$smart").await;
    let allowed = smart_allowed_from_bearer(bearer_key.as_ref(), scope_allowed).await;
    match crate::smart_routing::meta::handle_describe_tool(tool_name, allowed, None).await {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        // handle_describe_tool returns Ok with an error body for unknown
        // tools (origin parity) — Err here is always a server fault.
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response(),
    }
}

async fn smart_rest_call(
    headers: HeaderMap,
    body: Option<Json<serde_json::Value>>,
) -> Response {
    let bearer_key = match check_bearer_auth(&headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let gate = crate::smart_routing::meta::not_ready_message().await;
    if let Some(msg) = gate {
        return (StatusCode::FORBIDDEN, Json(json!({"error": msg}))).into_response();
    }
    let p = body.map(|Json(v)| v).unwrap_or(json!({}));
    let tool_name = p.get("toolName").and_then(|v| v.as_str()).unwrap_or("");
    let args = p.get("arguments").cloned().unwrap_or(json!({}));
    let (_, _, scope_allowed) = crate::smart_routing::meta::compute_scope("$smart").await;
    let allowed = smart_allowed_from_bearer(bearer_key.as_ref(), scope_allowed).await;
    // NOTE (no pin gate here, deliberately): the REST lane's search results
    // are NOT narrowed to pinned tools (REST /api/$smart/search returns the
    // full $smart scope), so /call must stay consistent with that surface.
    // The MCP $smart lane (tools/list + smart_route_call) has its own
    // list/call pin parity inside the bridge.
    // No trusted-proxy header ⇒ source stays None (spoofing-proof by
    // construction, matching client_ip_of's contract); a fabricated
    // "127.0.0.1" would poison activity-log source_ip data.
    let ip = client_ip_of(&headers);
    let start = std::time::Instant::now();
    match crate::smart_routing::meta::handle_call_tool(tool_name, args, allowed, None).await {
        Ok(v) => {
            // isError:true results are tool-level failures — don't log them
            // as "success" (parity with execute_openapi_impl's is_error split).
            let status = if v.get("isError").and_then(|b| b.as_bool()).unwrap_or(false) {
                "error"
            } else {
                "success"
            };
            let _ = log_service::write_activity(
                "smart",
                tool_name,
                Some(start.elapsed().as_millis() as i64),
                status,
                None,
                Some(v.clone()),
                None,
                ip.as_deref(),
            )
            .await;
            (StatusCode::OK, Json(v)).into_response()
        }
        Err(e) => {
            let _ = log_service::write_activity(
                "smart",
                tool_name,
                Some(start.elapsed().as_millis() as i64),
                "error",
                None,
                None,
                Some(&e),
                ip.as_deref(),
            )
            .await;
            // Not-found semantics parity with /rest and describe: unknown
            // toolName is a client error, not a server fault. The real Err
            // strings from handle_call_tool: "Tool not available: {name}"
            // (unknown/unresolvable), "Tool '{name}' is excluded by the
            // group's tool filter" (group gate), "Tool '{name}' is disabled"
            // (disabled gate), "toolName parameter is required" (empty
            // toolName), upstream "[tool-not-found]" / "not found" (pool).
            // Client-error classification. Prefix/bracket-marker based — a
            // bare contains("not found") would misfile an UPSTREAM tool error
            // whose text happens to contain it (e.g. "config not found") as a
            // client mistake (400) instead of a server-side execution failure
            // (500).
            let status = if e.starts_with("[tool-not-found]")
                || e.starts_with("Tool not available")
                || (e.starts_with("Tool '")
                    && (e.contains("' is excluded by") || e.contains("' is disabled")))
                || e.starts_with("toolName parameter is required")
                || e.starts_with("Unknown smart routing tool")
            {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, Json(json!({"error": e}))).into_response()
        }
    }
}

#[cfg(test)]
mod limit_and_version_tests {
    use super::*;

    #[test]
    fn parse_body_limit_units() {
        assert_eq!(parse_body_limit("1mb"), 1024 * 1024);
        assert_eq!(parse_body_limit("2MB"), 2 * 1024 * 1024);
        assert_eq!(parse_body_limit("512kb"), 512 * 1024);
        assert_eq!(parse_body_limit("4096b"), 4096);
        assert_eq!(parse_body_limit("1048576"), 1048576);
        assert_eq!(parse_body_limit(""), 1024 * 1024);
        assert_eq!(parse_body_limit("garbage"), 1024 * 1024);
        // saturating: a huge-but-parseable value must not panic in debug
        assert_eq!(parse_body_limit("18014398509481983mb"), usize::MAX);
        // values beyond usize::MAX fail to parse and fall back to the default
        assert_eq!(parse_body_limit("99999999999999999999mb"), 1024 * 1024);
    }

    #[test]
    fn known_protocol_versions_exact_set() {
        assert!(is_known_protocol_version("2024-11-05"));
        assert!(is_known_protocol_version("2025-03-26"));
        assert!(is_known_protocol_version("2025-06-18"));
        assert!(is_known_protocol_version("2025-11-25"));
        assert!(is_known_protocol_version("2026-07-28"));
        // Lexicographic lookalikes must NOT pass the whitelist (9999-01-01
        // sorts after 2026-07-28 and used to be misrouted to the modern path)
        assert!(!is_known_protocol_version("9999-01-01"));
        assert!(!is_known_protocol_version("1999-01-01"));
        assert!(!is_known_protocol_version("garbage"));
        assert!(!is_known_protocol_version(""));
    }
}
