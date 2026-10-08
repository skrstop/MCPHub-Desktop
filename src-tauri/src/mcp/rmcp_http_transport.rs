//! rmcp-backed Streamable HTTP transport — replaces the hand-written JSON-RPC
//! POST/SSE framing in `http_transport.rs` with the official rmcp client while
//! preserving the hub's upstream behaviours:
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

/// Empty client handler — the hub only makes requests upstream, it never
/// handles server-initiated requests/notifications.
#[derive(Default)]
struct BridgeClientHandler;

impl rmcp::ClientHandler for BridgeClientHandler {}

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
    connected: bool,
    /// True when the upstream answered `server/discover` (2026 modern,
    /// stateless) — logged for the activity panel, rmcp drives the difference.
    modern: bool,
    server_version: Option<String>,
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
        Self {
            server_name: server_name.into(),
            url: url.into(),
            headers,
            service: None,
            connected: false,
            modern: false,
            server_version: None,
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
            if k.eq_ignore_ascii_case("authorization") {
                auth_header = Some(v.clone());
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

        // Auto lifecycle: probe `server/discover` (modern) → fall back to the
        // legacy initialize handshake. SDK-native replacement of probe_modern.
        let service = rmcp::service::serve_client_with_lifecycle(
            BridgeClientHandler,
            transport,
            ClientLifecycleMode::Auto {
                preferred_versions: vec![
                    ProtocolVersion::V_2026_07_28,
                    ProtocolVersion::V_2025_11_25,
                    ProtocolVersion::V_2025_06_18,
                    ProtocolVersion::V_2025_03_26,
                ],
                legacy_version: Some(ProtocolVersion::V_2025_03_26),
            },
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
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.connected = false;
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
    }

    fn server_version(&self) -> Option<String> {
        self.server_version.clone()
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
        for _ in 0..100 {
            let params = cursor
                .clone()
                .map(|c| rmcp::model::PaginatedRequestParams::default().with_cursor(Some(c)));
            let result = service.peer().list_tools(params).await?;
            // Freshness only from a positive TTL on the final (non-paged) result.
            last_ttl = result.ttl_ms.filter(|t| *t > 0).map(|t| t as u64);
            all.extend(result.tools);
            match result.next_cursor {
                Some(next) if seen_cursors.insert(next.clone()) => {
                    cursor = Some(next);
                    last_ttl = None; // paged lists carry no freshness (origin #1277)
                }
                // None (done) or repeated/absent-progress cursor — stop.
                _ => break,
            }
        }
        *self.last_list_ttl.lock().unwrap() = last_ttl;
        Ok(all
            .into_iter()
            .map(|t| Tool {
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
                    .map_err(|e| anyhow!("tools/call failed: {}", e))?;
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
            .map_err(|e| anyhow!("tools/call failed: {}", e))?;
        self.map_call_response_once(response).await
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
