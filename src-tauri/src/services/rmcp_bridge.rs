//! rmcp bridge: official rmcp `StreamableHttpService` handler backed by the
//! MCP pool. Replaces the hand-written JSON-RPC dispatch (`dispatch_mcp`) on
//! the `/mcp` access point while `/rest`, `/api`, `/health` stay untouched.
//!
//! Scope strategy: one `HubBridge` instance per mounted path — the scope
//! (`""` global / `$smart` / group name / server name) is fixed at
//! construction, so the handler needs no per-request path parsing.
//! Bearer-key auth is resolved per-request from `http::request::Parts`
//! injected by the rmcp tower layer into `RequestContext.extensions`.

use std::collections::BTreeMap;
use std::borrow::Cow;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, CancelTaskParams, ContentBlock,
    CustomRequest, CustomResult, ErrorCode, GetPromptRequestParams, GetPromptResult,
    GetTaskParams, GetTaskResult, ListPromptsResult, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, Prompt,
    PromptArgument, PromptMessage, Role as PromptRole, ProtocolVersion,
    ReadResourceRequestParams, ReadResourceResult, ResourceContents, ServerCapabilities,
    ServerConfig, ToolAnnotations, UpdateTaskParams,
};
use rmcp::model::InputRequiredResult;
use rmcp::model::{GetPromptResponse, ReadResourceResponse};
use rmcp::service::{NotificationContext, Peer, RequestContext, RoleServer};
use rmcp::{ErrorData, ServerHandler};
use serde_json::{json, Value};
use std::sync::OnceLock;

// ─── Native list_changed notifications (2025-03/06/11 clients) ─────────────
// rmcp sessions are owned by LocalSessionManager; the handler only sees a
// `Peer` handle inside request/notification contexts. We remember each
// client's peer on `notifications/initialized` and fan out native
// `notifications/tools|prompts|resources/list_changed` from the change
// points in `subscription_hub`. Dead peers are pruned when a send fails.

static NATIVE_PEERS: OnceLock<tokio::sync::RwLock<Vec<Peer<RoleServer>>>> = OnceLock::new();

fn native_peers() -> &'static tokio::sync::RwLock<Vec<Peer<RoleServer>>> {
    NATIVE_PEERS.get_or_init(|| tokio::sync::RwLock::new(Vec::new()))
}

async fn remember_native_peer(peer: Peer<RoleServer>) {
    const MAX_NATIVE_PEERS: usize = 128;
    let mut peers = native_peers().write().await;
    // Dedup: same underlying transport handle registers once (initialized can
    // re-fire on reconnect with a fresh peer, so compare by pointer identity
    // is unnecessary — a Vec of clones of the same Peer would double-send).
    // Peer has no stable id exposed; rely on transport-level dedup instead:
    // duplicates send the same notification twice, which clients tolerate.
    // Cap: Peer exposes no stable identity (tx/id_provider are private), so
    // reconnecting clients can accumulate clones. Duplicate notifications are
    // idempotent for clients (they just re-list), but unbounded growth is a
    // leak — drop the oldest entries beyond the cap.
    peers.push(peer);
    if peers.len() > MAX_NATIVE_PEERS {
        let excess = peers.len() - MAX_NATIVE_PEERS;
        peers.drain(0..excess);
    }
}

/// Best-effort fan-out of a native list_changed notification to every live
/// 2025-style client session. Fire-and-forget: failures (session gone) just
/// prune the peer.
async fn fan_out_native_list_changed(kind: NativeListChanged) {
    use std::collections::HashSet;
    let peers = native_peers().write().await.clone();
    if peers.is_empty() {
        return;
    }
    let mut dead = Vec::new();
    for (i, peer) in peers.iter().enumerate() {
        let res = match kind {
            NativeListChanged::Tools => peer.notify_tool_list_changed().await,
            NativeListChanged::Prompts => peer.notify_prompt_list_changed().await,
            NativeListChanged::Resources => peer.notify_resource_list_changed().await,
        };
        if let Err(e) = res {
            crate::services::app_logger::log_to_db(
                "[rmcp] native list_changed send failed (pruning peer)",
                &format!("{:?}: {}", kind, e),
            );
            dead.push(i);
        }
    }
    if !dead.is_empty() {
        let dead_set: HashSet<usize> = dead.into_iter().collect();
        let mut peers = native_peers().write().await;
        *peers = peers
            .drain(..)
            .enumerate()
            .filter(|(i, _)| !dead_set.contains(i))
            .map(|(_, p)| p)
            .collect();
    }
}

#[derive(Debug, Clone, Copy)]
enum NativeListChanged {
    Tools,
    Prompts,
    Resources,
}

/// Public hooks called from `subscription_hub` change points (fire-and-forget).
pub fn spawn_native_notify(kind: &'static str) {
    let kind = match kind {
        "tools" => NativeListChanged::Tools,
        "prompts" => NativeListChanged::Prompts,
        "resources" => NativeListChanged::Resources,
        _ => return,
    };
    tokio::spawn(async move {
        fan_out_native_list_changed(kind).await;
    });
}
use crate::smart_routing::meta::GroupToolGate;
use super::http_server::builtin_allowed;

use super::http_server::{
    check_bearer_auth, get_allowed_servers, mcp_scope_server_filters, name_separator, ServerFilter,
};
use crate::mcp::pool;
use crate::models::server::Tool;

pub struct HubBridge;

impl HubBridge {
    /// 2026-07-28-only per-request version (modern requests carry `_meta`
    /// protocolVersion); false on legacy sessions.
    fn is_2026_session(context: &RequestContext<RoleServer>) -> bool {
        context
            .peer
            .peer_info()
            .map(|info| info.protocol_version.as_str() == "2026-07-28")
            .unwrap_or(false)
    }

    pub fn new() -> Self {
        Self
    }

    /// Per-request scope from the mounted path: `/mcp` -> "" (global),
    /// `/mcp/$smart`, `/mcp/{group}` etc. Mirror the dispatch semantics
    /// (scope = the raw path segment after /mcp).
    fn scope_from_ctx(context: &RequestContext<RoleServer>) -> String {
        context
            .extensions
            .get::<http::request::Parts>()
            .map(|p| p.uri.path().to_string())
            .and_then(|path| path.strip_prefix("/mcp").map(|s| s.to_string()))
            .map(|rest| rest.trim_start_matches('/').to_string())
            // axum exposes the raw percent-encoded path; server/group names
            // with non-ASCII chars (e.g. 本机公网ip查询) arrive as %XX sequences
            // and would never match the pool — decode before scope matching.
            .map(|rest| percent_encoding::percent_decode_str(&rest).decode_utf8_lossy().into_owned())
            .unwrap_or_default()
    }

    fn err(message: String) -> ErrorData {
        ErrorData::new(ErrorCode::INTERNAL_ERROR, message, None)
    }

    fn invalid_params(message: String) -> ErrorData {
        ErrorData::new(ErrorCode::INVALID_PARAMS, message, None)
    }

    /// Resolve bearer key (and 401 response as error) from the request Parts.
    async fn bearer_from_ctx(
        context: &RequestContext<RoleServer>,
    ) -> Result<Option<crate::models::bearer_key::BearerKey>, ErrorData> {
        let Some(parts) = context.extensions.get::<http::request::Parts>() else {
            return Ok(None);
        };
        match check_bearer_auth(&parts.headers).await {
            Ok(key) => Ok(key),
            Err(_resp) => Err(ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                "unauthorized".to_string(),
                None,
            )),
        }
    }

    /// Servers visible in this scope after bearer-key access control.
    async fn scope_filters(
        &self,
        scope: &str,
        bearer: Option<&crate::models::bearer_key::BearerKey>,
    ) -> Vec<ServerFilter> {
        let mut filters = mcp_scope_server_filters(scope).await;
        if let Some(allowed) = get_allowed_servers(bearer).await {
            filters.retain(|s| allowed.contains(&s.name));
        }
        filters
    }

    /// Aggregate visible tools for this scope. Mirrors the dispatch
    /// `tools/list` semantics: bearer filter, group allow-list, disabled skip,
    /// server-prefix when multiple servers are in scope, RAG builtin, smart
    /// meta tools on $smart scopes.
    async fn aggregate_tools(&self, scope: &str, bearer: Option<&crate::models::bearer_key::BearerKey>) -> Result<Vec<rmcp::model::Tool>, ErrorData> {
        // $smart scopes expose ONLY the meta tools.
        if crate::smart_routing::meta::is_smart_scope(scope) {
            if let Some(msg) = crate::smart_routing::meta::not_ready_message().await {
                return Err(Self::err(msg));
            }
            let settings = crate::smart_routing::models::get_settings().await;
            let bearer_list = get_allowed_servers(bearer)
                .await
                .map(|set| set.into_iter().collect::<Vec<_>>());
            let (scope_description, servers_list, _) =
                crate::smart_routing::meta::compute_scope_with(scope, bearer_list).await;
            let meta = crate::smart_routing::meta::build_meta_tools(
                &scope_description,
                &servers_list,
                settings.progressive_disclosure,
            );
            return to_rmcp_tools(&meta);
        }

        let server_filters = self.scope_filters(scope, bearer).await;
        let name_sep = name_separator().await;
        let use_prefix = server_filters.len() > 1;
        let mut tools: Vec<Value> = Vec::new();
        for sf in &server_filters {
            let is_builtin = sf.name == crate::rag::service::BUILTIN_SERVER_NAME;
            let ts: Vec<Tool> = if is_builtin {
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
                crate::services::server_tool_config_service::apply_tool_filters(&sf.name, ts)
                    .await
                    .unwrap_or_default()
            };
            for t in &filtered {
                if !t.enabled {
                    continue;
                }
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
                if let Some(a) = &t.annotations {
                    entry["annotations"] = a.clone();
                }
                if let Some(s) = &t.output_schema {
                    entry["outputSchema"] = s.clone();
                }
                tools.push(entry);
            }
        }
        to_rmcp_tools(&tools)
    }

    /// Resolve `(server_name, original_tool_name)` from an exposed tool name,
    /// mirroring the dispatch `tools/call` resolution (prefix strip, group
    /// allow-list, fallback by bare name).
    async fn resolve_target(
        &self,
        scope: &str,
        bearer: Option<&crate::models::bearer_key::BearerKey>,
        tool_name: &str,
    ) -> Option<(String, String)> {
        let server_filters = self.scope_filters(scope, bearer).await;
        let name_sep = name_separator().await;
        let use_prefix = server_filters.len() > 1;

        async fn tools_for(sf: &ServerFilter) -> Option<Vec<Tool>> {
            if sf.name == crate::rag::service::BUILTIN_SERVER_NAME {
                Some(crate::rag::service::builtin_tools())
            } else {
                pool::list_tools_for(&sf.name).await.ok()
            }
        }

        if use_prefix {
            for sf in &server_filters {
                let prefix = format!("{}{}", sf.name, name_sep);
                if let Some(orig_name) = tool_name.strip_prefix(&prefix) {
                    if let Some(ref allowed_tools) = sf.tools {
                        if !allowed_tools.contains(&orig_name.to_string()) {
                            continue;
                        }
                    }
                    if let Some(ts) = tools_for(sf).await {
                        if ts.iter().any(|t| t.name == orig_name) {
                            return Some((sf.name.clone(), orig_name.to_string()));
                        }
                    }
                }
            }
        }
        for sf in &server_filters {
            if let Some(ref allowed_tools) = sf.tools {
                if !allowed_tools.contains(&tool_name.to_string()) {
                    continue;
                }
            }
            if let Some(ts) = tools_for(sf).await {
                if ts.iter().any(|t| t.name == tool_name) {
                    return Some((sf.name.clone(), tool_name.to_string()));
                }
            }
        }
        None
    }
}

/// Convert our JSON tool entries (dispatch wire shape) into rmcp `Tool`s.
fn to_rmcp_tools(entries: &[Value]) -> Result<Vec<rmcp::model::Tool>, ErrorData> {
    let mut out = Vec::with_capacity(entries.len());
    for e in entries {
        let name = e
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ErrorData::new(ErrorCode::INTERNAL_ERROR, "tool entry missing name".to_string(), None))?;
        let input_schema: std::sync::Arc<serde_json::Map<String, Value>> = match e.get("inputSchema") {
            Some(Value::Object(m)) => std::sync::Arc::new(m.clone()),
            _ => std::sync::Arc::new(serde_json::Map::new()),
        };
        let output_schema = match e.get("outputSchema") {
            Some(Value::Object(m)) => Some(std::sync::Arc::new(m.clone())),
            _ => None,
        };
        let annotations = e.get("annotations").and_then(|a| {
            serde_json::from_value::<ToolAnnotations>(a.clone()).ok()
        });
        let tool = rmcp::model::Tool::new_with_raw(
            name.to_string(),
            e.get("description")
                .and_then(|v| v.as_str())
                .map(|s| Cow::Owned(s.to_string())),
            input_schema,
        );
        let tool = match output_schema {
            Some(o) => tool.with_raw_output_schema(o),
            None => tool,
        };
        let tool = match annotations {
            Some(a) => tool.with_annotations(a),
            None => tool,
        };
        out.push(tool);
    }
    Ok(out)
}

/// Convert a `ToolCallResult` into an rmcp response (MRTR aware: an upstream
/// `input_required` `_meta` becomes `CallToolResponse::InputRequired`).
fn to_call_response(result: crate::models::server::ToolCallResult) -> CallToolResponse {
    // MRTR: upstream asked for client-side input before completing.
    if !result.is_error {
        if let Some(meta) = &result.raw_meta {
            let rt = meta
                .get("io.modelcontextprotocol/resultType")
                .or_else(|| meta.get("resultType"))
                .and_then(|v| v.as_str());
            if rt == Some("input_required") {
                let input_requests: Option<BTreeMap<String, rmcp::model::InputRequest>> = meta
                    .get("io.modelcontextprotocol/inputRequests")
                    .or_else(|| meta.get("inputRequests"))
                    .and_then(|v| serde_json::from_value(v.clone()).ok());
                let request_state = meta
                    .get("io.modelcontextprotocol/requestState")
                    .or_else(|| meta.get("requestState"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                return CallToolResponse::InputRequired(InputRequiredResult::new(
                    input_requests,
                    request_state,
                ));
            }
        }
    }
    let mut out = CallToolResult::default();
    out.content = result
        .content
        .iter()
        .filter_map(|c| serde_json::from_value::<ContentBlock>(c.clone()).ok())
        .collect();
    out.structured_content = result.structured_content;
    out.is_error = Some(result.is_error);
    out.meta = result.raw_meta.and_then(|m| serde_json::from_value(m).ok());
    CallToolResponse::Complete(out)
}

impl HubBridge {
    async fn builtin_prompt_selection(&self, scope: &str) -> Option<Vec<String>> {
        let filters = mcp_scope_server_filters(scope).await;
        match filters.iter().find(|f| f.name == crate::rag::service::BUILTIN_SERVER_NAME) {
            Some(f) => f.prompts.clone(),
            None => None,
        }
    }

    async fn builtin_resource_selection(&self, scope: &str) -> Option<Vec<String>> {
        let filters = mcp_scope_server_filters(scope).await;
        match filters.iter().find(|f| f.name == crate::rag::service::BUILTIN_SERVER_NAME) {
            Some(f) => f.resources.clone(),
            None => None,
        }
    }

    async fn handle_custom_tasks(
        &self,
        method: &str,
        params: Option<Value>,
        stateless: bool,
    ) -> Option<Result<CustomResult, rmcp::ErrorData>> {
        let p = params.clone().unwrap_or_default();
        let task_id = p.get("taskId").and_then(|t| t.as_str()).unwrap_or("").to_string();
        let perr = |code: i32, msg: String| {
            rmcp::ErrorData::new(ErrorCode(code), msg, None)
        };
        match method {
            "tasks/get" => Some(Ok(CustomResult::new(if stateless {
                match crate::services::mcp_tasks::get_ext(&task_id).await {
                    Some(t) => t,
                    None => return Some(Err(Self::invalid_params(format!("Task '{}' not found", task_id)))),
                }
            } else {
                match crate::services::mcp_tasks::get(&task_id).await {
                    Some(t) => json!({"task": t}),
                    None => return Some(Err(Self::invalid_params(format!("Task '{}' not found", task_id)))),
                }
            }))),
            "tasks/result" => {
                if stateless {
                    return Some(Err(rmcp::ErrorData::new(ErrorCode::METHOD_NOT_FOUND, "tasks/result".to_string(), None)));
                }
                Some(match crate::services::mcp_tasks::result(&task_id).await {
                    Ok(r) => Ok(CustomResult::new(r)),
                    Err((code, msg)) => Err(perr(code as i32, msg)),
                })
            }
            "tasks/list" => {
                if stateless {
                    return Some(Err(rmcp::ErrorData::new(ErrorCode::METHOD_NOT_FOUND, "tasks/list".to_string(), None)));
                }
                Some(Ok(CustomResult::new(crate::services::mcp_tasks::list_all().await)))
            }
            _ => None,
        }
    }
}

impl ServerHandler for HubBridge {
    fn get_info(&self) -> ServerConfig {
        let mut caps = ServerCapabilities::builder()
            .enable_tools()
            .enable_tool_list_changed()
            .enable_prompts()
            .enable_prompts_list_changed()
            .enable_resources()
            .enable_resources_list_changed()
            .build();
        // 2026-07-28: tasks live in the SEP-1724 extensions map (not a core
        // capability); the bridge serves tasks/get|result|list|cancel.
        let mut extensions = rmcp::model::ExtensionCapabilities::new();
        extensions.insert(
            rmcp::model::TASKS_EXTENSION_ID.to_string(),
            serde_json::from_value(json!({})).expect("empty object is a valid JsonObject"),
        );
        caps.extensions = Some(extensions);
        ServerConfig::new(caps)
            .with_server_info(rmcp::model::Implementation::new("mcphub-desktop", env!("CARGO_PKG_VERSION")))
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(ProtocolVersion::KNOWN_VERSIONS)
    }

    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        remember_native_peer(context.peer).await;
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let bearer = Self::bearer_from_ctx(&context).await?;
        let scope = Self::scope_from_ctx(&context);
        let tools = self.aggregate_tools(&scope, bearer.as_ref()).await?;
        let result = ListToolsResult::with_all_items(tools);
        Ok(if Self::is_2026_session(&context) {
            result
                .with_ttl_ms(30_000)
                .with_cache_scope(rmcp::model::CacheScope::Private)
        } else {
            result
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let bearer = Self::bearer_from_ctx(&context).await?;
        let scope = Self::scope_from_ctx(&context);
        let scope_clean = scope.trim_start_matches('/').trim().to_string();
        let tool_name = request.name.to_string();
        let args = Value::Object(request.arguments.clone().unwrap_or_default());

        // $smart scopes: intercept the three meta tools.
        if crate::smart_routing::meta::is_smart_scope(&scope_clean) {
            if let Some(msg) = crate::smart_routing::meta::not_ready_message().await {
                return Err(Self::err(msg));
            }
            let allowed = {
                let (_, _, scope_allowed) =
                    crate::smart_routing::meta::compute_scope(&scope_clean).await;
                match (
                    scope_allowed,
                    get_allowed_servers(bearer.as_ref()).await,
                ) {
                    (Some(list), Some(keys_allowed)) => {
                        Some(list.into_iter().filter(|s| keys_allowed.contains(s)).collect::<Vec<_>>())
                    }
                    (Some(list), None) => Some(list),
                    (None, Some(keys_allowed)) => Some(keys_allowed.into_iter().collect()),
                    (None, None) => None,
                }
            };
            let group_gate: Option<GroupToolGate> = if scope_clean.starts_with("$smart/") {
                Some(
                    mcp_scope_server_filters(&scope)
                        .await
                        .into_iter()
                        .map(|sf| (sf.name, sf.tools))
                        .collect(),
                )
            } else {
                None
            };
            let result: Result<Value, String> = match tool_name.as_str() {
                "smart_route_search" => {
                    let q = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                    let limit = args.get("limit").cloned().unwrap_or(json!(10));
                    crate::smart_routing::meta::handle_search_tools(q, limit, allowed).await
                }
                "smart_route_describe" => {
                    let tn = args.get("toolName").and_then(|v| v.as_str()).unwrap_or("");
                    crate::smart_routing::meta::handle_describe_tool(tn, allowed, group_gate.as_ref()).await
                }
                "smart_route_call" => {
                    let tn = args.get("toolName").and_then(|v| v.as_str()).unwrap_or("");
                    let tool_args = args.get("arguments").cloned().unwrap_or(json!({}));
                    crate::smart_routing::meta::handle_call_tool(tn, tool_args, allowed, group_gate.as_ref()).await
                }
                other => Err(format!(
                    "Unknown smart routing tool '{}'. Available: smart_route_search, smart_route_describe, smart_route_call",
                    other
                )),
            };
            let value = result.map_err(Self::err)?;
            let content = vec![ContentBlock::text(value.to_string())];
            return Ok(CallToolResponse::Complete(
                CallToolResult::success(content),
            ));
        }

        let Some((sn, orig_name)) = self.resolve_target(&scope, bearer.as_ref(), &tool_name).await else {
            return Err(Self::invalid_params(format!(
                "Tool '{}' not found",
                tool_name
            )));
        };

        // RAG builtin server: local dispatch, no pool entry.
        if sn == crate::rag::service::BUILTIN_SERVER_NAME {
            let Some(app) = crate::mcp::progress::get_app_handle() else {
                return Err(Self::err("app handle unavailable".to_string()));
            };
            let result = crate::rag::service::call_builtin_tool(&app, &orig_name, &args)
                .await
                .map_err(|e| Self::err(e.to_string()))?;
            return Ok(to_call_response(result));
        }

        // Disabled-tool check.
        if let Ok(ts) = pool::list_tools_for(&sn).await {
            let filtered =
                crate::services::server_tool_config_service::apply_tool_filters(&sn, ts)
                    .await
                    .unwrap_or_default();
            if let Some(t) = filtered.iter().find(|t| t.name == orig_name) {
                if !t.enabled {
                    return Err(Self::invalid_params(format!(
                        "Tool '{}' is disabled",
                        orig_name
                    )));
                }
            }
        }

        // Per-session upstream isolation: reuse rmcp's session id header.
        let session_id = context
            .extensions
            .get::<http::request::Parts>()
            .and_then(|p| {
                p.headers
                    .get("mcp-session-id")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string())
            })
            .filter(|s| !s.is_empty());
        let is_isolated = session_id.is_some() && pool::is_per_session_client(&sn).await;

        // Request-level _meta passthrough (MRTR inputResponses retry, OTel).
        // A downstream client retrying an input_required call sends
        // `io.modelcontextprotocol/inputResponses` (+ requestState) as typed
        // params fields — merge them into the upstream `_meta` so the upstream
        // server can resume (mirrors the dispatch `tools/call` handling).
        let mut upstream_meta = request
            .meta
            .map(|m| Value::Object(m.0 .0.clone()))
            .unwrap_or_else(|| json!({}));
        if let Some(obj) = upstream_meta.as_object_mut() {
            if let Some(ir) = &request.input_responses {
                obj.insert(
                    "io.modelcontextprotocol/inputResponses".to_string(),
                    serde_json::to_value(ir).unwrap_or_default(),
                );
            }
            if let Some(rs) = &request.request_state {
                obj.insert(
                    "io.modelcontextprotocol/requestState".to_string(),
                    json!(rs),
                );
            }
        }

        if is_isolated {
            let sid = session_id.unwrap();
            let result = crate::mcp::session_pool::call_tool_isolated(
                &sid,
                &sn,
                &orig_name,
                args,
            )
            .await
            .map_err(|e| Self::err(e.to_string()))?;
            return Ok(to_call_response(result));
        }

        let result = pool::call_tool_with_meta(&sn, &orig_name, args, Some(upstream_meta))
            .await
            .map_err(|e| Self::err(e.to_string()))?;
        Ok(to_call_response(result))
    }

    // ── Prompts (builtin "mcphub-desktop" server only) ──────────────────

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, rmcp::ErrorData> {
        let _bearer = Self::bearer_from_ctx(&context).await?;
        let scope = Self::scope_from_ctx(&context);
        if crate::smart_routing::meta::is_smart_scope(scope.trim_start_matches('/').trim()) {
            let result = ListPromptsResult::with_all_items(Vec::new());
            return Ok(if Self::is_2026_session(&context) {
                result
                    .with_ttl_ms(30_000)
                    .with_cache_scope(rmcp::model::CacheScope::Private)
            } else {
                result
            });
        }
        let prompt_sel = self.builtin_prompt_selection(&scope).await;
        let prompts = crate::services::prompt_service::list_all().await.unwrap_or_default();
        let list = prompts
            .into_iter()
            .filter(|p| p.enabled && builtin_allowed(&prompt_sel, &p.name))
            .map(|p| {
                let args: Option<Vec<PromptArgument>> = Some(
                    p.arguments
                        .into_iter()
                        .map(|a| {
                            PromptArgument::new(a.name)
                                .with_description(a.description.unwrap_or_default())
                                .with_required(a.required)
                        })
                        .collect(),
                );
                let mut pr = Prompt::new(p.name, Some(p.description.unwrap_or_default()), args);
                pr.title = p.title;
                pr
            })
            .collect();
        let result = ListPromptsResult::with_all_items(list);
        Ok(if Self::is_2026_session(&context) {
            result
                .with_ttl_ms(30_000)
                .with_cache_scope(rmcp::model::CacheScope::Private)
        } else {
            result
        })
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, rmcp::ErrorData> {
        let scope = Self::scope_from_ctx(&context);
        let name = request.name.to_string();
        let args: Value = Value::Object(request.arguments.clone().unwrap_or_default());
        let prompt_sel = self.builtin_prompt_selection(&scope).await;
        let prompt = crate::services::prompt_service::list_all()
            .await
            .ok()
            .and_then(|ps| {
                ps.into_iter()
                    .find(|p| p.enabled && p.name == name && builtin_allowed(&prompt_sel, &p.name))
            });
        match prompt {
            Some(p) => {
                let text = crate::services::prompt_service::render_template(&p.template, &args);
                let mut gpr = GetPromptResult::default();
                gpr.description = p.description;
                gpr.messages = vec![PromptMessage::new_text(PromptRole::User, text)];
                Ok(gpr.into())
            }
            None => Err(Self::invalid_params(format!("Prompt '{}' not found", name))),
        }
    }

    // ── Resources (builtin only) ────────────────────────────────────────

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, rmcp::ErrorData> {
        let scope = Self::scope_from_ctx(&context);
        if crate::smart_routing::meta::is_smart_scope(scope.trim_start_matches('/').trim()) {
            let result = ListResourcesResult::with_all_items(Vec::new());
            return Ok(if Self::is_2026_session(&context) {
                result
                    .with_ttl_ms(30_000)
                    .with_cache_scope(rmcp::model::CacheScope::Private)
            } else {
                result
            });
        }
        let resource_sel = self.builtin_resource_selection(&scope).await;
        let resources = crate::services::resource_service::list_all().await.unwrap_or_default();
        let list = resources
            .into_iter()
            .filter(|r| r.enabled && builtin_allowed(&resource_sel, &r.uri))
            .map(|r| {
                let mut res = rmcp::model::Resource::new(r.uri.clone(), r.name.clone().unwrap_or_default());
                res.description = Some(r.description.unwrap_or_default());
                res.mime_type = Some(r.mime_type);
                res
            })
            .collect();
        let result = ListResourcesResult::with_all_items(list);
        Ok(if Self::is_2026_session(&context) {
            result
                .with_ttl_ms(30_000)
                .with_cache_scope(rmcp::model::CacheScope::Private)
        } else {
            result
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, rmcp::ErrorData> {
        let scope = Self::scope_from_ctx(&context);
        let uri = request.uri.clone();
        let resource_sel = self.builtin_resource_selection(&scope).await;
        let resource = crate::services::resource_service::list_all()
            .await
            .ok()
            .and_then(|rs| {
                rs.into_iter()
                    .find(|r| r.enabled && r.uri == uri && builtin_allowed(&resource_sel, &r.uri))
            });
        match resource {
            Some(r) => Ok(ReadResourceResult::new(vec![ResourceContents::TextResourceContents {
                uri: r.uri,
                mime_type: Some(r.mime_type),
                text: r.content,
                meta: None,
            }])
            .into()),
            None => Err(Self::invalid_params(format!("Resource '{}' not found", uri))),
        }
    }

    // ── Tasks (SEP-2663 + 2025-11-25 snapshots) ─────────────────────────

    async fn get_task(
        &self,
        request: GetTaskParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetTaskResult, rmcp::ErrorData> {
        let scope = Self::scope_from_ctx(&context);
        let stateless = crate::smart_routing::meta::is_smart_scope(scope.trim_start_matches('/').trim());
        let _ = stateless;
        // DetailedTask-shape gap: the hub's task store returns the wire JSON
        // (2026 ext shape or 2025-11 snapshot). Expose it raw via the custom
        // path; this typed handler serves the same store for rmcp-native
        // clients and errors like the old dispatch when missing.
        // The hub's task store produces the wire shape directly (2026 ext or
        // 2025-11 snapshot); rmcp's DetailedTask cannot express both, so the
        // typed tasks/get serves rmcp-native clients from the same store via
        // serde round-trip when possible.
        match crate::services::mcp_tasks::get_ext(&request.task_id).await {
            Some(t) => serde_json::from_value::<GetTaskResult>(t)
                .map_err(|e| Self::err(format!("task shape mismatch: {e}"))),
            None => Err(Self::invalid_params(format!(
                "Task '{}' not found",
                request.task_id
            ))),
        }
    }

    async fn update_task(
        &self,
        request: UpdateTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), rmcp::ErrorData> {
        crate::services::mcp_tasks::update_input(
            &request.task_id,
            serde_json::to_value(request.input_responses).unwrap_or_default(),
        )
        .await
        .map_err(|(code, msg)| rmcp::ErrorData::new(ErrorCode(code), msg, None))
    }

    async fn cancel_task(
        &self,
        request: CancelTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), rmcp::ErrorData> {
        crate::services::mcp_tasks::cancel(&request.task_id)
            .await
            .map(|_| ())
            .map_err(|(code, msg)| rmcp::ErrorData::new(ErrorCode(code), msg, None))
    }

    // tasks/get (full shape), tasks/result, tasks/list ride the custom path.

    async fn on_custom_request(
        &self,
        request: CustomRequest,
        _context: RequestContext<RoleServer>,
    ) -> Result<CustomResult, rmcp::ErrorData> {
        let scope = Self::scope_from_ctx(&_context);
        let stateless = crate::smart_routing::meta::is_smart_scope(scope.trim_start_matches('/').trim());
        if let Some(r) = self.handle_custom_tasks(&request.method, request.params.clone(), stateless).await {
            return r;
        }
        Err(rmcp::ErrorData::new(ErrorCode::METHOD_NOT_FOUND, request.method.clone(), None))
    }
}


#[cfg(test)]
mod native_notify_tests {
    use super::*;
    use rmcp::service::RoleClient;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone)]
    struct ProbeClient {
        tool_changes: Arc<AtomicUsize>,
        prompt_changes: Arc<AtomicUsize>,
        resource_changes: Arc<AtomicUsize>,
        seen: Arc<tokio::sync::Notify>,
    }

    impl rmcp::ClientHandler for ProbeClient {
        fn on_tool_list_changed(
            &self,
            _context: NotificationContext<RoleClient>,
        ) -> impl std::future::Future<Output = ()> + std::marker::Send + '_ {
            self.tool_changes.fetch_add(1, Ordering::SeqCst);
            self.seen.notify_one();
            std::future::ready(())
        }
        fn on_prompt_list_changed(
            &self,
            _context: NotificationContext<RoleClient>,
        ) -> impl std::future::Future<Output = ()> + std::marker::Send + '_ {
            self.prompt_changes.fetch_add(1, Ordering::SeqCst);
            self.seen.notify_one();
            std::future::ready(())
        }
        fn on_resource_list_changed(
            &self,
            _context: NotificationContext<RoleClient>,
        ) -> impl std::future::Future<Output = ()> + std::marker::Send + '_ {
            self.resource_changes.fetch_add(1, Ordering::SeqCst);
            self.seen.notify_one();
            std::future::ready(())
        }
    }

    #[tokio::test]
    async fn native_list_changed_fans_out_to_registered_peers() {
        let server = HubBridge::new();
        let client = ProbeClient {
            tool_changes: Arc::new(AtomicUsize::new(0)),
            prompt_changes: Arc::new(AtomicUsize::new(0)),
            resource_changes: Arc::new(AtomicUsize::new(0)),
            seen: Arc::new(tokio::sync::Notify::new()),
        };

        let (server_transport, client_transport) = tokio::io::duplex(4096);
        let _server_handle = tokio::spawn(async move {
            use rmcp::ServiceExt;
            server.serve(server_transport).await
        });
        use rmcp::ServiceExt as _;
        let client_service = client.clone().serve(client_transport).await.unwrap();
        let _ = client_service.peer().peer_info().expect("client initialized");
        // give the server side time to process notifications/initialized
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        spawn_native_notify("tools");
        spawn_native_notify("prompts");
        spawn_native_notify("resources");

        // Poll counters: Notify permits don't accumulate, so 3 rapid
        // notifications can leave waiters stuck. Counters are the truth.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let t = client.tool_changes.load(Ordering::SeqCst);
            let p = client.prompt_changes.load(Ordering::SeqCst);
            let r = client.resource_changes.load(Ordering::SeqCst);
            if t >= 1 && p >= 1 && r >= 1 {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out: tools={t} prompts={p} resources={r}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        assert_eq!(client.tool_changes.load(Ordering::SeqCst), 1, "tools/list_changed");
        assert_eq!(client.prompt_changes.load(Ordering::SeqCst), 1, "prompts/list_changed");
        assert_eq!(client.resource_changes.load(Ordering::SeqCst), 1, "resources/list_changed");
        client_service.cancel().await;
    }
}
