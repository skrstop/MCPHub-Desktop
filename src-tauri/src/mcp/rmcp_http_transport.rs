//! rmcp-backed Streamable HTTP transport — official rmcp client with the
//! hub's upstream behaviours preserved:
//!
//! - **Auto lifecycle**: rmcp probes `server/discover` first (2026 modern,
//!   stateless) and falls back to the legacy `initialize` handshake — exactly
//!   the A1 probe logic of the old transport, now SDK-native.
//! - **Custom headers / Authorization** from the server config travel on every
//!   request via `StreamableHttpClientTransportConfig`.
//! - **MRTR** (SEP-2322): two paths, mirroring the old transport's contract —
//!   ① downstream-driven retry: request `_meta` carrying
//!   `io.modelcontextprotocol/inputResponses` + `requestState` is applied to
//!   the upstream params verbatim (`call_tool_once`, no local retry);
//!   ② hub-driven retry: `Peer::call_tool` drives the input_required
//!   round-trips with bounded retries. NOTE: our client handler is empty, so
//!   an upstream input request (elicitation/sampling) is DENIED and the call
//!   converges to an error after the bounded rounds — it is not fulfilled.
//! - **Tasks** (SEP-2663): a `CallToolResponse::Task` is polled via
//!   `tasks/get` to a terminal state (old `finish_modern_call` behaviour).
//! - **raw_meta passthrough** (A2/A9): upstream `_meta` travels back to the
//!   downstream client (MRTR contract shapes, OTel trace keys).

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, GetTaskParams, ProtocolVersion, RequestMetaObject,
};
use rmcp::service::{ClientLifecycleMode, RoleClient, RunningService};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use serde_json::{json, Value};

use crate::mcp::client::McpTransport;
use crate::models::server::{Tool, ToolCallResult};
use crate::services::app_logger;

/// Client handler for the upstream rmcp connection — forwards
/// `elicitation/create` to the downstream client when a per-call relay is
/// registered (isolated perSessionClient calls, origin #1370). See
/// rmcp_stdio_transport::BridgeClientHandler for the full contract.
struct BridgeClientHandler {
    elicitation: crate::mcp::elicitation::ElicitationSlot,
    /// Connection-lifetime URL-mode completion forwarder (survives the call).
    elicitation_notify: crate::mcp::elicitation::ElicitNotifySlot,
}

impl BridgeClientHandler {
    /// Slots are created by the OWNING transport and shared here — the
    /// session pool registers relays via the transport's accessors, so the
    /// handler MUST read the same Arcs (separate slots would silently
    /// disable all forwarding).
    fn new(
        elicitation: crate::mcp::elicitation::ElicitationSlot,
        elicitation_notify: crate::mcp::elicitation::ElicitNotifySlot,
    ) -> Self {
        Self {
            elicitation,
            elicitation_notify,
        }
    }
}

impl rmcp::ClientHandler for BridgeClientHandler {
    fn get_info(&self) -> rmcp::model::ClientConfig {
        let mut info = rmcp::model::ClientConfig::default();
        info.capabilities.elicitation = Some(
            rmcp::model::ElicitationCapability::new()
                .with_form(rmcp::model::FormElicitationCapability::new())
                .with_url(rmcp::model::UrlElicitationCapability::new()),
        );
        info
    }

    fn create_elicitation(
        &self,
        request: rmcp::model::ElicitRequestParams,
        _context: rmcp::service::RequestContext<RoleClient>,
    ) -> impl std::future::Future<Output = Result<rmcp::model::ElicitResult, rmcp::ErrorData>> + Send + '_ {
        let relay = crate::mcp::elicitation::take_active(&self.elicitation);
        async move {
            match relay {
                None => Err(crate::mcp::elicitation::elicitation_error(
                    crate::mcp::elicitation::NO_ACTIVE_TOOL_CALL_MSG,
                )),
                Some(relay) => relay.forward(request).await,
            }
        }
    }

    fn on_custom_notification(
        &self,
        notification: rmcp::model::CustomNotification,
        _context: rmcp::service::NotificationContext<RoleClient>,
    ) -> impl std::future::Future<Output = ()> + Send + '_ {
        let relay = crate::mcp::elicitation::take_active(&self.elicitation);
        // Connection-lifetime fallback: URL-mode completions commonly arrive
        // AFTER the tool call returned (the user is still on the auth page) —
        // origin installs its completion handler for the client's lifetime,
        // so mirror that instead of dropping outside the per-call window.
        let lifetime_notify = relay
            .as_ref()
            .and_then(|r| r.notify_fn())
            .or_else(|| {
                self.elicitation_notify
                    .lock()
                    .ok()
                    .and_then(|g| g.clone())
            });
        async move {
            if notification.method == "notifications/elicitation/complete" {
                match lifetime_notify {
                    Some(notify) => notify(notification.params),
                    None => log::debug!(
                        "[{}] dropping notifications/elicitation/complete (no downstream owner)",
                        "rmcp-http"
                    ),
                }
            }
        }
    }
}

fn request_meta_from_value(meta: Option<Value>) -> Option<RequestMetaObject> {
    meta.and_then(|m| serde_json::from_value(m).ok())
}

/// Extract downstream-driven MRTR retry fields from the request `_meta`.
/// Returns `(input_responses, request_state)` when present.
fn downstream_mrtr_retry(meta: &Value) -> Option<(std::collections::BTreeMap<String, Value>, Option<String>)> {
    let obj = meta.as_object()?;
    let ir = obj
        .get("io.modelcontextprotocol/inputResponses")
        .or_else(|| obj.get("inputResponses"))?;
    let rs = obj
        .get("io.modelcontextprotocol/requestState")
        .or_else(|| obj.get("requestState"))
        .and_then(|v| v.as_str().map(|s| s.to_string()));
    ir.as_object().map(|m| {
        (
            m.iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<std::collections::BTreeMap<String, Value>>(),
            rs,
        )
    })
}

pub struct RmcpHttpTransport {
    server_name: String,
    url: String,
    headers: HashMap<String, String>,
    service: Option<RunningService<RoleClient, BridgeClientHandler>>,
    /// Per-connection elicitation relay slot (origin #1370).
    elicitation_slot: crate::mcp::elicitation::ElicitationSlot,
    elicitation_notify_slot: crate::mcp::elicitation::ElicitNotifySlot,
    connected: std::sync::atomic::AtomicBool,
    /// True when the upstream answered `server/discover` (2026 modern,
    /// stateless) — logged for the activity panel, rmcp drives the difference.
    modern: bool,
    /// Upstream protocol negotiation (origin #1382): when true
    /// (`protocolNegotiation: 'legacy'`), skip the `server/discover` probe and
    /// go straight to the legacy initialize handshake.
    legacy_only: bool,
    server_version: Option<String>,
    protocol_version: Option<String>,
    /// Upstream-advertised TTL from the most recent list_tools fetch
    /// (only positive single-page TTLs recorded — origin #1277 freshness).
    last_list_ttl: std::sync::Mutex<Option<u64>>,
}

impl RmcpHttpTransport {
    pub fn new(
        server_name: impl Into<String>,
        url: impl Into<String>,
        headers: HashMap<String, String>,
    ) -> Self {
        Self::with_protocol_mode(server_name, url, headers, false)
    }

    /// `legacy_only` mirrors origin #1382 `protocolNegotiation: 'legacy'` —
    /// skip the modern `server/discover` probe entirely.
    pub fn with_protocol_mode(
        server_name: impl Into<String>,
        url: impl Into<String>,
        headers: HashMap<String, String>,
        legacy_only: bool,
    ) -> Self {
        Self {
            server_name: server_name.into(),
            url: url.into(),
            headers,
            service: None,
            elicitation_slot: crate::mcp::elicitation::new_slot(),
            elicitation_notify_slot: crate::mcp::elicitation::new_notify_slot(),
            connected: std::sync::atomic::AtomicBool::new(false),
            modern: false,
            legacy_only,
            server_version: None,
            protocol_version: None,
            last_list_ttl: std::sync::Mutex::new(None),
        }
    }

    /// Poll a task handle to a terminal state via `tasks/get` (mirrors the
    /// old `finish_modern_call` loop: 10 min deadline, upstream pollInterval).
    async fn poll_task_to_terminal(
        &self,
        task_id: String,
        poll_interval_ms: u64,
    ) -> Result<ToolCallResult> {
        let service = self
            .service
            .as_ref()
            .ok_or_else(|| anyhow!("[{}] not connected", self.server_name))?;
        let poll_interval = poll_interval_ms.clamp(200, 30_000);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
        let mut transient_failures = 0u32;
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(poll_interval)).await;
            // Recheck deadline AFTER the sleep — a clamped 30s sleep must
            // not overshoot the deadline check.
            if std::time::Instant::now() > deadline {
                return Err(anyhow!("modern task '{}' poll timed out (10min)", task_id));
            }
            // Bounded tolerance for transient network errors: a single
            // blip must not discard a task that is still running upstream
            // (review round 9). Three consecutive failures = real failure.
            let snap = match service
                .peer()
                .get_task(GetTaskParams::new(task_id.clone()))
                .await
            {
                Ok(s) => {
                    transient_failures = 0;
                    s
                }
                Err(e) => {
                    transient_failures += 1;
                    if transient_failures >= 3 {
                        self.mark_conn_loss(&e);
                        return Err(anyhow!("tasks/get failed: {}", e));
                    }
                    continue;
                }
            };
            match &snap.task.payload {
                rmcp::model::TaskPayload::Working => continue,
                rmcp::model::TaskPayload::InputRequired { input_requests } => {
                    let mut raw = serde_json::to_value(&snap)?;
                    if let Some(obj) = raw.as_object_mut() {
                        obj.insert(
                            "io.modelcontextprotocol/inputRequests".to_string(),
                            serde_json::to_value(input_requests).unwrap_or(json!([])),
                        );
                        // GetTaskResult serializes resultType:"complete"
                        // (flattened DetailedTask); override it so the bridge's
                        // to_call_response MRTR gate
                        // (resultType == "input_required") fires and the
                        // response is promoted to InputRequired for 2026
                        // clients — same as the direct path below.
                        obj.insert(
                            "io.modelcontextprotocol/resultType".to_string(),
                            json!("input_required"),
                        );
                    }
                    return Ok(ToolCallResult {
                        content: vec![json!({"type":"text","text":"task requires input"})],
                        is_error: false,
                        structured_content: None,
                        raw_meta: Some(raw),
                    });
                }
                rmcp::model::TaskPayload::Completed { result } => {
                    let content = result
                        .get("content")
                        .and_then(|c| c.as_array())
                        .cloned()
                        .unwrap_or_default();
                    let is_error = result
                        .get("isError")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let structured_content =
                        result.get("structuredContent").cloned().filter(|v| !v.is_null());
                    let raw_meta = result.get("_meta").cloned().filter(|v| !v.is_null());
                    return Ok(ToolCallResult { content, is_error, structured_content, raw_meta });
                }
                rmcp::model::TaskPayload::Failed { error } => {
                    let msg = error
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("upstream task failed");
                    return Err(anyhow!("modern task failed: {}", msg));
                }
                rmcp::model::TaskPayload::Cancelled => {
                    return Err(anyhow!("modern task '{}' was cancelled", task_id));
                }
                _ => return Err(anyhow!("unknown task status")),
            }
        }
    }
}

#[async_trait]
impl McpTransport for RmcpHttpTransport {
    async fn connect(&mut self) -> Result<()> {
        let connect_start = std::time::Instant::now();
        let conn_msg = format!(
            "[{}] Connecting to HTTP endpoint: {}",
            self.server_name, self.url
        );
        log::info!("{}", conn_msg);
        app_logger::log_to_db("info", &conn_msg);

        // Split user headers: Authorization → auth_header, rest → custom_headers.
        let mut auth_header = None;
        let mut custom_headers = HashMap::new();
        for (k, v) in &self.headers {
            // rmcp rejects these reserved headers PER-REQUEST via
            // validate_custom_header (ReservedHeaderConflict) — a
            // user-configured one turns connect/every request into a fatal
            // error loop feeding mark_conn_loss. mcp-protocol-version is
            // reserved but allowed through by rmcp itself. (Rounds 11-12.)
            if k.eq_ignore_ascii_case("mcp-session-id")
                || k.eq_ignore_ascii_case("accept")
                || k.eq_ignore_ascii_case("last-event-id")
            {
                log::warn!(
                    "[{}] ignoring user-configured reserved header '{}' (managed by the MCP client)",
                    self.server_name,
                    k
                );
                continue;
            }
            if k.eq_ignore_ascii_case("authorization") {
                // rmcp applies auth_header via reqwest bearer_auth(), which
                // unconditionally prepends "Bearer " — a value that already
                // carries a scheme (OAuth-resolved "Bearer x", "Basic ...")
                // would go out as "Bearer Bearer x" (round 11 review). Pass
                // scheme-carrying values through custom_headers verbatim
                // (applied after bearer_auth, so it wins); bare tokens take
                // the auth_header path.
                let lower = v.to_ascii_lowercase();
                let has_scheme = ["bearer ", "basic ", "digest "]
                    .iter()
                    .any(|p| lower.starts_with(p));
                if has_scheme {
                    match (
                        http::HeaderName::try_from("authorization"),
                        http::HeaderValue::from_str(v),
                    ) {
                        (Ok(name), Ok(value)) => {
                            custom_headers.insert(name, value);
                        }
                        // A silently dropped Authorization goes out
                        // unauthenticated and only surfaces as an upstream
                        // 401 — warn like the generic invalid-header branch
                        // (round 12 review).
                        _ => log::warn!(
                            "[{}] dropping invalid Authorization header value",
                            self.server_name
                        ),
                    }
                } else {
                    auth_header = Some(v.clone());
                }
            } else if let (Ok(name), Ok(value)) = (
                http::HeaderName::try_from(k.as_str()),
                http::HeaderValue::from_str(v),
            ) {
                custom_headers.insert(name, value);
            } else {
                log::warn!(
                    "[{}] skipping invalid custom header '{}'",
                    self.server_name,
                    k
                );
            }
        }

        let mut config = StreamableHttpClientTransportConfig::with_uri(self.url.as_str());
        // Modern (2026) servers never assign a session to discover; legacy
        // servers assign one at initialize — both work with stateless allowed.
        config.allow_stateless = true;
        config.auth_header = auth_header;
        config.custom_headers = custom_headers;
        let transport = rmcp::transport::streamable_http_client::StreamableHttpClientTransport::from_config(config);

        // Lifecycle selection (origin #1382): `protocolNegotiation: 'legacy'`
        // skips the modern probe and goes straight to the legacy initialize
        // handshake; the default (auto) probes `server/discover` (2026 modern)
        // first and falls back to legacy initialize.
        let lifecycle = if self.legacy_only {
            ClientLifecycleMode::Initialize
        } else {
            ClientLifecycleMode::Auto {
                preferred_versions: vec![
                    ProtocolVersion::V_2026_07_28,
                    ProtocolVersion::V_2025_11_25,
                    ProtocolVersion::V_2025_06_18,
                    ProtocolVersion::V_2025_03_26,
                ],
                legacy_version: Some(ProtocolVersion::V_2025_03_26),
            }
        };
        let service = rmcp::service::serve_client_with_lifecycle(
            BridgeClientHandler::new(
                self.elicitation_slot.clone(),
                self.elicitation_notify_slot.clone(),
            ),
            transport,
            lifecycle,
        )
        .await
        .map_err(|e| {
            let elapsed = connect_start.elapsed();
            let msg = format!(
                "[{}] MCP connect failed after {:.1}s: {}",
                self.server_name,
                elapsed.as_secs_f64(),
                e
            );
            log::error!("{}", msg);
            app_logger::log_to_db("error", &msg);
            anyhow!(msg)
        })?;

        // Detect modern (discover) vs legacy (initialize) from peer info.
        // `is_some()` is always true in rmcp 3.4.1 (both lifecycles set peer
        // info) — discriminate by the negotiated protocol version instead.
        self.modern = service
            .peer_info()
            .map(|info| info.protocol_version == rmcp::model::ProtocolVersion::V_2026_07_28)
            .unwrap_or(false);
        if let Some(info) = service.peer_info() {
            // Empty version string → None so the update check doesn't treat
            // "unknown" as a real recorded version.
            self.server_version = info
                .server_info
                .as_ref()
                .map(|i| i.version.clone())
                .filter(|v| !v.is_empty());
            self.protocol_version = Some(info.protocol_version.to_string());
        }

        let mode = if self.modern { "2026 modern (discover, stateless)" } else { "legacy initialize" };
        let ok_msg = format!(
            "[{}] HTTP transport connected ({}, total={:.1}s, rmcp http)",
            self.server_name,
            mode,
            connect_start.elapsed().as_secs_f64()
        );
        log::info!("{}", ok_msg);
        app_logger::log_to_db("info", &ok_msg);

        self.service = Some(service);
        self.connected
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        // Drop the connection-lifetime elicitation completion forwarder —
        // the downstream owner is gone with the connection.
        if let Ok(mut g) = self.elicitation_notify_slot.lock() {
            *g = None;
        }
        self.connected
            .store(false, std::sync::atomic::Ordering::SeqCst);
        if let Some(service) = self.service.take() {
            // Bounded like the stdio transport: a wedged service (streaming
            // response / half-closed TCP) must not stall disconnect forever —
            // disconnect holds the per-server connect lock, so an unbounded
            // cancel would block every subsequent lifecycle op for this
            // server (review round 8, 2026-10-04).
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), service.cancel()).await;
        }
        let msg = format!("[{}] HTTP transport disconnected (rmcp http)", self.server_name);
        log::info!("{}", msg);
        app_logger::log_to_db("info", &msg);
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Post-rmcp migration, `is_connected()` only flips on explicit
    /// connect/disconnect — a dead child process / dropped stream never
    /// lowered the flag, which left every eviction/rebuild consumer
    /// (session_pool / on_demand run_call, mcp_manager session_rebuild's
    /// `!s.connected` filter) dead code for rmcp transports. Connection-
    /// class call errors mark the transport disconnected so those existing
    /// paths go live again.

    fn server_version(&self) -> Option<String> {
        self.server_version.clone()
    }

    fn protocol_version(&self) -> Option<String> {
        self.protocol_version.clone()
    }

    async fn list_tools(&self) -> Result<Vec<Tool>> {
        let service = self
            .service
            .as_ref()
            .ok_or_else(|| anyhow!("[{}] not connected", self.server_name))?;
        // Follow pagination cursors so servers using a small page size are not
        // silently truncated to their first page. Guarded: a buggy/malicious
        // server may echo the same cursor forever (or never return None) —
        // cap pages and break on cursor repetition instead of looping
        // unboundedly (on_demand's connect path has no outer timeout).
        let mut all = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen_cursors = std::collections::HashSet::new();
        let mut last_ttl: Option<u64> = None;
        let mut was_paged = false;
        for _ in 0..100 {
            let params = cursor
                .clone()
                .map(|c| rmcp::model::PaginatedRequestParams::default().with_cursor(Some(c)));
            let result = match service.peer().list_tools(params).await {
                Ok(r) => r,
                Err(e) => {
                    self.mark_conn_loss(&e);
                    return Err(anyhow!("tools/list failed: {e}"));
                }
            };
            // Freshness only from a positive TTL on the final (non-paged) result.
            if !was_paged {
                last_ttl = result.ttl_ms.filter(|t| *t > 0).map(|t| t as u64);
            }
            all.extend(result.tools);
            match result.next_cursor {
                Some(next) if seen_cursors.insert(next.clone()) => {
                    cursor = Some(next);
                    // Paged lists carry no freshness (origin #1277) — the
                    // flag, not a None-then-overwrite, keeps a ttl-bearing
                    // FINAL page from restoring freshness after the fact.
                    last_ttl = None;
                    was_paged = true;
                }
                // None (done) or repeated/absent-progress cursor — stop.
                _ => break,
            }
        }
        *self.last_list_ttl.lock().unwrap() = last_ttl;
        Ok(all
            .into_iter()
            .map(|t| Tool {
                hidden_params: None,
                name: t.name.to_string(),
                description: t.description.map(|d| d.to_string()),
                input_schema: Value::Object(t.input_schema.as_ref().clone()),
                server_name: self.server_name.clone(),
                enabled: true,
                annotations: t
                    .annotations
                    .as_ref()
                    .and_then(|a| serde_json::to_value(a).ok())
                    .filter(|v| !v.is_null()),
                output_schema: t
                    .output_schema
                    .as_ref()
                    .map(|s| Value::Object(s.as_ref().clone()))
                    .filter(|v| !v.is_null()),
                title: t.title.clone().filter(|s| !s.is_empty()),
                // rmcp 3.4.1 Tool has no execution field (added in a later SDK)
                execution: None,
                icons: t
                    .icons
                    .as_ref()
                    .and_then(|i| serde_json::to_value(i).ok())
                    .filter(|v| !v.is_null()),
                meta: t
                    .meta
                    .as_ref()
                    .and_then(|m| serde_json::to_value(m).ok())
                    .filter(|v| !v.is_null()),
                description_overridden: false,
            })
            .collect())
    }

    async fn list_tools_with_ttl(&self) -> Result<(Vec<Tool>, Option<u64>)> {
        let tools = self.list_tools().await?;
        let ttl = *self.last_list_ttl.lock().unwrap();
        Ok((tools, ttl))
    }

    async fn call_tool(&self, name: &str, arguments: Value) -> Result<ToolCallResult> {
        self.call_tool_with_meta(name, arguments, None).await
    }

    async fn call_tool_with_meta(
        &self,
        name: &str,
        arguments: Value,
        request_meta: Option<Value>,
    ) -> Result<ToolCallResult> {
        let service = self
            .service
            .as_ref()
            .ok_or_else(|| anyhow!("[{}] not connected", self.server_name))?;
        let mut params = CallToolRequestParams::new(name.to_string());
        params.arguments = Some(arguments.as_object().cloned().unwrap_or_default());

        // ① Downstream-driven MRTR retry: the downstream client already
        // fulfilled input requests and echoes inputResponses + requestState.
        // Apply verbatim and send once — no hub-side retry.
        if let Some(meta) = &request_meta {
            if let Some((input_responses, request_state)) = downstream_mrtr_retry(meta) {
                params.input_responses = Some(input_responses);
                params.request_state = request_state;
                let response = service
                    .peer()
                    .call_tool_once(params)
                    .await
                    .map_err(|e| {
                        self.mark_conn_loss(&e);
                        anyhow!("tools/call failed: {}", e)
                    })?;
                return self.map_call_response_once(response).await;
            }
            // ② Other request-level meta (OTel trace keys, …) travels upstream.
            params.meta = request_meta_from_value(Some(meta.clone()));
        }

        // Hub-driven path: call_tool_once + map_call_response_once reproduces
        // the old A1/A2 contract exactly — the SDK's Peer::call_tool (MRTR
        // helper) errors on CallToolResponse::Task (service/client.rs:
        // call_tool_with_mrtr_max_rounds → UnexpectedResponse), but a 2026
        // upstream MAY answer a plain tools/call with a task (SEP-2663
        // server's choice). map_call_response_once polls tasks to terminal
        // and promotes InputRequired via raw_meta.
        let response = service
            .peer()
            .call_tool_once(params)
            .await
            .map_err(|e| {
                self.mark_conn_loss(&e);
                anyhow!("tools/call failed: {}", e)
            })?;
        self.map_call_response_once(response).await
    }

    async fn read_resource(&self, uri: &str) -> Result<Vec<rmcp::model::ResourceContents>> {
        let service = self
            .service
            .as_ref()
            .ok_or_else(|| anyhow!("[{}] not connected", self.server_name))?;
        let params = rmcp::model::ReadResourceRequestParams::new(uri);
        let result = service
            .peer()
            .read_resource(params)
            .await
            .map_err(|e| {
                self.mark_conn_loss(&e);
                anyhow!("resources/read failed: {}", e)
            })?;
        Ok(result.contents)
    }

    fn elicitation_slot(&self) -> Option<crate::mcp::elicitation::ElicitationSlot> {
        Some(self.elicitation_slot.clone())
    }

    fn elicitation_notify_slot(&self) -> Option<crate::mcp::elicitation::ElicitNotifySlot> {
        Some(self.elicitation_notify_slot.clone())
    }
}

impl RmcpHttpTransport {
    /// Map a `call_tool_once` response (used on both downstream-driven MRTR
    /// retries and task-augmented calls) to the hub's ToolCallResult,
    /// preserving the old A1/A2 wire contracts:
    /// - `Task` → poll to terminal via tasks/get
    /// - `InputRequired` → raw_meta carries resultType/inputRequests so the
    ///   dispatcher promotes them to the response `_meta`
    /// - `Complete` → plain mapping
    async fn map_call_response_once(&self, response: CallToolResponse) -> Result<ToolCallResult> {
        match response {
            CallToolResponse::Complete(result) => {
                let structured_content = result.structured_content.clone().filter(|v| !v.is_null());
                Ok(ToolCallResult {
                    content: result
                        .content
                        .iter()
                        .filter_map(|c| serde_json::to_value(c).ok())
                        .collect(),
                    is_error: result.is_error.unwrap_or(false),
                    structured_content,
                    raw_meta: result.meta.as_ref().and_then(|m| serde_json::to_value(m).ok()),
                })
            }
            CallToolResponse::InputRequired(ir) => {
                // Fold the whole InputRequiredResult into raw_meta so nothing
                // is lost downstream (old parse_modern_result contract).
                let mut raw = serde_json::to_value(&ir).unwrap_or(json!({}));
                if let Some(obj) = raw.as_object_mut() {
                    obj.insert(
                        "io.modelcontextprotocol/resultType".to_string(),
                        json!("input_required"),
                    );
                }
                Ok(ToolCallResult {
                    content: vec![json!({"type":"text","text":"task requires input"})],
                    is_error: false,
                    structured_content: None,
                    raw_meta: Some(raw),
                })
            }
            CallToolResponse::Task(create) => {
                // Mirror the stdio transport: clamp both ends so a hostile
                // poll_interval_ms can neither spin nor swallow the deadline.
                let poll_interval = create
                    .task
                    .poll_interval_ms
                    .unwrap_or(1000)
                    .clamp(200, 30_000);
                let task_id = create.task.task_id.clone();
                self.poll_task_to_terminal(task_id, poll_interval).await
            }
            _ => Err(anyhow!("unexpected call_tool response")),
        }
    }
}

impl RmcpHttpTransport {
    /// Connection-loss classification by rmcp ServiceError VARIANT (the
    /// previous string-marker version was dead code: a dead stdio child
    /// surfaces as TransportClosed -> "Transport closed", which matched none
    /// of the old markers; "connection closed"/"send message error" exist
    /// only on initialize-time errors). Mark on transport-class failures —
    /// McpError is the SERVER's JSON-RPC error (its message text is
    /// server-controlled and often mentions "connection closed" for its own
    /// DB), and Timeout is our own cap: neither means the transport died.
    fn mark_conn_loss(&self, e: &rmcp::ServiceError) {
        use rmcp::ServiceError as SE;
        let conn_class = matches!(
            e,
            SE::TransportClosed { .. }
                | SE::TransportSend { .. }
                | SE::UnexpectedResponse { .. }
        );
        if conn_class {
            self.connected
                .store(false, std::sync::atomic::Ordering::SeqCst);
            crate::services::app_logger::log_to_db(
                "warn",
                &format!(
                    "[{}] connection-class error, marked disconnected: {e}",
                    self.server_name
                ),
            );
        }
    }
}
