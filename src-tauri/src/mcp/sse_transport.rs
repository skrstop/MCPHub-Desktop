/// SSE transport — connects to a remote MCP server via Server-Sent Events.
///
/// ⚠️ Intentionally NOT rmcp-backed (unlike stdio/http/openapi): rmcp's
/// `transport-sse-client` feature is not enabled, so this is the only
/// hand-written JSON-RPC implementation left in the codebase. It exists as a
/// compatibility fallback for legacy SSE-only servers; when the SDK ships a
/// suitable SSE client, replace it to complete the single-stack migration.
use super::client::McpTransport;
use crate::models::server::{Tool, ToolCallResult};
use crate::services::app_logger;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
    sync::Arc,
};
use tokio::sync::{oneshot, Mutex};

static SSE_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    SSE_REQUEST_ID.fetch_add(1, Ordering::SeqCst)
}

pub struct SseTransport {
    base_url: String,
    headers: HashMap<String, String>,
    client: Client,
    /// endpoint returned by SSE /sse handshake for POSTing requests
    post_endpoint: Option<String>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
    /// Shared so the background reader can flip it off when the stream dies
    /// (is_connected() used to keep reporting true until the next request
    /// failed and the pool corrected the status).
    connected: Arc<std::sync::atomic::AtomicBool>,
    server_name: String,
    /// Whether to use the traditional SSE pattern (background reader)
    /// or Streamable HTTP pattern (response on POST request)
    use_background_reader: bool,
    /// Channel to signal the background reader to stop
    stop_signal: Option<tokio::sync::oneshot::Sender<()>>,
    session_id: Arc<Mutex<String>>,
    /// Monotonic reader generation. Each connect() bumps it and the spawned
    /// reader snapshots its own; teardown-time actions (clearing `pending`,
    /// flipping `connected`) are skipped by a stale reader whose generation no
    /// longer matches — otherwise a retiring reader wipes the NEW session's
    /// pending requests and falsely marks the live connection down.
    reader_generation: Arc<std::sync::atomic::AtomicU64>,
}

impl SseTransport {
    pub fn new(
        server_name: impl Into<String>,
        base_url: impl Into<String>,
        headers: HashMap<String, String>,
    ) -> Self {
        // Drop header pairs that reqwest would panic on in
        // RequestBuilder::header (invalid name/value chars). Mirrors the
        // defensive validation openapi_transport applies before building
        // requests; failing closed to "header absent" beats a connect panic.
        let server_name_early = server_name.into();
        let headers: HashMap<String, String> = headers
            .into_iter()
            .filter(|(k, v)| {
                let ok = reqwest::header::HeaderName::try_from(k.as_str()).is_ok()
                    && reqwest::header::HeaderValue::try_from(v.as_str()).is_ok();
                if !ok {
                    // Silent drops hide config mistakes (e.g. a broken auth
                    // header then surfaces as upstream 401) — log them.
                    log::warn!("[{}] dropping invalid SSE header '{}'", server_name_early, k);
                }
                ok
            })
            .collect();
        let builder = Client::builder()
            // Per-read stall guard: a server that accepts the connection but
            // never sends bytes used to hang call_tool/list_tools forever
            // (only connect() had a timeout at the pool layer). Time between
            // bytes — SSE keepalive comments reset it.
            .read_timeout(std::time::Duration::from_secs(120));
        let client = builder
            .build()
            .unwrap_or_else(|_| Client::new());
        // mcp-session-id priority: server response > user-provided > generated UUID
        // Start with user-provided or empty (will be set from server response)
        let session_id = headers
            .get("mcp-session-id")
            .cloned()
            .unwrap_or_default();
        Self {
            server_name: server_name_early,
            base_url: base_url.into(),
            headers,
            client,
            post_endpoint: None,
            pending: Arc::new(Mutex::new(HashMap::new())),
            connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            use_background_reader: true, // Will be updated during connect
            stop_signal: None,
            session_id: Arc::new(Mutex::new(session_id)),
            reader_generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    /// Send a JSON-RPC **notification** (no id — the server never replies).
    /// Used for `notifications/initialized` after the initialize handshake
    /// (spec: clients MUST send it; strict servers gate further requests on it).
    async fn post_notification(&self, method: &str, params: Value) -> Result<()> {
        let endpoint = self
            .post_endpoint
            .as_deref()
            .ok_or_else(|| anyhow!("SSE endpoint not established"))?;
        let body = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let sid = self.session_id.lock().await.clone();
        let mut req = self.client.post(endpoint).header("Content-Type", "application/json");
        if !sid.is_empty() {
            req = req.header("mcp-session-id", &sid);
        }
        req = req.json(&body);
        for (k, v) in &self.headers {
            // Never replay a user-provided mcp-session-id here: after the
            // server assigns a fresh session the replayed stale header is
            // APPENDED (reqwest .header() adds, not replaces) and servers
            // then see two conflicting session ids.
            if k.eq_ignore_ascii_case("mcp-session-id") {
                continue;
            }
            req = req.header(k, v);
        }
        // Fire-and-forget: notifications get 202-style empty replies at best.
        // A non-2xx (session expired etc.) used to vanish silently.
        let resp = req.send().await?;
        if !resp.status().is_success() {
            log::warn!("[{}] notification '{}' got HTTP {}", self.server_name, method, resp.status());
        }
        Ok(())
    }

    async fn post_request(&self, method: &str, params: Value) -> Result<Value> {
        let endpoint = self
            .post_endpoint
            .as_deref()
            .ok_or_else(|| anyhow!("SSE endpoint not established"))?;

        let id = next_id();
        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        log::info!("[{}] Sending {} request (id={}) to {}", self.server_name, method, id, endpoint);

        if self.use_background_reader {
            // Traditional SSE pattern: send POST and wait for response via background reader
            let (tx, rx) = oneshot::channel::<Value>();
            {
                let mut map = self.pending.lock().await;
                map.insert(id, tx);
            }

            let sid = self.session_id.lock().await.clone();
            let mut req = self.client.post(endpoint)
                .header("Content-Type", "application/json");
            if !sid.is_empty() {
                req = req.header("mcp-session-id", &sid);
            }
            req = req.json(&body);
            for (k, v) in &self.headers {
                if k.eq_ignore_ascii_case("mcp-session-id") {
                    continue;
                }
                req = req.header(k, v);
            }

            log::debug!("[{}] Sending POST request...", self.server_name);
            let resp = req.send().await.map_err(|e| {
                e
            });
            let resp = match resp {
                Ok(r) => r,
                Err(e) => {
                    // Don't leak the pending entry when the POST itself fails.
                    self.pending.lock().await.remove(&id);
                    return Err(anyhow!(e));
                }
            };

            // Capture mcp-session-id from response (highest priority)
            if let Some(sid_header) = resp.headers().get("mcp-session-id") {
                if let Ok(sid_val) = sid_header.to_str() {
                    let mut current = self.session_id.lock().await;
                    *current = sid_val.to_string();
                    log::debug!("[{}] Captured mcp-session-id from response: {}", self.server_name, sid_val);
                }
            }

            let post_status = resp.status();
            log::debug!("[{}] POST response status: {}", self.server_name, post_status);
            if !post_status.is_success() {
                // Session expired / endpoint gone: surface the real reason
                // instead of burning the 60s waiter for a reply that will
                // never come.
                let body = resp.text().await.unwrap_or_default();
                self.pending.lock().await.remove(&id);
                return Err(anyhow!("MCP POST failed ({}): {}", post_status, body.chars().take(300).collect::<String>()));
            }

            log::debug!("[{}] Waiting for response (id={})...", self.server_name, id);
            let response = match tokio::time::timeout(std::time::Duration::from_secs(60), rx).await {
                Ok(Ok(v)) => v,
                Ok(Err(_)) => {
                    self.pending.lock().await.remove(&id);
                    log::error!("[{}] Response channel closed (id={})", self.server_name, id);
                    return Err(anyhow!("Response channel closed"));
                }
                Err(_) => {
                    self.pending.lock().await.remove(&id);
                    log::error!("[{}] Request timeout waiting for response (id={})", self.server_name, id);
                    return Err(anyhow!("Request timeout"));
                }
            };

            log::info!("[{}] Received response for id={}", self.server_name, id);

            if let Some(err) = response.get("error") {
                return Err(anyhow!("MCP error: {}", err));
            }
            Ok(response["result"].clone())
        } else {
            // Streamable HTTP pattern: send POST and read response directly
            let sid = self.session_id.lock().await.clone();
            let mut req = self.client.post(endpoint)
                .header("Content-Type", "application/json");
            if !sid.is_empty() {
                req = req.header("mcp-session-id", &sid);
            }
            req = req.json(&body);
            for (k, v) in &self.headers {
                if k.eq_ignore_ascii_case("mcp-session-id") {
                    continue;
                }
                req = req.header(k, v);
            }

            let resp = req.send().await?;

            // Capture mcp-session-id from response (highest priority)
            if let Some(sid_header) = resp.headers().get("mcp-session-id") {
                if let Ok(sid_val) = sid_header.to_str() {
                    let mut current = self.session_id.lock().await;
                    *current = sid_val.to_string();
                    log::debug!("[{}] Captured mcp-session-id from response: {}", self.server_name, sid_val);
                }
            }

            let status = resp.status();
            if !status.is_success() {
                return Err(anyhow!("HTTP error: {}", status));
            }

            let content_type = resp.headers().get("content-type")
                .map(|v| v.to_str().unwrap_or("").to_string())
                .unwrap_or_default();

            if content_type.contains("text/event-stream") {
                // Response is SSE stream, parse it
                use futures_util::StreamExt;
                let mut stream = resp.bytes_stream();
                // Byte buffer: a multi-byte UTF-8 char split across TCP chunks
                // must not be lossy-decoded per chunk (U+FFFD would silently
                // corrupt tool output). Decode only complete lines.
                let mut buffer: Vec<u8> = Vec::new();
                // Accumulated `data:` lines of the in-flight event (SSE allows
                // one JSON payload split across multiple data: lines, joined
                // with \n and terminated by an empty line) — same per-EVENT
                // parsing as the background reader below.
                let mut data_acc: Vec<String> = Vec::new();
                let mut result: Option<Value> = None;

                // Overall deadline: a stream that trickles bytes forever would
                // otherwise extend the call without bound (the read timeout
                // only caps inter-BYTE gaps, not total duration).
                let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);

                loop {
                    // Bound the wait by the REMAINING budget: checking the
                    // deadline only after a full 60s chunk-wait let a trickle
                    // feed overshoot to ~120s.
                    let chunk = tokio::time::timeout_at(deadline, stream.next())
                    .await
                    .map_err(|_| anyhow!("SSE stream read exceeded 60s overall budget"))?;
                    let chunk = match chunk {
                        Some(c) => c?,
                        None => break,
                    };
                    buffer.extend_from_slice(&chunk);

                    while let Some(newline_pos) = buffer.iter().position(|&b| b == b'\n') {
                        let line_bytes: Vec<u8> = buffer.drain(..=newline_pos).collect();
                        // Mirror the background reader: strip ONLY a trailing
                        // '\r'. A full trim() eats significant whitespace when
                        // a server splits JSON across data: lines (spaces
                        // inside string values are payload).
                        let mut line = String::from_utf8_lossy(&line_bytes[..line_bytes.len() - 1])
                            .to_string();
                        if line.ends_with('\r') {
                            line.pop();
                        }

                        if line.is_empty() {
                            // Event boundary: flush accumulated data lines.
                            if data_acc.is_empty() {
                                continue;
                            }
                            let joined = data_acc.join("\n");
                            data_acc.clear();
                            if let Ok(msg) = serde_json::from_str::<Value>(&joined) {
                                if msg.get("method").is_none()
                                    && msg.get("id").and_then(|v| v.as_u64()) == Some(id)
                                {
                                    result = Some(msg);
                                }
                            }
                            if result.is_some() {
                                break;
                            }
                            continue;
                        }
                        if line.starts_with("event:") {
                            continue;
                        }

                        // SSE payload semantics: strip only the single
                        // optional leading space after the colon (matching the
                        // background reader below) — a full trim() would eat
                        // significant leading/trailing whitespace inside
                        // multi-line JSON string values (review round 8).
                        let data_str = if let Some(data) = line.strip_prefix("data: ") {
                            Some(data.strip_suffix('\r').unwrap_or(data))
                        } else if let Some(data) = line.strip_prefix("data:") {
                            Some(data.strip_prefix(' ').unwrap_or(data).strip_suffix('\r').unwrap_or(data))
                        } else {
                            None
                        };

                        if let Some(data) = data_str {
                            data_acc.push(data.to_string());
                        }
                    }
                    if result.is_some() {
                        break;
                    }
                }

                let response = result.ok_or_else(|| anyhow!("No response received"))?;
                if let Some(err) = response.get("error") {
                    return Err(anyhow!("MCP error: {}", err));
                }
                Ok(response["result"].clone())
            } else {
                // Response is regular JSON
                let json: Value = resp.json().await?;
                if let Some(err) = json.get("error") {
                    return Err(anyhow!("MCP error: {}", err));
                }
                Ok(json["result"].clone())
            }
        }
    }
}

#[async_trait]
impl McpTransport for SseTransport {
    async fn connect(&mut self) -> Result<()> {
        // A re-connect on the same instance must not silently orphan the old
        // background reader (its stop channel would be dropped here).
        if let Some(stop_tx) = self.stop_signal.take() {
            let _ = stop_tx.send(());
        }
        // Use the URL as-is without adding any suffix
        let sse_url = self.base_url.trim_end_matches('/').to_string();

        let conn_msg = format!("[{}] Connecting to SSE endpoint: {}", self.server_name, sse_url);
        log::info!("{}", conn_msg);
        app_logger::log_to_db("info", &conn_msg);

        // Try GET first (traditional SSE), then POST (Streamable HTTP with SSE response)

        // Try GET request first
        let sid = self.session_id.lock().await.clone();
        let mut req = self.client.get(&sse_url)
            .header("Accept", "text/event-stream");
        if !sid.is_empty() {
            req = req.header("mcp-session-id", &sid);
        }
        for (k, v) in &self.headers {
            // Never replay a user-provided mcp-session-id here: after the
            // server assigns a fresh session the replayed stale header is
            // APPENDED (reqwest .header() adds, not replaces) and servers
            // then see two conflicting session ids.
            if k.eq_ignore_ascii_case("mcp-session-id") {
                continue;
            }
            req = req.header(k, v);
        }

        let get_resp = req.send().await?;

        // Capture mcp-session-id from response (highest priority)
        if let Some(sid_header) = get_resp.headers().get("mcp-session-id") {
            if let Ok(sid_val) = sid_header.to_str() {
                let mut current = self.session_id.lock().await;
                *current = sid_val.to_string();
                log::debug!("[{}] Captured mcp-session-id from GET response: {}", self.server_name, sid_val);
            }
        }

        let status = get_resp.status();
        let content_type = get_resp.headers().get("content-type")
            .map(|v| v.to_str().unwrap_or("unknown").to_string())
            .unwrap_or_else(|| "none".to_string());

        log::info!("[{}] GET response: status={}, content-type={}", self.server_name, status, content_type);

        let mut probe_sent_initialize = false;
        let response = if status.is_success() && content_type.contains("text/event-stream") {
            get_resp
        } else {
            // GET didn't return SSE, try POST
            log::info!("[{}] GET didn't return SSE, trying POST", self.server_name);
            let sid = self.session_id.lock().await.clone();
            let mut req = self.client.post(&sse_url)
                .header("Accept", "text/event-stream")
                .header("Content-Type", "application/json");
            if !sid.is_empty() {
                req = req.header("mcp-session-id", &sid);
            }
            let init_body = serde_json::to_string(&json!({
                "jsonrpc": "2.0",
                "id": 0,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-03-26",
                    "capabilities": {},
                    "clientInfo": { "name": "mcphub-desktop", "version": env!("CARGO_PKG_VERSION") }
                }
            })).expect("serialize init body");
            req = req.body(init_body);
            for (k, v) in &self.headers {
                if k.eq_ignore_ascii_case("mcp-session-id") {
                    continue;
                }
                req = req.header(k, v);
            }

            let post_resp = req.send().await?;

            // Capture mcp-session-id from response (highest priority)
            if let Some(sid_header) = post_resp.headers().get("mcp-session-id") {
                if let Ok(sid_val) = sid_header.to_str() {
                    let mut current = self.session_id.lock().await;
                    *current = sid_val.to_string();
                    log::debug!("[{}] Captured mcp-session-id from POST response: {}", self.server_name, sid_val);
                }
            }

            let status = post_resp.status();
            let content_type = post_resp.headers().get("content-type")
                .map(|v| v.to_str().unwrap_or("unknown").to_string())
                .unwrap_or_else(|| "none".to_string());

            log::info!("[{}] POST response: status={}, content-type={}", self.server_name, status, content_type);

            if status.is_success() && content_type.contains("text/event-stream") {
                // The probe body IS an initialize (id=0) — remember so the
                // common handshake below doesn't initialize the same session
                // twice (strict servers reject a second initialize).
                probe_sent_initialize = true;
                post_resp
            } else if status.is_success() {
                // POST returned non-SSE (likely JSON from a Streamable HTTP server)
                // Read the initialize response directly and set up no-background-reader mode
                log::info!("[{}] POST returned non-SSE ({}), treating as Streamable HTTP", self.server_name, content_type);
                let json: Value = post_resp.json().await?;
                if let Some(err) = json.get("error") {
                    return Err(anyhow!("MCP error: {}", err));
                }
                self.post_endpoint = Some(sse_url.clone());
                self.use_background_reader = false;
                self.connected.store(true, std::sync::atomic::Ordering::SeqCst);
                log::info!("[{}] Connected via Streamable HTTP mode (no background reader)", self.server_name);
                // Spec: clients MUST send notifications/initialized after the
                // initialize response, before any other request. The early
                // return used to skip this — strict servers (rmcp included)
                // then answer the first tools/list with -32002 "Server not
                // initialized".
                if let Err(e) = self.post_notification("notifications/initialized", json!({})).await {
                    log::warn!("[{}] notifications/initialized failed (continuing): {}", self.server_name, e);
                }
                return Ok(());
            } else {
                return Err(anyhow!("SSE connect failed: Neither GET nor POST returned SSE stream (url: {})", sse_url));
            }
        };

        // The first SSE event contains the endpoint URL for JSON-RPC POSTs
        // MCP protocol sends: event: endpoint\ndata: /messages?sessionId=xxx
        // Some servers also send JSON: data: {"endpoint": "/messages"}
        let mut stream = response.bytes_stream();
        use futures_util::StreamExt;
        let mut endpoint: Option<String> = None;
        // Byte buffer: decode only complete lines so a multi-byte char split
        // across TCP chunks is never corrupted (see post_request loop).
        let mut buffer: Vec<u8> = Vec::new();
        let mut current_event_type: Option<String> = None;
        let mut first_chunk = true;

        // Overall deadline on the endpoint capture: some servers open an SSE
        // stream (200 + text/event-stream) that never sends an endpoint event
        // but emits periodic keepalive comment lines — those reset reqwest's
        // read_timeout forever and this loop (unlike post_request's 60s
        // deadline) would block connect() indefinitely. Fall back to the
        // base-URL endpoint on timeout.
        let capture = async {
        while let Some(chunk) = stream.next().await {
            let Ok(chunk) = chunk else {
                log::warn!("[{}] SSE endpoint stream error during handshake", self.server_name);
                break;
            };

            // Log the first chunk to see what the server is sending
            if first_chunk {
                let preview: String = String::from_utf8_lossy(&chunk).chars().take(300).collect();
                log::info!("[{}] First SSE chunk ({} bytes): {}", self.server_name, chunk.len(), preview);
                first_chunk = false;
            }

            buffer.extend_from_slice(&chunk);

            // Process complete lines
            while let Some(newline_pos) = buffer.iter().position(|&b| b == b'\n') {
                let line_bytes: Vec<u8> = buffer.drain(..=newline_pos).collect();
                let line = String::from_utf8_lossy(&line_bytes[..line_bytes.len() - 1])
                    .trim()
                    .to_string();

                // Skip empty lines (they mark the end of an event)
                if line.is_empty() {
                    current_event_type = None;
                    continue;
                }

                // Track event type
                if let Some(event_type) = line.strip_prefix("event:") {
                    current_event_type = Some(event_type.trim().to_string());
                    log::debug!("[{}] SSE event type: {}", self.server_name, event_type.trim());
                    continue;
                }

                // Parse data line - handle both "data: ..." and "data:..." formats
                let data_str = if let Some(data) = line.strip_prefix("data: ") {
                    Some(data.trim())
                } else if let Some(data) = line.strip_prefix("data:") {
                    Some(data.trim())
                } else {
                    None
                };

                if let Some(data) = data_str {
                    // If event type is "endpoint", this data is the endpoint URL
                    if current_event_type.as_deref() == Some("endpoint") {
                        endpoint = Some(data.to_string());
                        log::info!("[{}] Found endpoint from event: {}", self.server_name, data);
                        break;
                    }

                    // Try JSON format first: {"endpoint": "/messages"}
                    if let Ok(v) = serde_json::from_str::<Value>(data) {
                        if let Some(ep) = v.get("endpoint").and_then(|e| e.as_str()) {
                            endpoint = Some(ep.to_string());
                            log::info!("[{}] Found endpoint from JSON: {}", self.server_name, ep);
                            break;
                        }
                    }

                    // If no event type specified, try to parse as endpoint URL
                    // Some servers don't send event type, just data
                    if current_event_type.is_none() && (data.starts_with('/') || data.starts_with("http")) {
                        // This might be an endpoint - but be careful, it could also be a message
                        // Accept both absolute http(s) URLs and root-relative paths;
                        // anything embedding a JSON-RPC body is a message, not a URL.
                        if (data.starts_with('/') || data.starts_with("http://") || data.starts_with("https://"))
                            && !data.contains("\"jsonrpc\"")
                        {
                            endpoint = Some(data.to_string());
                            log::info!("[{}] Found endpoint from data (no event type): {}", self.server_name, data);
                            break;
                        }
                    }
                }
            }
            if endpoint.is_some() {
                break;
            }
        }
        endpoint
        };
        let captured = tokio::time::timeout(std::time::Duration::from_secs(60), capture).await;
        let mut endpoint = match captured {
            Ok(ep) => ep,
            Err(_) => {
                log::warn!(
                    "[{}] endpoint capture timed out after 60s (stream alive but no endpoint event); falling back to base URL",
                    self.server_name
                );
                None::<String>
            }
        };

        // If no endpoint received, try using the base URL itself as the endpoint
        // This handles Streamable HTTP servers that respond with SSE on POST
        if endpoint.is_none() {
            log::info!("[{}] No endpoint event received, trying base URL as POST endpoint", self.server_name);
            endpoint = Some(sse_url.clone());
            self.use_background_reader = false;
        } else {
            self.use_background_reader = true;
        }

        let ep = endpoint.ok_or_else(|| anyhow!("SSE handshake: no endpoint received"))?;
        log::info!("[{}] SSE endpoint resolved to: {}", self.server_name, ep);

        // If relative URL, prepend origin (scheme + host + port) from base URL
        self.post_endpoint = Some(if ep.starts_with("http") {
            ep
        } else {
            // Extract origin from base URL (e.g., "http://127.0.0.1:64343/sse" -> "http://127.0.0.1:64343")
            let origin = if let Ok(parsed) = url::Url::parse(&self.base_url) {
                format!("{}://{}", parsed.scheme(), parsed.host_str().unwrap_or("localhost"))
                    + &parsed.port().map(|p| format!(":{}", p)).unwrap_or_default()
            } else {
                // Fallback: use base URL as-is
                self.base_url.trim_end_matches('/').to_string()
            };
            format!("{}{}", origin, ep)
        });

        log::info!("[{}] POST endpoint resolved to: {}", self.server_name, self.post_endpoint.as_deref().unwrap_or("none"));

        // For traditional SSE pattern: spawn background reader to continue reading from the stream
        // The stream is already established, we just need to keep reading responses
        if self.use_background_reader {
            let pending = self.pending.clone();
            let server_name = self.server_name.clone();
            let connected_flag = self.connected.clone();
            let reader_gen = {
                let g = self.reader_generation.load(std::sync::atomic::Ordering::SeqCst) + 1;
                self.reader_generation
                    .store(g, std::sync::atomic::Ordering::SeqCst);
                let arc = self.reader_generation.clone();
                (g, arc)
            };

            // Continue reading from the existing stream in the background
            // We need to move the stream into the background task
            let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
            self.stop_signal = Some(stop_tx);

            tokio::spawn(async move {
                use futures_util::StreamExt;
                // Take over the handshake loop's byte buffer: it may hold residual
                // bytes AFTER the endpoint event (a TCP chunk can pack the
                // endpoint event plus the beginning of a subsequent JSON-RPC
                // response) — starting from an empty buffer would drop them and
                // the response's id would never reach the pending table (caller
                // hangs 60s).
                let mut buffer = buffer;
                // Accumulated `data:` lines of the in-flight SSE event. Must
                // persist across chunks: a chunk boundary can fall between the
                // last data line and its terminating empty line.
                let mut data_acc: Vec<String> = Vec::new();

                loop {
                    tokio::select! {
                        chunk = stream.next() => {
                            match chunk {
                                Some(Ok(bytes)) => {
                                    buffer.extend_from_slice(&bytes);

                                    // Process complete lines
                                    // SSE spec: an event's payload may span
                                    // multiple `data:` lines, joined with \n
                                    // and terminated by an empty line. Parse
                                    // per-EVENT, not per-line, so servers that
                                    // split a JSON response across data lines
                                    // are not dropped (previously: silently
                                    // lost → 60s timeout). NOTE: `data_acc`
                                    // must live ACROSS chunks — a chunk
                                    // boundary can fall between the last data
                                    // line and its terminating empty line, and
                                    // a per-chunk accumulator would drop the
                                    // partial event.
                                    while let Some(newline_pos) = buffer.iter().position(|&b| b == b'\n') {
                                        let line_bytes: Vec<u8> = buffer.drain(..=newline_pos).collect();
                                        // SSE: strip only the trailing CR (CRLF
                                        // line ends). A full trim() would eat
                                        // leading/trailing whitespace that is part
                                        // of the payload when servers split JSON
                                        // across data lines (spaces inside string
                                        // values are significant).
                                        let mut line = String::from_utf8_lossy(&line_bytes[..line_bytes.len() - 1])
                                            .to_string();
                                        if line.ends_with('\r') {
                                            line.pop();
                                        }

                                        // Skip event type lines
                                        if line.starts_with("event:") {
                                            continue;
                                        }
                                        // Other legal SSE fields are neither data
                                        // nor event boundaries — skip them without
                                        // flushing the accumulator (previously any
                                        // non-data line cut the event short and
                                        // multi-part JSON was lost).
                                        if line.starts_with("id:") || line.starts_with("retry:") || line.starts_with(':') {
                                            continue;
                                        }

                                        // Parse data line (both "data: x" and "data:x").
                                        // SSE says strip ONE optional space after the
                                        // colon; the rest of the content is payload.
                                        let data_str = if let Some(data) = line.strip_prefix("data: ") {
                                            Some(data.to_string())
                                        } else if let Some(data) = line.strip_prefix("data:") {
                                            Some(data.strip_prefix(' ').unwrap_or(data).to_string())
                                        } else {
                                            None
                                        };

                                        if let Some(data) = data_str {
                                            data_acc.push(data);
                                            continue;
                                        }

                                        // SSE spec: only an empty line terminates
                                        // an event. Unknown extension field lines
                                        // must be ignored WITHOUT flushing — a
                                        // server splitting JSON across data: lines
                                        // may interleave custom fields.
                                        if !line.trim().is_empty() {
                                            continue;
                                        }
                                        if data_acc.is_empty() {
                                            continue;
                                        }
                                        let joined = data_acc.join("\n");
                                        data_acc.clear();
                                        if let Ok(msg) = serde_json::from_str::<Value>(&joined) {
                                                // Only route RESPONSES (no `method`)
                                                // into the pending table — a
                                                // colliding server-initiated request
                                                // would otherwise steal the entry.
                                                if msg.get("method").is_some() {
                                                    continue;
                                                }
                                                if let Some(id) = msg.get("id").and_then(|v| v.as_u64()) {
                                                    log::debug!("[{}] Received response for id: {}", server_name, id);
                                                    let mut map = pending.lock().await;
                                                    if let Some(tx) = map.remove(&id) {
                                                        let _ = tx.send(msg);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                Some(Err(e)) => {
                                    log::warn!("[{}] SSE stream error: {}", server_name, e);
                                    if reader_gen.0 == reader_gen.1.load(std::sync::atomic::Ordering::SeqCst) {
                                        connected_flag.store(false, std::sync::atomic::Ordering::SeqCst);
                                    }
                                    break;
                                }
                                None => {
                                    log::info!("[{}] SSE stream ended", server_name);
                                    if reader_gen.0 == reader_gen.1.load(std::sync::atomic::Ordering::SeqCst) {
                                        connected_flag.store(false, std::sync::atomic::Ordering::SeqCst);
                                    }
                                    break;
                                }
                            }
                        }
                        _ = &mut stop_rx => {
                            log::info!("[{}] SSE background reader stopped", server_name);
                            break;
                        }
                    }
                }
                // The stream is gone: fail every pending waiter immediately
                // (dropping the senders closes their oneshot channels) instead
                // of letting them hang until the 60s timeout, and stop the
                // pending map from leaking entries that will never be answered.
                // STALE-READER GUARD: a superseded reader (a newer connect()
                // already spawned its own) must NOT wipe the new session's
                // pending entries or flip the shared connected flag — its
                // teardown would destroy the live connection's in-flight
                // requests.
                if reader_gen.0 == reader_gen.1.load(std::sync::atomic::Ordering::SeqCst) {
                    pending.lock().await.clear();
                    log::warn!("[{}] SSE background reader ended; pending waiters failed fast", server_name);
                } else {
                    log::info!("[{}] SSE background reader ended (stale generation); leaving newer reader's state untouched", server_name);
                }
            });
        } else {
            log::info!("[{}] Using Streamable HTTP mode (no background reader)", self.server_name);
        }

        // MCP initialize — skipped when the POST probe above already sent one
        // (same session; a second initialize is rejected by strict servers).
        if probe_sent_initialize {
            log::info!("[{}] Skipping duplicate initialize (probe already initialized)", self.server_name);
        } else {
        self.post_request(
            "initialize",
            json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "mcphub-desktop", "version": env!("CARGO_PKG_VERSION") }
            }),
        )
        .await
        .inspect_err(|_| {
            // The background reader was already spawned above; on init failure
            // stop it now, otherwise it lingers consuming the stream (a
            // keepalive stream may never end on its own).
            if let Some(tx) = self.stop_signal.take() {
                let _ = tx.send(());
            }
        })?;
        }

        // Spec: clients MUST send notifications/initialized after the
        // initialize response, before any other request.
        if let Err(e) = self.post_notification("notifications/initialized", json!({})).await {
            log::warn!("[{}] notifications/initialized failed (continuing): {}", self.server_name, e);
        }

        self.connected.store(true, std::sync::atomic::Ordering::SeqCst);
        log::info!("[{}] SSE transport connected", self.server_name);
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.connected.store(false, std::sync::atomic::Ordering::SeqCst);
        let msg = format!("[{}] Disconnecting SSE transport...", self.server_name);
        log::info!("{}", msg);
        app_logger::log_to_db("info", &msg);

        // Stop the background reader if it's running
        if let Some(stop_tx) = self.stop_signal.take() {
            let _ = stop_tx.send(());
        }
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(std::sync::atomic::Ordering::SeqCst)
    }

    async fn list_tools(&self) -> Result<Vec<Tool>> {
        // Follow pagination cursors so servers using a small page size are not
        // silently truncated to their first page (parity with the rmcp stdio/
        // http transports). Guarded: cap pages and break on cursor repetition.
        let mut raw_tools = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen_cursors = std::collections::HashSet::new();
        for _ in 0..100 {
            let mut body = json!({});
            if let Some(c) = &cursor {
                body["cursor"] = json!(c);
            }
            let result = self.post_request("tools/list", body).await?;
            raw_tools.extend(
                result["tools"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default(),
            );
            match result.get("nextCursor").and_then(|v| v.as_str()) {
                Some(next) if seen_cursors.insert(next.to_string()) => cursor = Some(next.to_string()),
                // None (done) or repeated cursor — stop.
                _ => break,
            }
        }
        let tools = raw_tools
            .into_iter()
            .map(|t| Tool {
                name: t["name"].as_str().unwrap_or("").to_string(),
                description: t["description"].as_str().map(|s| s.to_string()),
                input_schema: t["inputSchema"].clone(),
                server_name: self.server_name.clone(),
                enabled: true,
                annotations: t.get("annotations").cloned().filter(|v| !v.is_null()),
                output_schema: t.get("outputSchema").cloned().filter(|v| !v.is_null()),
                title: t.get("title").and_then(|v| v.as_str()).map(|s| s.to_string()),
                execution: t.get("execution").cloned().filter(|v| !v.is_null()),
                icons: t.get("icons").cloned().filter(|v| !v.is_null()),
                meta: t.get("_meta").cloned().filter(|v| !v.is_null()),
                description_overridden: false,
            })
            .collect();
        Ok(tools)
    }

    async fn call_tool(&self, name: &str, arguments: Value) -> Result<ToolCallResult> {
        let result = self
            .post_request("tools/call", json!({ "name": name, "arguments": arguments }))
            .await?;
        let content = result["content"].as_array().cloned().unwrap_or_default();
        let is_error = result["isError"].as_bool().unwrap_or(false);
        let structured_content = result.get("structuredContent").cloned().filter(|v| !v.is_null());
        // A2/A9: carry upstream _meta verbatim (MRTR input_required, trace).
        let raw_meta = result.get("_meta").cloned().filter(|v| !v.is_null());
        Ok(ToolCallResult { content, is_error, structured_content, raw_meta })
    }
}
