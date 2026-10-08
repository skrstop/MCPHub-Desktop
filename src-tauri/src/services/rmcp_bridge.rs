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
    CallToolRequestParams, CallToolResponse, CallToolResult, CancelTaskParams, CompleteRequestParams,
    CompleteRequestMethod, CompleteResult, ContentBlock,
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
use futures_util::FutureExt;

// ─── Native list_changed notifications (2025-03/06/11 clients) ─────────────
// rmcp sessions are owned by LocalSessionManager; the handler only sees a
// `Peer` handle inside request/notification contexts. We remember each
// client's peer on `notifications/initialized` and fan out native
// `notifications/tools|prompts|resources/list_changed` from the change
// points in `subscription_hub`. Dead peers are pruned when a send fails.

static NATIVE_PEERS: OnceLock<tokio::sync::RwLock<Vec<(std::sync::Arc<()>, Peer<RoleServer>)>>> =
    OnceLock::new();

fn native_peers()
-> &'static tokio::sync::RwLock<Vec<(std::sync::Arc<()>, Peer<RoleServer>)>> {
    NATIVE_PEERS.get_or_init(|| tokio::sync::RwLock::new(Vec::new()))
}

async fn remember_native_peer(peer: Peer<RoleServer>) {
    const MAX_NATIVE_PEERS: usize = 128;
    let mut peers = native_peers().write().await;
    // Identity tag: each registration gets a unique Arc<()> so pruning can
    // remove exactly the peers that failed THIS fan-out pass (by Arc pointer
    // identity) — immune to the index-shift race caused by concurrent
    // cap-drain in remember_native_peer.
    peers.push((std::sync::Arc::new(()), peer));
    // Cap: Peer exposes no stable identity (tx/id_provider are private), so
    // reconnecting clients can accumulate clones. Duplicate notifications are
    // idempotent for clients (they just re-list), but unbounded growth is a
    // leak — drop the oldest entries beyond the cap.
    if peers.len() > MAX_NATIVE_PEERS {
        let excess = peers.len() - MAX_NATIVE_PEERS;
        peers.drain(0..excess);
    }
}

/// Best-effort fan-out of a native list_changed notification to every live
/// 2025-style client session. Fire-and-forget: failures (session gone) just
/// prune the peer.
async fn fan_out_native_list_changed(kind: NativeListChanged) {
    // Note: a client that both sent `notifications/initialized` (registered
    // here) and opened a `subscriptions/listen` stream receives each
    // list_changed twice — once on its session stream, once on the listen
    // stream. Both are valid channels per rmcp; duplicates are idempotent
    // for clients (they just re-list). Upstream rmcp treats the streams as
    // independent and exposes no peer identity to dedupe safely.
    use std::collections::HashSet;
    use std::sync::Arc as StdArc;
    let snapshot = native_peers().read().await.clone();
    if snapshot.is_empty() {
        return;
    }
    let mut dead_tokens: Vec<StdArc<()>> = Vec::new();
    for (token, peer) in snapshot.iter() {
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
            dead_tokens.push(token.clone());
        }
    }
    if !dead_tokens.is_empty() {
        let dead_ptrs: HashSet<usize> = dead_tokens
            .iter()
            .map(|t| StdArc::as_ptr(t) as usize)
            .collect();
        let mut peers = native_peers().write().await;
        peers.retain(|(token, _)| !dead_ptrs.contains(&(StdArc::as_ptr(token) as usize)));
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

// ─── Legacy resources/subscribe (2025-03/06/11 clients) ────────────────────
// We advertise `resources.subscribe` in capabilities (needed so the 2026
// `subscriptions/listen` resourceSubscriptions filter is not stripped), so
// legacy clients per spec may call `resources/subscribe`. Registry of
// (identity token, peer, uri, session fingerprint); fan-out on resource
// content change, pruning dead peers by Arc pointer identity (same pattern as
// NATIVE_PEERS). The fingerprint is a Weak to the peer's per-session PeerInfo
// Arc — it lets unsubscribe match the exact session instead of "most recent
// matching uri", which could delete ANOTHER client's registration (review
// round 8, 2026-10-04). Weak (not a raw address) because rmcp re-runs
// set_peer_info on a repeated initialize in the same session, REPLACING the
// Arc: a raw address would go stale and the session could never unsubscribe
// (review round 9, 2026-10-04). Dead Weak = re-initialized session.
static LEGACY_RESOURCE_SUBS: OnceLock<
    tokio::sync::RwLock<Vec<(std::sync::Arc<()>, Peer<RoleServer>, String, Option<std::sync::Weak<rmcp::model::InitializeRequestParams>>)>>,
> = OnceLock::new();

fn legacy_resource_subs() -> &'static tokio::sync::RwLock<
    Vec<(std::sync::Arc<()>, Peer<RoleServer>, String, Option<std::sync::Weak<rmcp::model::InitializeRequestParams>>)>,
> {
    LEGACY_RESOURCE_SUBS.get_or_init(|| tokio::sync::RwLock::new(Vec::new()))
}

fn peer_session_fingerprint(
    peer: &Peer<RoleServer>,
) -> Option<std::sync::Weak<rmcp::model::InitializeRequestParams>> {
    peer.peer_info().map(|i| std::sync::Arc::downgrade(&i))
}

const MAX_LEGACY_SUBS: usize = 256;
// Known cap tradeoff (same as NATIVE_PEERS): drain(0..excess) evicts the
// OLDEST entries, and dead sessions are only pruned on fan-out send failure —
// a swarm of silent dead sessions can evict live subscribers. Accepted bound;
// documented in the round-8 review record.

pub async fn legacy_resource_subscribe(peer: Peer<RoleServer>, uri: String) {
    let fp = peer_session_fingerprint(&peer);
    let mut subs = legacy_resource_subs().write().await;
    // Dedup on (uri, fingerprint): a repeated subscribe from the same session
    // must not stack duplicate entries (duplicate notifications) (review
    // round 10).
    let fp_now = fp.as_ref();
    subs.retain(|(_, _, u, f)| {
        !(u == &uri
            && match (f, fp_now) {
                (Some(f), Some(fp)) => f.ptr_eq(fp) || f.strong_count() == 0,
                (None, None) => true,
                _ => false,
            })
    });
    // Drop stale entries (dead fingerprint = the session re-initialized and
    // its PeerInfo Arc was replaced): the Peer channel is still alive so
    // fan-out never prunes them naturally — left alone they duplicate
    // deliveries and the 256 cap drain can evict LIVE subscribers (review
    // round 10).
    subs.retain(|(_, _, _, f)| match f {
        Some(f) => f.strong_count() > 0,
        None => true,
    });
    subs.push((std::sync::Arc::new(()), peer, uri, fp));
    if subs.len() > MAX_LEGACY_SUBS {
        let excess = subs.len() - MAX_LEGACY_SUBS;
        subs.drain(0..excess);
    }
}

pub async fn legacy_resource_unsubscribe(peer: &Peer<RoleServer>, uri: &str) {
    // Match the exact session first (PeerInfo Weak identity); then fall back
    // to a stale fingerprint (dead Weak, same uri) — that is the SAME session
    // after a repeated initialize replaced its PeerInfo Arc. Never fall back
    // to "most recent matching uri" with a live foreign fingerprint: that
    // would delete ANOTHER client's registration.
    let fp = peer_session_fingerprint(peer);
    let mut subs = legacy_resource_subs().write().await;
    let pos = subs
        .iter()
        .rposition(|(_, _, u, f)| {
            u == uri
                && match (f, &fp) {
                    (Some(f), Some(fp)) => f.ptr_eq(fp) || f.strong_count() == 0,
                    _ => false,
                }
        })
        // unsubscribed before initialize (no info on either side)
        .or_else(|| subs.iter().rposition(|(_, _, u, f)| u == uri && f.is_none() && fp.is_none()));
    if let Some(pos) = pos {
        subs.remove(pos);
    }
}

/// Fan out `notifications/resources/updated` to legacy subscribers of `uri`.
pub async fn fan_out_legacy_resource_updated(uri: &str) {
    use std::collections::HashSet;
    use std::sync::Arc as StdArc;
    let snapshot = legacy_resource_subs().read().await.clone();
    if snapshot.is_empty() {
        return;
    }
    let mut dead_tokens: Vec<StdArc<()>> = Vec::new();
    for (token, peer, sub_uri, _) in snapshot.iter() {
        if sub_uri != uri {
            continue;
        }
        if let Err(e) = peer.notify_resource_updated(rmcp::model::ResourceUpdatedNotificationParam::new(uri)).await {
            dead_tokens.push(token.clone());
            let _ = e;
        }
    }
    if !dead_tokens.is_empty() {
        let dead_ptrs: HashSet<usize> = dead_tokens
            .iter()
            .map(|t| StdArc::as_ptr(t) as usize)
            .collect();
        let mut subs = legacy_resource_subs().write().await;
        subs.retain(|(token, _, _, _)| !dead_ptrs.contains(&(StdArc::as_ptr(token) as usize)));
    }
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
    /// protocolVersion); false on legacy sessions. Aligned with the SDK's own
    /// SEP-2322 gating (service.rs `RequestContext::protocol_version`): the
    /// per-request `_meta.protocolVersion` takes precedence over peer_info so
    /// response shaping (resultType/ttlMs/cacheScope) can't contradict the
    /// SDK's strip decision on the same response.
    fn is_2026_session(context: &RequestContext<RoleServer>) -> bool {
        context
            .protocol_version()
            .is_some_and(|v| v.as_str() == "2026-07-28")
    }

    /// 2024-11-05 legacy sessions: the spec predates annotations/outputSchema,
    /// so the retired dispatch stripped them from tools/list — restore that
    /// contract (e2e TC-06).
    fn is_2024_session(context: &RequestContext<RoleServer>) -> bool {
        context
            .protocol_version()
            .is_some_and(|v| v.as_str() == "2024-11-05")
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
            // Single normalization point: trailing slashes must be trimmed
            // here or `$smart/{group}/` reaches pinned_tools_for_scope with a
            // trailing-slash group name (list_all lookup fails → group-level
            // pins silently vanish, while server-level pins via
            // mcp_scope_server_filters — which trims — still work).
            .map(|rest| rest.trim_end_matches('/').to_string())
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
        // A group listing the same server twice must not duplicate tools/pins.
        let mut seen = std::collections::HashSet::new();
        filters.retain(|s| seen.insert(s.name.clone()));
        if let Some(allowed) = get_allowed_servers(bearer).await {
            filters.retain(|s| allowed.contains(&s.name));
        }
        filters
    }

    /// Per-member pinned tool names for a `$smart/{group}` scope (origin
    /// `getPinnedSmartRoutingTools` selection rules at the name level): each
    /// member's `pinnedTools`, narrowed to its tools selection ("all" / absent
    /// selection = everything allowed).
    /// Pinned `(server, [tool])` pairs visible in this $smart scope — the
    /// single source for both the listing parity gate and the resolve
    /// fallback. Group scope: member pins ∪ server-level pins per visible
    /// member; root scope: server-level pins across bearer-visible servers.
    async fn smart_pinned_servers(
        &self,
        scope: &str,
        bearer: Option<&crate::models::bearer_key::BearerKey>,
    ) -> Vec<(String, Vec<String>)> {
        let scope_clean = scope.trim_start_matches('/').trim();
        let mut out: Vec<(String, Vec<String>)> = Vec::new();
        let member_pins: std::collections::HashMap<String, Vec<String>> =
            if scope_clean.starts_with("$smart/") {
                Self::pinned_tools_for_scope(scope_clean)
                    .await
                    .into_iter()
                    .collect()
            } else {
                std::collections::HashMap::new()
            };
        for sf in self.scope_filters(scope, bearer).await {
            let mut pins = member_pins.get(&sf.name).cloned().unwrap_or_default();
            match crate::services::server_tool_config_service::list_pinned_tools(&sf.name).await {
                Ok(extra) => {
                    for p in extra {
                        if !pins.contains(&p) {
                            pins.push(p);
                        }
                    }
                }
                Err(e) => log::warn!("[smart] list_pinned_tools({}) failed: {}", sf.name, e),
            }
            if !pins.is_empty() {
                out.push((sf.name.clone(), pins));
            }
        }
        out
    }

    /// Resolve a pinned direct-call name on a $smart scope under the same
    /// parity rules as the listing: prefixed form when multiple servers are
    /// visible, bare name only in single-server scopes, meta-named pins never
    /// callable. Unlike `resolve_target`, this does NOT consult the pool's
    /// tool cache — a pin on a sleeping (never-woken) on-demand server still
    /// resolves, and the subsequent `pool::call_tool` path cold-starts it.
    pub(crate) async fn resolve_pinned_target(
        &self,
        scope: &str,
        bearer: Option<&crate::models::bearer_key::BearerKey>,
        tool_name: &str,
    ) -> Option<(String, String)> {
        let pinned = self.smart_pinned_servers(scope, bearer).await;
        let server_filters = self.scope_filters(scope, bearer).await;
        let name_sep = name_separator().await;
        let use_prefix = server_filters.len() > 1;
        for (server, pins) in &pinned {
            for p in pins {
                // A pin named like a meta tool is never listed — its prefixed
                // form must not be callable either (parity).
                if crate::smart_routing::meta::is_meta_tool(p) {
                    continue;
                }
                let hit = if use_prefix {
                    format!("{}{}{}", server, name_sep, p) == tool_name
                } else {
                    *p == tool_name
                };
                if !hit {
                    continue;
                }
                // Group selection whitelist parity: the listing narrows every
                // pin (server-level ones included) to the member's `tools`
                // selection; the call side must refuse the same set.
                if let Some(sf) = server_filters.iter().find(|f| f.name == *server) {
                    if let Some(ref allowed) = sf.tools {
                        if !allowed.contains(p) {
                            continue;
                        }
                    }
                }
                return Some((server.clone(), p.clone()));
            }
        }
        None
    }

    async fn pinned_tools_for_scope(scope: &str) -> Vec<(String, Vec<String>)> {
        let clean = scope.trim_start_matches('/').trim();
        let Some(group_name) = clean.strip_prefix("$smart/") else {
            return Vec::new();
        };
        let Ok(groups) = crate::services::group_service::list_all().await else {
            return Vec::new();
        };
        let Some(g) = groups
            .iter()
            .find(|g| g.name == group_name || g.id == group_name)
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for member in &g.servers {
            let Some(name) = member.get("name").and_then(|v| v.as_str()) else {
                continue;
            };
            let selection: Option<Vec<String>> = member.get("tools").and_then(|v| v.as_array()).map(
                |a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect()
                },
            );
            let pinned: Vec<String> = member
                .get("pinnedTools")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .filter(|t| match &selection {
                            Some(sel) => sel.contains(t),
                            None => true,
                        })
                        .collect()
                })
                .unwrap_or_default();
            if !pinned.is_empty() {
                out.push((name.to_string(), pinned));
            }
        }
        out
    }

    /// Aggregate visible tools for this scope. Mirrors the dispatch
    /// `tools/list` semantics: bearer filter, group allow-list, disabled skip,
    /// server-prefix when multiple servers are in scope, RAG builtin, smart
    /// meta tools on $smart scopes.
    async fn aggregate_tools(
        &self,
        scope: &str,
        bearer: Option<&crate::models::bearer_key::BearerKey>,
        strip_2024: bool,
    ) -> Result<Vec<rmcp::model::Tool>, ErrorData> {
        // $smart scopes expose the meta tools plus — on group scopes only —
        // each member's pinned tools (origin parity). The root $smart scope
        // stays meta-only.
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
            let full_n = if settings.progressive_disclosure {
                None
            } else {
                settings.full_schema_top_n
            };
            let meta = crate::smart_routing::meta::build_meta_tools(
                &scope_description,
                &servers_list,
                settings.progressive_disclosure,
                full_n,
            );
            let mut tools = to_rmcp_tools(&meta)?;
            let scope_clean = scope.trim_start_matches('/').trim();
            if scope_clean.starts_with("$smart/") {
                let server_filters = self.scope_filters(scope, bearer).await;
                let name_sep = name_separator().await;
                let use_prefix = server_filters.len() > 1;
                let member_pins: std::collections::HashMap<String, Vec<String>> =
                    Self::pinned_tools_for_scope(scope)
                        .await
                        .into_iter()
                        .collect();
                // Iterate ALL visible members (not just ones with group pins):
                // server-level pins must surface even when the group member
                // entry carries none.
                for sf in &server_filters {
                    let mut pinned_names = member_pins
                        .get(&sf.name)
                        .cloned()
                        .unwrap_or_default();
                    match crate::services::server_tool_config_service::list_pinned_tools(&sf.name)
                        .await
                    {
                        Ok(pins) => {
                            for p in pins {
                                if !pinned_names.contains(&p) {
                                    pinned_names.push(p);
                                }
                            }
                        }
                        Err(e) => log::warn!("[smart] list_pinned_tools({}) failed: {}", sf.name, e),
                    }
                    if pinned_names.is_empty() {
                        continue;
                    }
                    let Ok(ts) = pool::list_tools_for(&sf.name).await else {
                        continue;
                    };
                    let filtered = crate::services::server_tool_config_service::apply_tool_filters(
                        &sf.name, ts,
                    )
                    .await
                    .unwrap_or_else(|e| {
                        log::warn!("[{}] apply_tool_filters failed: {}", sf.name, e);
                        Vec::new()
                    });
                    for t in &filtered {
                        if !t.enabled || !pinned_names.contains(&t.name) {
                            continue;
                        }
                        if let Some(ref allowed) = sf.tools {
                            if !allowed.contains(&t.name) {
                                continue;
                            }
                        }
                        // A pin named like a meta tool would be intercepted by
                        // it on call — never list it (origin parity).
                        if crate::smart_routing::meta::is_meta_tool(&t.name) {
                            continue;
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
                        if !strip_2024 {
                            if let Some(a) = &t.annotations {
                                entry["annotations"] = a.clone();
                            }
                            if let Some(s) = &t.output_schema {
                                entry["outputSchema"] = s.clone();
                            }
                        }
                        tools.push(
                            to_rmcp_tools(std::slice::from_ref(&entry))
                                .map(|mut v| v.pop().unwrap())?,
                        );
                    }
                }
            } else if scope_clean == "$smart" {
                // Root smart scope with server-level pins: the desktop-only
                // "server smart" surface — meta tools plus every visible
                // server's own pinned tools (origin only pins at group level).
                let server_filters = self.scope_filters(scope, bearer).await;
                let name_sep = name_separator().await;
                let use_prefix = server_filters.len() > 1;
                for sf in &server_filters {
                    let Ok(pins) =
                        crate::services::server_tool_config_service::list_pinned_tools(&sf.name)
                            .await
                    else {
                        continue;
                    };
                    if pins.is_empty() {
                        continue;
                    }
                    let Ok(ts) = pool::list_tools_for(&sf.name).await else {
                        continue;
                    };
                    let filtered = crate::services::server_tool_config_service::apply_tool_filters(
                        &sf.name, ts,
                    )
                    .await
                    .unwrap_or_else(|e| {
                        log::warn!("[{}] apply_tool_filters failed: {}", sf.name, e);
                        Vec::new()
                    });
                    for t in &filtered {
                        if !t.enabled || !pins.contains(&t.name) {
                            continue;
                        }
                        if let Some(ref allowed) = sf.tools {
                            if !allowed.contains(&t.name) {
                                continue;
                            }
                        }
                        if crate::smart_routing::meta::is_meta_tool(&t.name) {
                            continue;
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
                        if !strip_2024 {
                            if let Some(a) = &t.annotations {
                                entry["annotations"] = a.clone();
                            }
                            if let Some(s) = &t.output_schema {
                                entry["outputSchema"] = s.clone();
                            }
                        }
                        tools.push(
                            to_rmcp_tools(std::slice::from_ref(&entry))
                                .map(|mut v| v.pop().unwrap())?,
                        );
                    }
                }
            }
            return Ok(tools);
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
            // Filters apply to the builtin server too: the servers panel can
            // disable rag_* tools and the HTTP /mcp path must honour it
            // (parity with the Tauri command path and execute_tool_call).
            let filtered =
                crate::services::server_tool_config_service::apply_tool_filters(&sf.name, ts)
                    .await
                    .unwrap_or_else(|e| {
                        // Fail-open only for tool LISTING (visibility); log it
                        // so a broken filter store is visible in the logs.
                        log::warn!("[{}] apply_tool_filters failed: {}", sf.name, e);
                        Vec::new()
                    });
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
                if !strip_2024 {
                    if let Some(a) = &t.annotations {
                        entry["annotations"] = a.clone();
                    }
                    if let Some(s) = &t.output_schema {
                        entry["outputSchema"] = s.clone();
                    }
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
            // Longest prefix first: `web` must not shadow `web-search` when
            // separator-stripping `web-search-x` (server order is pool order,
            // not length order — first-match otherwise hijacks the target).
            let mut ordered: Vec<&ServerFilter> = server_filters.iter().collect();
            ordered.sort_by(|a, b| {
                let la = a.name.len() + name_sep.len();
                let lb = b.name.len() + name_sep.len();
                lb.cmp(&la)
            });
            for sf in ordered {
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
fn to_call_response(
    result: crate::models::server::ToolCallResult,
    strip_2024: bool,
    sep_2322: bool,
) -> CallToolResponse {
    // MRTR: upstream asked for client-side input before completing.
    if !result.is_error {
        if let Some(meta) = &result.raw_meta {
            let rt = meta
                .get("io.modelcontextprotocol/resultType")
                .or_else(|| meta.get("resultType"))
                .and_then(|v| v.as_str());
            if rt == Some("input_required") {
                // Distinguish "key absent" from "key present but unparsable":
                // a malformed payload collapsed to None by .ok() must NOT fall
                // through to a request-less InputRequired (client cannot
                // construct inputResponses — the interaction dead-ends).
                let raw_input_requests = meta
                    .get("io.modelcontextprotocol/inputRequests")
                    .or_else(|| meta.get("inputRequests"));
                let input_requests: Option<BTreeMap<String, rmcp::model::InputRequest>> = raw_input_requests
                    .and_then(|v| serde_json::from_value(v.clone()).ok());
                let request_state = meta
                    .get("io.modelcontextprotocol/requestState")
                    .or_else(|| meta.get("requestState"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                // SDK invariant (model/mrtr.rs deserializer): at least one of
                // inputRequests/requestState MUST be present — a bare
                // `{"resultType":"input_required"}` cannot be deserialized by
                // strict downstream clients. The hand-written SSE transport can
                // fold such raw upstream JSON; fall back to an errored Complete
                // instead of an unanswerable InputRequired.
                if raw_input_requests.is_some() && input_requests.is_none() {
                    // Present but unparsable — same unanswerable-outcome guard
                    // as the both-absent case.
                    let mut out = CallToolResult::default();
                    out.is_error = Some(true);
                    out.content = vec![ContentBlock::text(
                        "upstream declared input_required with an unparseable inputRequests payload",
                    )];
                    return CallToolResponse::Complete(out);
                }
                if input_requests.is_none() && request_state.is_none() {
                    let mut out = CallToolResult::default();
                    out.is_error = Some(true);
                    out.content = vec![ContentBlock::text("upstream declared input_required without inputRequests/requestState")];
                    return CallToolResponse::Complete(out);
                }
                // SEP-2322 gating (SDK handler/server.rs:245): the SDK folds an
                // InputRequiredResult returned to a pre-2026 peer into an
                // opaque INVALID_REQUEST, losing the upstream semantics AND the
                // tool content. Preserve visibility with an errored Complete.
                if !sep_2322 {
                    let mut out = CallToolResult::default();
                    out.is_error = Some(true);
                    out.content = vec![ContentBlock::text(
                        "upstream requested client input (input_required) — the negotiated protocol version does not support task input; re-negotiate 2026-07-28 or call without requiring input",
                    )];
                    out.meta = result.raw_meta.as_ref().and_then(|m| serde_json::from_value(m.clone()).ok());
                    return CallToolResponse::Complete(out);
                }
                let mut ir = InputRequiredResult::new(input_requests, request_state);
                // Preserve unconsumed raw_meta keys (OTel trace, progress
                // receipts, SEP extensions) instead of dropping them.
                if let Some(obj) = result.raw_meta.as_ref().and_then(|m| m.as_object()) {
                    let leftover: serde_json::Map<String, Value> = obj
                        .iter()
                        .filter(|(k, _)| {
                            !k.ends_with("/resultType")
                                && k.as_str() != "resultType"
                                && !k.ends_with("/inputRequests")
                                && k.as_str() != "inputRequests"
                                && !k.ends_with("/requestState")
                                && k.as_str() != "requestState"
                        })
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    if !leftover.is_empty() {
                        ir.meta = Some(rmcp::model::MetaObject(rmcp::model::JsonObject::from_iter(leftover)));
                    }
                }
                return CallToolResponse::InputRequired(ir);
            }
        }
    }
    let mut out = CallToolResult::default();
    out.content = result
        .content
        .iter()
        .filter_map(|c| match serde_json::from_value::<ContentBlock>(c.clone()) {
            Ok(b) => Some(b),
            Err(e) => {
                // Never silently drop content: an unrecognized block shape
                // (e.g. a new content type folded through the hand-written
                // SSE transport) degrades to a text block so the payload
                // survives to the client.
                log::warn!("[rmcp] unmappable content block degraded to text: {e}");
                Some(ContentBlock::text(c.to_string()))
            }
        })
        .collect();
    // structuredContent postdates the 2024-11-05 spec (e2e TC-07): strip for
    // legacy sessions, exactly as the retired dispatch did.
    out.structured_content = if strip_2024 {
        None
    } else {
        result.structured_content
    };
    out.is_error = Some(result.is_error);
    out.meta = result.raw_meta.and_then(|m| serde_json::from_value(m).ok());
    CallToolResponse::Complete(out)
}

impl HubBridge {
    /// Request-level `_meta` passthrough (MRTR inputResponses retry, OTel).
    /// A downstream client retrying an input_required call sends
    /// `io.modelcontextprotocol/inputResponses` (+ requestState) as typed
    /// params fields — merge them into the upstream `_meta` so the upstream
    /// server can resume (mirrors the dispatch `tools/call` handling).
    fn upstream_meta_from(request: &CallToolRequestParams) -> Value {
        let mut upstream_meta = request
            .meta
            .clone()
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
                obj.insert("io.modelcontextprotocol/requestState".to_string(), json!(rs));
            }
        }
        upstream_meta
    }

    /// Shared execution path for synchronous and task-directed tool calls:
    /// $smart meta tools, target resolution, RAG builtin, disabled checks,
    /// per-session isolation and the shared pool.
    async fn execute_tool_call(
        &self,
        scope: &str,
        tool_name: &str,
        args: Value,
        bearer: Option<&crate::models::bearer_key::BearerKey>,
        upstream_meta: Value,
        session_id: Option<String>,
        source_ip: Option<String>,
        strip_2024: bool,
        sep_2322: bool,
    ) -> Result<CallToolResponse, ErrorData> {
        let scope = scope.to_string();
        let tool_name = tool_name.to_string();
        let scope_clean = scope.trim_start_matches('/').trim().to_string();

        // $smart scopes: intercept the three meta tools. A non-meta name on a
        // smart scope is a pinned tool called directly — fall through to the
        // normal call path below ($smart resolves to all pool servers,
        // $smart/{group} to the group's members, so resolve_target / bearer /
        // disabled gates all apply unchanged). This mirrors origin, where a
        // raw tool name on a smart session routes through the normal path.
        let smart_group_direct = scope_clean == "$smart"
            || scope_clean.starts_with("$smart/");
        let smart_group_direct =
            smart_group_direct && !crate::smart_routing::meta::is_meta_tool(&tool_name);
        // List/call gate symmetry: SR disabled / runtime not ready refuses the
        // whole $smart surface (tools/list and the meta branch both gate on
        // not_ready) — a remembered pin name must not stay callable when the
        // listing is down.
        if smart_group_direct {
            if let Some(msg) = crate::smart_routing::meta::not_ready_message().await {
                return Err(Self::err(msg));
            }
        }
        // Direct calls on a $smart scope must target a pinned tool visible in
        // that scope — the listing only exposes pinned tools, so the call side
        // enforces the same set (list/call parity). Allowed set = group-member
        // pins ∪ server-level pins (group scope), or server-level pins across
        // all bearer-visible servers (root scope).
        if smart_group_direct {
            if Self::resolve_pinned_target(self, &scope, bearer, &tool_name)
                .await
                .is_none()
            {
                return Err(Self::invalid_params(format!(
                    "Tool '{}' is not a pinned tool of this $smart scope",
                    tool_name
                )));
            }
        }
        if crate::smart_routing::meta::is_smart_scope(&scope_clean) && !smart_group_direct {
            if let Some(msg) = crate::smart_routing::meta::not_ready_message().await {
                return Err(Self::err(msg));
            }
            let allowed = {
                let (_, _, scope_allowed) =
                    crate::smart_routing::meta::compute_scope(&scope_clean).await;
                match (
                    scope_allowed,
                    get_allowed_servers(bearer).await,
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
            // Disabled-tool gate for the meta tools themselves (fail-closed):
            // the servers panel can disable smart_route_* like any other
            // builtin tool (they ride on the builtin server's tool list); a
            // filter-store error must refuse the call, not bypass the check.
            let meta_gate = match pool::list_tools_for(crate::rag::service::BUILTIN_SERVER_NAME).await {
                Ok(ts) => {
                    match crate::services::server_tool_config_service::apply_tool_filters(
                        crate::rag::service::BUILTIN_SERVER_NAME,
                        ts,
                    )
                    .await
                    {
                        Ok(filtered) => Ok(filtered
                            .iter()
                            .find(|t| t.name == tool_name)
                            .map(|t| !t.enabled)
                            .unwrap_or(false)),
                        Err(e) => Err(e),
                    }
                }
                Err(e) => Err(anyhow::anyhow!(e.to_string())),
            };
            // Timer BEFORE the meta dispatch so activity duration covers the
            // actual upstream work (self-review: measuring after the await
            // recorded ~0ms for every smart call).
            let started = std::time::Instant::now();
            match meta_gate {
                Ok(true) => {
                    return Err(Self::invalid_params(format!(
                        "Tool '{}' is disabled",
                        tool_name
                    )));
                }
                Err(e) => {
                    log::warn!("[smart] disabled-tool check failed for '{}': {} — refusing call", tool_name, e);
                    return Err(Self::err(format!(
                        "Tool '{}' unavailable (tool filter check failed)",
                        tool_name
                    )));
                }
                Ok(false) => {}
            }
            let result: Result<Value, String> = match tool_name.as_str() {
                "smart_route_search" => {
                    let q = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                    let limit = args.get("limit").cloned().unwrap_or(json!(10));
                    crate::smart_routing::meta::handle_search_tools(q, limit, allowed, group_gate.as_ref()).await
                }
                "smart_route_describe" => {
                    let tn = args.get("toolName").and_then(|v| v.as_str()).unwrap_or("");
                    crate::smart_routing::meta::handle_describe_tool(tn, allowed, group_gate.as_ref()).await
                }
                "smart_route_call" => {
                    let tn = args.get("toolName").and_then(|v| v.as_str()).unwrap_or("");
                    // Pin parity for the meta lane too: the listing only
                    // exposes pinned tools, so routing a toolName through
                    // smart_route_call must not reach an unpinned tool (the
                    // raw-name direct-call lane is already gated above; this
                    // is the same gate applied to the meta wrapper).
                    if Self::resolve_pinned_target(self, &scope, bearer, tn)
                        .await
                        .is_none()
                    {
                        return Err(Self::invalid_params(format!(
                            "Tool '{}' is not a pinned tool of this $smart scope",
                            tn
                        )));
                    }
                    let tool_args = args.get("arguments").cloned().unwrap_or(json!({}));
                    crate::smart_routing::meta::handle_call_tool(tn, tool_args, allowed, group_gate.as_ref()).await
                }
                other => Err(format!(
                    "Unknown smart routing tool '{}'. Available: smart_route_search, smart_route_describe, smart_route_call",
                    other
                )),
            };
            let value = match result {
                Ok(v) => v,
                Err(e) => {
                    let _ = crate::services::log_service::write_activity(
                        "smart",
                        tool_name.as_str(),
                        Some(started.elapsed().as_millis() as i64),
                        "error",
                        Some(args.clone()),
                        None,
                        Some(e.as_str()),
                        source_ip.as_deref(),
                    )
                    .await;
                    return Err(Self::err(e));
                }
            };
            // Meta hop must preserve the same result semantics as the direct
            // path: upstream isError and MRTR input_required (raw_meta) are
            // re-assembled into a ToolCallResult so to_call_response maps them
            // (flattening both into a bare Complete text was a parity break —
            // clients could not see failures or answer input requests).
            let tcr = crate::models::server::ToolCallResult {
                content: value
                    .get("content")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default(),
                is_error: value.get("isError").and_then(|v| v.as_bool()).unwrap_or(false),
                structured_content: value.get("structuredContent").cloned(),
                raw_meta: value.get("_meta").cloned(),
            };
            let _ = crate::services::log_service::write_activity(
                "smart",
                tool_name.as_str(),
                Some(started.elapsed().as_millis() as i64),
                if tcr.is_error { "error" } else { "success" },
                Some(args.clone()),
                serde_json::to_value(&tcr).ok(),
                None,
                source_ip.as_deref(),
            )
            .await;
            return Ok(to_call_response(tcr, strip_2024, sep_2322));
        }

        let resolved = match self.resolve_target(&scope, bearer, &tool_name).await {
            Some(hit) => Some(hit),
            None => {
                // Pool-cache miss fallback: on a $smart scope the tool may be
                // a pin on a sleeping (never-woken) on-demand server whose
                // cached tool list is empty — resolve from the pin registry
                // and let the pool::call_tool path below cold-start it.
                if smart_group_direct {
                    self.resolve_pinned_target(&scope, bearer, &tool_name).await
                } else {
                    None
                }
            }
        };
        let Some((sn, orig_name)) = resolved else {
            // `[tool-not-found]` sentinel: REST callers classify 404 by this
            // stable prefix (prose matching would false-positive on upstream
            // error text that happens to contain the same substrings).
            return Err(Self::invalid_params(format!(
                "[tool-not-found] Tool '{}' not found",
                tool_name
            )));
        };

        // Disabled-tool check — MUST run before the RAG builtin early-return:
        // the servers panel can disable rag_* tools like any other server and
        // the Tauri command path enforces it; without this check the HTTP
        // /mcp path would still list/call disabled builtin tools.
        //
        // ⚠️ NOT fail-open: an empty filter result (store read error) would
        // make find() miss the tool and silently bypass the disable check —
        // an execution-layer fail-open, not just a listing gap. On error the
        // tool is treated as unavailable.
        let disabled_gate = match pool::list_tools_for(&sn).await {
            Ok(ts) => match crate::services::server_tool_config_service::apply_tool_filters(&sn, ts).await {
                Ok(filtered) => {
                    let disabled = match filtered.iter().find(|t| t.name == orig_name) {
                        Some(t) => Ok(!t.enabled),
                        None => {
                            // Absent from the pool cache: benign for
                            // builtin/meta resolution paths, but a smart pin
                            // fallback can resolve a NEVER-WOKEN on-demand
                            // server whose cache is empty — a disabled config
                            // row there would bypass the gate. Consult the
                            // config store directly (fail-closed on error).
                            match crate::services::server_tool_config_service::get_config(
                                &sn, "tool", &orig_name,
                            )
                            .await
                            {
                                Ok(Some(cfg)) => Ok(!cfg.enabled),
                                Ok(None) => Ok(false),
                                Err(e) => Err(anyhow::anyhow!(e.to_string())),
                            }
                        }
                    };
                    disabled
                }
                Err(e) => Err(e),
            },
            Err(e) => Err(anyhow::anyhow!(e.to_string())),
        };
        match disabled_gate {
            Ok(true) => {
                return Err(Self::invalid_params(format!(
                    "Tool '{}' is disabled",
                    orig_name
                )));
            }
            Err(e) => {
                log::warn!("[{}] disabled-tool check failed for '{}': {} — refusing call", sn, orig_name, e);
                return Err(Self::err(format!(
                    "Tool '{}' unavailable (tool filter check failed)",
                    orig_name
                )));
            }
            Ok(false) => {}
        }

        // RAG builtin server: local dispatch, no pool entry.
        if sn == crate::rag::service::BUILTIN_SERVER_NAME {
            let Some(app) = crate::mcp::progress::get_app_handle() else {
                return Err(Self::err("app handle unavailable".to_string()));
            };
            let start = std::time::Instant::now();
            // Timeout parity: every other execution path (shared pool, meta
            // tools) is wrapped — a hung local model call / long-held runtime
            // lock (large reindex under the same mutex) would otherwise block
            // the /mcp call indefinitely (task stuck `working` for background
            // callers with no ttl).
            let result = crate::mcp::time::timeout_tool_call(
                crate::rag::service::call_builtin_tool(&app, &orig_name, &args),
            )
            .await;
            // rmcp migration parity: /mcp tool calls must hit the activity log
            // like the /rest and /api faces (migration dropped this — the
            // Activity page went blind to all /mcp traffic).
            let dur = start.elapsed().as_millis() as i64;
            match &result {
                Ok(r) => {
                    let _ = crate::services::log_service::write_activity(
                        &sn, &orig_name, Some(dur),
                        if r.is_error { "error" } else { "success" },
                        Some(args.clone()),
                        serde_json::to_value(r).ok(), None, source_ip.as_deref(),
                    ).await;
                }
                Err(e) => {
                    let _ = crate::services::log_service::write_activity(
                        &sn, &orig_name, Some(dur), "error",
                        Some(args.clone()), None, Some(&e.to_string()), source_ip.as_deref(),
                    ).await;
                }
            }
            let result = result.map_err(|e| Self::err(e.to_string()))?;
            return Ok(to_call_response(result, strip_2024, sep_2322));
        }

        // Per-session upstream isolation: caller passes rmcp's session id.
        let is_isolated = session_id.is_some() && pool::is_per_session_client(&sn).await;

        if is_isolated {
            let sid = session_id.unwrap();
            let start = std::time::Instant::now();
            let result = crate::mcp::time::timeout_tool_call(
                crate::mcp::session_pool::call_tool_isolated(
                    &sid,
                    &sn,
                    &orig_name,
                    args.clone(),
                ),
            )
            .await;
            let dur = start.elapsed().as_millis() as i64;
            Self::log_call_activity(&sn, &orig_name, dur, &args, result.as_ref(), source_ip.as_deref()).await;
            let result = result.map_err(|e| Self::err(e.to_string()))?;
            return Ok(to_call_response(result, strip_2024, sep_2322));
        }

        let start = std::time::Instant::now();
        let result = crate::mcp::time::timeout_tool_call(pool::call_tool_with_meta(
            &sn,
            &orig_name,
            args.clone(),
            Some(upstream_meta),
        ))
        .await;
        let dur = start.elapsed().as_millis() as i64;
        Self::log_call_activity(&sn, &orig_name, dur, &args, result.as_ref(), source_ip.as_deref()).await;
        let result = result.map_err(|e| Self::err(e.to_string()))?;
        Ok(to_call_response(result, strip_2024, sep_2322))
    }

    /// Activity-log parity helper (see execute_tool_call comment).
    async fn log_call_activity(
        server: &str,
        tool: &str,
        duration_ms: i64,
        args: &Value,
        result: Result<&crate::models::server::ToolCallResult, &anyhow::Error>,
        source_ip: Option<&str>,
    ) {
        match result {
            Ok(r) => {
                let _ = crate::services::log_service::write_activity(
                    server, tool, Some(duration_ms),
                    if r.is_error { "error" } else { "success" },
                    Some(args.clone()),
                    serde_json::to_value(r).ok(), None, source_ip,
                ).await;
            }
            Err(e) => {
                let _ = crate::services::log_service::write_activity(
                    server, tool, Some(duration_ms), "error",
                    Some(args.clone()), None, Some(&e.to_string()), source_ip,
                ).await;
            }
        }
    }

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

    /// Bearer-key parity gate for builtin prompts/resources: aggregate_tools
    /// filters servers by the key's allowed set (builtin included), so the
    /// prompt/resource surfaces must hide builtin content too — otherwise a
    /// restricted key sees no builtin tools but can still read every builtin
    /// prompt template and resource body.
    async fn builtin_visible_for_bearer(
        bearer: Option<&crate::models::bearer_key::BearerKey>,
    ) -> bool {
        match get_allowed_servers(bearer).await {
            None => true,
            Some(set) => set.contains(crate::rag::service::BUILTIN_SERVER_NAME),
        }
    }

    async fn handle_custom_tasks(
        &self,
        method: &str,
        params: Option<Value>,
        stateless: bool,
        modern: bool,
        caller: Option<&str>,
    ) -> Option<Result<CustomResult, rmcp::ErrorData>> {
        let p = params.clone().unwrap_or_default();
        let task_id = p.get("taskId").and_then(|t| t.as_str()).unwrap_or("").to_string();
        let perr = |code: i32, msg: String| {
            rmcp::ErrorData::new(ErrorCode(code), msg, None)
        };
        match method {
            // `tasks/get` never reaches this custom path — rmcp 3.4.1 parses
            // it as a typed GetTaskRequest routed to ServerHandler::get_task.
            "tasks/result" => {
                if stateless {
                    return Some(Err(rmcp::ErrorData::new(ErrorCode::METHOD_NOT_FOUND, "tasks/result".to_string(), None)));
                }
                Some(match crate::services::mcp_tasks::result(&task_id, caller, modern).await {
                    Ok(r) => Ok(CustomResult::new(r)),
                    Err((code, msg)) => Err(perr(code as i32, msg)),
                })
            }
            "tasks/list" => {
                if stateless {
                    return Some(Err(rmcp::ErrorData::new(ErrorCode::METHOD_NOT_FOUND, "tasks/list".to_string(), None)));
                }
                Some(Ok(CustomResult::new(
                    crate::services::mcp_tasks::list_all(caller, modern).await,
                )))
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
            // Hub pushes resource updates (subscription_hub::notify_resource_updated);
            // without `subscribe` rmcp strips resourceSubscriptions from every
            // subscriptions/listen honored filter (supported_by caps gate).
            .enable_resources_subscribe()
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

    // Discover answers static server info: cacheable for 1h and shareable
    // across clients (rmcp's from_server_info default is ttl=0/priv — that
    // would defeat the 2026 discovery cache contract, see doc/upgrade notes).
    async fn discover(
        &self,
        _context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::DiscoverResult, ErrorData> {
        Ok(rmcp::model::DiscoverResult::from_server_info(
            self.supported_protocol_versions().into_owned(),
            self.get_info(),
        )
        .with_ttl_ms(3_600_000)
        .with_cache_scope(rmcp::model::CacheScope::Public))
    }

    // Capabilities don't advertise completions — per spec, unadvertised
    // request methods answer -32601 instead of the SDK's empty-result default
    // (which would silently pretend the feature exists).
    async fn complete(
        &self,
        _request: CompleteRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, ErrorData> {
        Err(ErrorData::method_not_found::<CompleteRequestMethod>())
    }
    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        remember_native_peer(context.peer).await;
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let started = std::time::Instant::now();
        let bearer = Self::bearer_from_ctx(&context).await?;
        let scope = Self::scope_from_ctx(&context);
        let scope_clean = scope.trim_start_matches('/').trim().to_string();
        let tools = self
            .aggregate_tools(&scope, bearer.as_ref(), Self::is_2024_session(&context))
            .await?;
        let result = ListToolsResult::with_all_items(tools);
        Ok(if Self::is_2026_session(&context) {
            // Origin #1277 parity: a positive list TTL requires recorded
            // freshness from every participating upstream, capped at 5s minus
            // projection elapsed. $smart (gateway-generated meta tools) and
            // builtin-only scopes never advertise a positive TTL.
            let ttl = if crate::smart_routing::meta::is_smart_scope(&scope_clean) {
                0
            } else {
                let upstreams: Vec<String> = self
                    .scope_filters(&scope, bearer.as_ref())
                    .await
                    .into_iter()
                    .filter(|sf| sf.name != crate::rag::service::BUILTIN_SERVER_NAME)
                    .map(|sf| sf.name)
                    .collect();
                crate::services::list_freshness::remaining(
                    &upstreams,
                    "tools",
                    started.elapsed().as_millis() as u64,
                )
            };
            result
                .with_ttl_ms(ttl)
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
        let tool_name = request.name.to_string();
        let strip_2024 = Self::is_2024_session(&context);
        let args = Value::Object(request.arguments.clone().unwrap_or_default());

        // SEP-2663 client-directed task: the leniency middleware surfaces the
        // spec-level `params.task` (dropped by rmcp's typed params) as an
        // internal header. Materialize a task, run the real call in the
        // background, and return the CreateTaskResult handle immediately.
        let task_spec = context
            .extensions
            .get::<http::request::Parts>()
            .and_then(|p| p.headers.get("x-mcphub-task-requested"))
            .and_then(|v| v.to_str().ok())
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .filter(|t| t.is_object());
        if let Some(task_spec) = task_spec {
            // $smart scope: tasks/get|result|list are METHOD_NOT_FOUND there
            // (global task store unreachable) — creating a task would hand the
            // client a handle it can never poll, so tasking is rejected and
            // the call does NOT fall back to synchronous execution (review
            // round 9: prior comment wrongly claimed a plain-call fallback).
            if crate::smart_routing::meta::is_smart_scope(scope.trim_start_matches('/').trim()) {
                return Err(ErrorData::new(
                    ErrorCode::METHOD_NOT_FOUND,
                    "tasks are not available on the $smart scope".to_string(),
                    None,
                ));
            }
            let ttl = task_spec
                .get("ttl")
                .and_then(|v| v.as_u64())
                .or_else(|| task_spec.get("ttlMs").and_then(|v| v.as_u64()));
            let task_id =
                match crate::services::mcp_tasks::create(ttl, bearer.as_ref().map(|k| k.id.clone())).await {
                    Ok(id) => id,
                    Err(msg) => {
                        return Err(rmcp::ErrorData::new(
                            rmcp::model::ErrorCode::INTERNAL_ERROR,
                            msg,
                            None,
                        ));
                    }
                };
            let task_id_bg = task_id.clone();
            let created = chrono::Utc::now().to_rfc3339();
            let this = Self;
            let scope = scope.clone();
            let tool_name = tool_name.clone();
            let bearer_ref = bearer.clone();
            let upstream_meta = Self::upstream_meta_from(&request);
            let session_id_bg = context
                .extensions
                .get::<http::request::Parts>()
                .and_then(|p| {
                    p.headers
                        .get("mcp-session-id")
                        .and_then(|v| v.to_str().ok())
                        .map(|s| s.to_string())
                })
                .filter(|s| !s.is_empty());
            // Task-directed calls still go through execute_tool_call for the
            // activity log — keep source_ip parity with the synchronous path.
            let source_ip_bg = context
                .extensions
                .get::<http::request::Parts>()
                .and_then(|p| crate::services::http_server::client_ip_of(&p.headers));
            let strip_2024_bg = strip_2024;
            let sep_2322_bg = Self::is_2026_session(&context);
            tokio::spawn(async move {
                // catch_unwind: a panicking execute_tool_call must not leave
                // the task stuck in `working` forever (with no TTL the sweeper
                // would never reap it and the client would poll indefinitely).
                let outcome = std::panic::AssertUnwindSafe(
                    this.execute_tool_call(&scope, &tool_name, args, bearer_ref.as_ref(), upstream_meta, session_id_bg, source_ip_bg, strip_2024_bg, sep_2322_bg),
                )
                .catch_unwind()
                .await
                .unwrap_or_else(|p| {
                    let msg = p
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| p.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "panic in background tool call".to_string());
                    Err(rmcp::ErrorData::new(
                        rmcp::model::ErrorCode::INTERNAL_ERROR,
                        format!("background task panicked: {msg}"),
                        None,
                    ))
                });
                match outcome {
                    // InputRequired payloads are stored as the task result and
                    // delivered in a TERMINAL completed state: the client reads
                    // them via tasks/result, then starts a NEW tools/call with
                    // `inputResponses` in `_meta`. There is no tasks/update
                    // reactivation path — update_input on a terminal task
                    // returns a clear error (see mcp_tasks::update_input).
                    Ok(resp) => {
                        // CallToolResponse is not Serialize; shape via the
                        // model's own conversion into a ServerResult JSON.
                        let shaped = match resp {
                            CallToolResponse::Complete(cr) => {
                                serde_json::to_value(&cr).unwrap_or_else(|_| json!({}))
                            }
                            CallToolResponse::InputRequired(ir) => {
                                serde_json::to_value(&ir).unwrap_or_else(|_| json!({}))
                            }
                            // Non-exhaustive enum: unknown future variants have
                            // no serializable shape — record an empty result.
                            _ => json!({}),
                        };
                        crate::services::mcp_tasks::complete(&task_id_bg, shaped).await;
                    }
                    Err(e) => {
                        crate::services::mcp_tasks::fail(&task_id_bg, e.message.to_string()).await;
                    }
                }
            });
            let mut task = rmcp::model::Task::new(
                task_id.clone(),
                rmcp::model::TaskStatus::Working,
                created.clone(),
                created,
            );
            if let Some(ttl) = ttl {
                task = task.with_ttl_ms(ttl);
            }
            return Ok(CallToolResponse::Task(rmcp::model::CreateTaskResult::new(task)));
        }
        self.execute_tool_call(
            &scope,
            &tool_name,
            args,
            bearer.as_ref(),
            Self::upstream_meta_from(&request),
            context
                .extensions
                .get::<http::request::Parts>()
                .and_then(|p| {
                    p.headers
                        .get("mcp-session-id")
                        .and_then(|v| v.to_str().ok())
                        .map(|s| s.to_string())
                })
                .filter(|s| !s.is_empty()),
            context
                .extensions
                .get::<http::request::Parts>()
                .and_then(|p| crate::services::http_server::client_ip_of(&p.headers)),
            strip_2024,
            Self::is_2026_session(&context),
        )
        .await
    }

    // ── Prompts (builtin "mcphub-desktop" server only) ──────────────────

    // Legacy resources/subscribe: we advertise the capability (for the 2026
    // resourceSubscriptions filter), so honor it for 2025 clients instead of
    // rmcp's method_not_found default.
    async fn subscribe(
        &self,
        request: rmcp::model::SubscribeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), rmcp::ErrorData> {
        legacy_resource_subscribe(context.peer.clone(), request.uri.to_string()).await;
        Ok(())
    }

    async fn unsubscribe(
        &self,
        request: rmcp::model::UnsubscribeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), rmcp::ErrorData> {
        legacy_resource_unsubscribe(&context.peer, request.uri.as_ref()).await;
        Ok(())
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, rmcp::ErrorData> {
        let bearer = Self::bearer_from_ctx(&context).await?;
        let scope = Self::scope_from_ctx(&context);
        if crate::smart_routing::meta::is_smart_scope(scope.trim_start_matches('/').trim()) {
            let result = ListPromptsResult::with_all_items(Vec::new());
            // Origin #1277 parity: gateway-generated lists (prompts/resources/
            // $smart meta) never advertise a positive TTL — ttlMs 0.
            return Ok(if Self::is_2026_session(&context) {
                result
                    .with_ttl_ms(0)
                    .with_cache_scope(rmcp::model::CacheScope::Private)
            } else {
                result
            });
        }
        if !Self::builtin_visible_for_bearer(bearer.as_ref()).await {
            let result = ListPromptsResult::with_all_items(Vec::new());
            return Ok(result);
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
                .with_ttl_ms(0)
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
        // Bearer auth must be evaluated before any scope shortcut so an
        // invalid key surfaces 401 rather than a silent "not found" (auth
        // errors must not degrade to anonymous — round-9 F-3 parity).
        let bearer = Self::bearer_from_ctx(&context).await?;
        if !Self::builtin_visible_for_bearer(bearer.as_ref()).await {
            return Err(Self::invalid_params(format!("Prompt '{}' not found", name)));
        }
        // $smart scope: builtin prompts are not part of the smart meta-tool
        // surface — list_prompts returns empty here, so get must not serve
        // content the list hides (list/read gating parity).
        if crate::smart_routing::meta::is_smart_scope(scope.trim_start_matches('/').trim()) {
            return Err(Self::invalid_params(format!("Prompt '{}' not found", name)));
        }
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
                // Required-arg validation: a missing arg would otherwise leak
                // the literal {{placeholder}} into LLM-bound content.
                crate::services::prompt_service::validate_required_args(&p.arguments, &args)
                    .map_err(Self::invalid_params)?;
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
        // Bearer auth must be evaluated before any scope shortcut so an
        // invalid key surfaces 401 rather than a silent empty list.
        let bearer = Self::bearer_from_ctx(&context).await?;
        if !Self::builtin_visible_for_bearer(bearer.as_ref()).await {
            let result = ListResourcesResult::with_all_items(Vec::new());
            return Ok(result);
        }
        if crate::smart_routing::meta::is_smart_scope(scope.trim_start_matches('/').trim()) {
            let result = ListResourcesResult::with_all_items(Vec::new());
            return Ok(if Self::is_2026_session(&context) {
                result
                    .with_ttl_ms(0)
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
                .with_ttl_ms(0)
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
        // Bearer auth must be evaluated before any scope shortcut so an
        // invalid key surfaces 401 rather than a silent "not found".
        let bearer = Self::bearer_from_ctx(&context).await?;
        if !Self::builtin_visible_for_bearer(bearer.as_ref()).await {
            return Err(Self::invalid_params(format!("Resource '{}' not found", uri)));
        }
        // $smart scope: builtin resources are not part of the smart meta-tool
        // surface — list_resources returns empty here, so read must not serve
        // content the list hides (list/read gating parity).
        if crate::smart_routing::meta::is_smart_scope(scope.trim_start_matches('/').trim()) {
            return Err(Self::invalid_params(format!("Resource '{}' not found", uri)));
        }
        let resource_sel = self.builtin_resource_selection(&scope).await;
        let resource = crate::services::resource_service::list_all()
            .await
            .ok()
            .and_then(|rs| {
                rs.into_iter()
                    .find(|r| r.enabled && r.uri == uri && builtin_allowed(&resource_sel, &r.uri))
            });
        match resource {
            Some(r) => {
                let result: ReadResourceResponse = ReadResourceResult::new(vec![ResourceContents::TextResourceContents {
                    uri: r.uri,
                    mime_type: Some(r.mime_type),
                    text: r.content,
                    meta: None,
                }])
                .into();
                // CacheableResult contract (2026-07-28): resources/read is a
                // cacheable list-shaped read like prompts/get — without
                // ttlMs/cacheScope a 2026 client never caches the content and
                // every read hits the hub again.
                Ok(if Self::is_2026_session(&context) {
                    match result {
                        ReadResourceResponse::Complete(res) => {
                            ReadResourceResponse::Complete(
                                res.with_ttl_ms(0)
                                    .with_cache_scope(rmcp::model::CacheScope::Private),
                            )
                        }
                        other => other,
                    }
                } else {
                    result
                })
            }
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
        // Auth-first (round-9 F-3 parity): evaluate bearer before any scope
        // shortcut so auth errors never degrade to a scope-gate message.
        let caller = Self::bearer_from_ctx(&context).await?.map(|k| k.id);
        // $smart scope has its own meta-tool surface — the global task store is
        // not reachable there (same gate as custom tasks/result|list paths).
        if crate::smart_routing::meta::is_smart_scope(scope.trim_start_matches('/').trim()) {
            return Err(ErrorData::new(
                ErrorCode::METHOD_NOT_FOUND,
                "tasks are not available on the $smart scope".to_string(),
                None,
            ));
        }
        // The hub's task store produces the wire shape directly (2026 ext or
        // 2025-11 snapshot); rmcp's DetailedTask cannot express both, so the
        // typed tasks/get serves rmcp-native clients from the same store via
        // serde round-trip when possible. Version split: legacy (2025-11)
        // sessions must receive the legacy field names (pollInterval/ttl) —
        // get_ext always emits 2026 names (pollIntervalMs/ttlMs), which a
        // legacy client would read as missing fields. Same modern gate as the
        // custom tasks/result|list paths.
        let modern = Self::is_2026_session(&context);
        let fetched = if modern {
            crate::services::mcp_tasks::get_ext(&request.task_id, caller.as_deref()).await
        } else {
            crate::services::mcp_tasks::get(&request.task_id, caller.as_deref()).await
        };
        match fetched {
            Some(mut t) => {
                // ⚠️ Typed round-trip limitation: rmcp 3.4.1's `Task` struct has
                // no legacy aliases (`ttl`/`pollInterval`) — a serde
                // from_value::<GetTaskResult> round-trip silently DROPS the
                // legacy field names and re-serializes `ttlMs: null`. Rename the
                // legacy keys to their modern counterparts BEFORE the round-trip
                // so the values survive (a legacy client reading typed tasks/get
                // sees the modern names it ignores; the authoritative legacy wire
                // shape remains tasks/result, which keeps pollInterval/ttl).
                if let Some(map) = t.as_object_mut() {
                    if let Some(ttl) = map.remove("ttl") {
                        map.entry("ttlMs".to_string()).or_insert(ttl);
                    }
                    if let Some(pi) = map.remove("pollInterval") {
                        map.entry("pollIntervalMs".to_string()).or_insert(pi);
                    }
                }
                serde_json::from_value::<GetTaskResult>(t)
                    .map_err(|e| Self::err(format!("task shape mismatch: {e}")))
            }
            None => Err(Self::invalid_params(format!(
                "Task '{}' not found",
                request.task_id
            ))),
        }
    }

    async fn update_task(
        &self,
        request: UpdateTaskParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), rmcp::ErrorData> {
        // Same $smart gate as get_task (global task store is not reachable on
        // the $smart scope). Auth-first: bearer before scope shortcut.
        let scope = Self::scope_from_ctx(&context);
        let caller = Self::bearer_from_ctx(&context).await?.map(|k| k.id);
        if crate::smart_routing::meta::is_smart_scope(scope.trim_start_matches('/').trim()) {
            return Err(ErrorData::new(
                ErrorCode::METHOD_NOT_FOUND,
                "tasks are not available on the $smart scope".to_string(),
                None,
            ));
        }
        crate::services::mcp_tasks::update_input(
            &request.task_id,
            serde_json::to_value(request.input_responses).unwrap_or_default(),
            caller.as_deref(),
        )
        .await
        .map_err(|(code, msg)| rmcp::ErrorData::new(ErrorCode(code), msg, None))
    }

    async fn cancel_task(
        &self,
        request: CancelTaskParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), rmcp::ErrorData> {
        // Same $smart gate as get_task (global task store is not reachable on
        // the $smart scope). Auth-first: bearer before scope shortcut.
        let scope = Self::scope_from_ctx(&context);
        let caller = Self::bearer_from_ctx(&context).await?.map(|k| k.id);
        if crate::smart_routing::meta::is_smart_scope(scope.trim_start_matches('/').trim()) {
            return Err(ErrorData::new(
                ErrorCode::METHOD_NOT_FOUND,
                "tasks are not available on the $smart scope".to_string(),
                None,
            ));
        }
        crate::services::mcp_tasks::cancel(&request.task_id, caller.as_deref())
            .await
            .map(|_| ())
            .map_err(|(code, msg)| rmcp::ErrorData::new(ErrorCode(code), msg, None))
    }

    // ─── 2026-07-28 subscriptions/listen (rmcp-native) ────────────────────
    //
    // The hub advertises listChanged for tools/prompts/resources in get_info
    // and pushes resource updates via subscription_hub, so the accepted
    // filter is simply the requested categories intersected with what this
    // server can deliver (all four). Returning None here would leave
    // subscriptions/listen unimplemented (rmcp default).

    fn accepted_subscription_filter(
        &self,
        requested: &rmcp::model::SubscriptionFilter,
    ) -> Option<rmcp::model::SubscriptionFilter> {
        // Advertise everything this hub can deliver: the three list_changed
        // lanes (declared in get_info) plus any requested resource URI (the
        // hub pushes resource updates for arbitrary URIs). The intersection
        // yields exactly the requested categories.
        let requested_uris = requested.resource_subscriptions.clone().unwrap_or_default();
        let advertised = rmcp::model::SubscriptionFilter::builder()
            .tools_list_changed()
            .prompts_list_changed()
            .resources_list_changed()
            .resource_subscriptions(requested_uris)
            .build();
        Some(advertised.intersection(requested))
    }

    async fn listen(
        &self,
        context: rmcp::service::SubscriptionContext,
    ) -> Result<(), rmcp::ErrorData> {
        use crate::services::subscription_hub::HubEvent;
        use rmcp::model::{ResourceUpdatedNotification, ResourceUpdatedNotificationParam, ServerNotification};
        let sink = context.sink().clone();
        // Local URI gate: the hub bus broadcasts resource updates for ALL uris
        // to every listener, but this sink's accepted filter only carries the
        // uris THIS subscriber requested — an out-of-filter send returns
        // Err(NotificationNotAccepted), which must NOT terminate the stream
        // (an unrelated client's resource update would otherwise kill this
        // subscription). Pre-filter locally and only skip the event.
        let accepted_uris = sink
            .accepted()
            .resource_subscriptions
            .clone()
            .unwrap_or_default();
        // Symmetric lane pre-filter: the sink's accepted filter is the
        // INTERSECTION of advertised and requested lanes — a client that
        // subscribed to a single lane must not receive the others (their send
        // would return Err(NotificationNotAccepted), which must NOT terminate
        // the stream — it is filter rejection, not transport death).
        let accepted_lanes = sink.accepted().clone();
        let lane_ok = |lane: Option<bool>| lane != Some(false);
        let mut rx = crate::services::subscription_hub::subscribe_events();
        loop {
            tokio::select! {
                _ = context.cancelled() => return Ok(()),
                ev = rx.recv() => match ev {
                    Ok(ev) => {
                        // The sink offers typed helpers for the three
                        // list_changed lanes; resource updates build the
                        // notification directly.
                        let result = match ev {
                            HubEvent::ToolsListChanged => {
                                if !lane_ok(accepted_lanes.tools_list_changed) { continue; }
                                sink.notify_tool_list_changed().await
                            }
                            HubEvent::PromptsListChanged => {
                                if !lane_ok(accepted_lanes.prompts_list_changed) { continue; }
                                sink.notify_prompt_list_changed().await
                            }
                            HubEvent::ResourcesListChanged => {
                                if !lane_ok(accepted_lanes.resources_list_changed) { continue; }
                                sink.notify_resource_list_changed().await
                            }
                            HubEvent::ResourceUpdated(uri) => {
                                if !accepted_uris.contains(&uri) {
                                    continue;
                                }
                                sink.send(ServerNotification::ResourceUpdatedNotification(
                                    ResourceUpdatedNotification::new(ResourceUpdatedNotificationParam::new(uri)),
                                )).await
                            }
                            // Task status push rides the tasks extension, not the
                            // core subscription sink (filter has no taskIds lane).
                            HubEvent::TaskStatus(_) => continue,
                        };
                        // A send failure here means the stream itself is gone
                        // (client disconnected / transport closed) — terminate
                        // this forwarder. Filter rejections
                        // (NotificationNotAccepted / UnsupportedNotification)
                        // are NOT transport errors: skip the event and keep
                        // the subscription alive.
                        if result.is_err() {
                            use rmcp::service::SubscriptionSendError;
                            let e = result.unwrap_err();
                            match e {
                                SubscriptionSendError::NotificationNotAccepted(_)
                                | SubscriptionSendError::UnsupportedNotification(_) => continue,
                                _ => return Ok(()),
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // Dropped notifications: the client's CacheableResult
                        // ttl (30s) self-heals the cache, but record the loss.
                        crate::services::app_logger::log_to_db(
                            "warn",
                            &format!("[rmcp] subscription listener lagged, {} notifications dropped", n),
                        );
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return Ok(()),
                }
            }
        }
    }

    // tasks/result, tasks/list ride the custom path (tasks/get is typed).

    async fn on_custom_request(
        &self,
        request: CustomRequest,
        _context: RequestContext<RoleServer>,
    ) -> Result<CustomResult, rmcp::ErrorData> {
        let scope = Self::scope_from_ctx(&_context);
        // Stateless gate: $smart scope only. 2026 clients DO get tasks/get|
        // result|list|cancel (modern field names per the R158 fix); the 2026
        // extension redesign did not delete them.
        let stateless = crate::smart_routing::meta::is_smart_scope(scope.trim_start_matches('/').trim());
        // 2026-07-28 sessions speak the extension task field names
        // (pollIntervalMs/ttlMs) in tasks/list and working-state tasks/result
        // snapshots — legacy sessions keep the 2025-11 core names.
        let modern = Self::is_2026_session(&_context);
        // Auth error (not "no key") propagates rather than downgrading to
        // anonymous caller (review round 9 F-3).
        let caller = match Self::bearer_from_ctx(&_context).await {
            Ok(k) => k.map(|key| key.id),
            Err(e) => return Err(e),
        };
        if let Some(r) = self
            .handle_custom_tasks(&request.method, request.params.clone(), stateless, modern, caller.as_deref())
            .await
        {
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
        // Wait for the server side to register our peer (NATIVE_PEERS) by
        // polling instead of a fixed sleep — a slow CI machine could miss the
        // registration window entirely and silently drop all three notifies.
        // Registration happens on notifications/initialized, so a no-op
        // list_changed probe arriving on the peer means we're registered.
        let reg_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let registered = match NATIVE_PEERS.get() {
                Some(rw) => {
                    let peers = rw.read().await;
                    !peers.is_empty()
                }
                None => false,
            };
            if registered
                || tokio::time::Instant::now() >= reg_deadline
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }

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

        // >= 1 not == 1: NATIVE_PEERS is process-global and cargo runs test
        // modules in parallel — subscription_hub's notify tests can fan out
        // extra notifications to this probe concurrently (exact-count would
        // flake).
        assert!(client.tool_changes.load(Ordering::SeqCst) >= 1, "tools/list_changed");
        assert!(client.prompt_changes.load(Ordering::SeqCst) >= 1, "prompts/list_changed");
        assert!(client.resource_changes.load(Ordering::SeqCst) >= 1, "resources/list_changed");
        client_service.cancel().await;
    }
}
