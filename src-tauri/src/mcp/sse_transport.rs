/// SSE transport — connects to a remote MCP server via Server-Sent Events.
use super::client::McpTransport;
use crate::models::server::{Tool, ToolCallResult};
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
    connected: bool,
    server_name: String,
    /// Whether to use the traditional SSE pattern (background reader)
    /// or Streamable HTTP pattern (response on POST request)
    use_background_reader: bool,
    /// Channel to signal the background reader to stop
    stop_signal: Option<tokio::sync::oneshot::Sender<()>>,
}

impl SseTransport {
    pub fn new(
        server_name: impl Into<String>,
        base_url: impl Into<String>,
        headers: HashMap<String, String>,
    ) -> Self {
        let builder = Client::builder();
        // Apply per-transport headers via default headers would require building them here
        let client = builder.build().expect("Failed to build reqwest client");
        Self {
            server_name: server_name.into(),
            base_url: base_url.into(),
            headers,
            client,
            post_endpoint: None,
            pending: Arc::new(Mutex::new(HashMap::new())),
            connected: false,
            use_background_reader: true, // Will be updated during connect
            stop_signal: None,
        }
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

            let mut req = self.client.post(endpoint)
                .header("Content-Type", "application/json")
                .json(&body);
            for (k, v) in &self.headers {
                req = req.header(k, v);
            }

            log::debug!("[{}] Sending POST request...", self.server_name);
            let resp = req.send().await?;
            log::debug!("[{}] POST response status: {}", self.server_name, resp.status());

            log::debug!("[{}] Waiting for response (id={})...", self.server_name, id);
            let response = tokio::time::timeout(
                std::time::Duration::from_secs(60),
                rx,
            )
            .await
            .map_err(|_| {
                log::error!("[{}] Request timeout waiting for response (id={})", self.server_name, id);
                anyhow!("Request timeout")
            })?
            .map_err(|_| {
                log::error!("[{}] Response channel closed (id={})", self.server_name, id);
                anyhow!("Response channel closed")
            })?;

            log::info!("[{}] Received response for id={}", self.server_name, id);

            if let Some(err) = response.get("error") {
                return Err(anyhow!("MCP error: {}", err));
            }
            Ok(response["result"].clone())
        } else {
            // Streamable HTTP pattern: send POST and read response directly
            let mut req = self.client.post(endpoint)
                .header("Accept", "text/event-stream")
                .header("Content-Type", "application/json")
                .json(&body);
            for (k, v) in &self.headers {
                req = req.header(k, v);
            }

            let resp = req.send().await?;
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
                let mut buffer = String::new();
                let mut result: Option<Value> = None;

                while let Some(chunk) = stream.next().await {
                    let chunk = chunk?;
                    let text = String::from_utf8_lossy(&chunk);
                    buffer.push_str(&text);

                    while let Some(newline_pos) = buffer.find('\n') {
                        let line = buffer[..newline_pos].trim().to_string();
                        buffer = buffer[newline_pos + 1..].to_string();

                        if line.is_empty() || line.starts_with("event:") {
                            continue;
                        }

                        let data_str = if let Some(data) = line.strip_prefix("data: ") {
                            Some(data.trim())
                        } else if let Some(data) = line.strip_prefix("data:") {
                            Some(data.trim())
                        } else {
                            None
                        };

                        if let Some(data) = data_str {
                            if let Ok(msg) = serde_json::from_str::<Value>(data) {
                                if msg.get("id").and_then(|v| v.as_u64()) == Some(id) {
                                    result = Some(msg);
                                    break;
                                }
                            }
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
        // Use the URL as-is without adding any suffix
        let sse_url = self.base_url.trim_end_matches('/').to_string();

        log::info!("[{}] Connecting to SSE endpoint: {}", self.server_name, sse_url);

        // Try GET first (traditional SSE), then POST (Streamable HTTP with SSE response)
        let mut response = None;

        // Try GET request first
        let mut req = self.client.get(&sse_url)
            .header("Accept", "text/event-stream");
        for (k, v) in &self.headers {
            req = req.header(k, v);
        }

        let get_resp = req.send().await?;
        let status = get_resp.status();
        let content_type = get_resp.headers().get("content-type")
            .map(|v| v.to_str().unwrap_or("unknown").to_string())
            .unwrap_or_else(|| "none".to_string());

        log::info!("[{}] GET response: status={}, content-type={}", self.server_name, status, content_type);

        if status.is_success() && content_type.contains("text/event-stream") {
            response = Some(get_resp);
        } else {
            // GET didn't return SSE, try POST
            log::info!("[{}] GET didn't return SSE, trying POST", self.server_name);
            let mut req = self.client.post(&sse_url)
                .header("Accept", "text/event-stream")
                .header("Content-Type", "application/json")
                .body(r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"mcphub-desktop","version":"0.1.0"}}}"#);
            for (k, v) in &self.headers {
                req = req.header(k, v);
            }

            let post_resp = req.send().await?;
            let status = post_resp.status();
            let content_type = post_resp.headers().get("content-type")
                .map(|v| v.to_str().unwrap_or("unknown").to_string())
                .unwrap_or_else(|| "none".to_string());

            log::info!("[{}] POST response: status={}, content-type={}", self.server_name, status, content_type);

            if status.is_success() && content_type.contains("text/event-stream") {
                response = Some(post_resp);
            } else {
                return Err(anyhow!("SSE connect failed: Neither GET nor POST returned SSE stream (url: {})", sse_url));
            }
        }

        let response = response.unwrap();

        // The first SSE event contains the endpoint URL for JSON-RPC POSTs
        // MCP protocol sends: event: endpoint\ndata: /messages?sessionId=xxx
        // Some servers also send JSON: data: {"endpoint": "/messages"}
        let mut stream = response.bytes_stream();
        use futures_util::StreamExt;
        let mut endpoint: Option<String> = None;
        let mut buffer = String::new();
        let mut current_event_type: Option<String> = None;
        let mut first_chunk = true;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            let text = String::from_utf8_lossy(&chunk);

            // Log the first chunk to see what the server is sending
            if first_chunk {
                let preview = if text.len() > 500 { &text[..500] } else { &text };
                log::info!("[{}] First SSE chunk ({} bytes): {}", self.server_name, text.len(), preview);
                first_chunk = false;
            }

            buffer.push_str(&text);

            // Process complete lines
            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim().to_string();
                buffer = buffer[newline_pos + 1..].to_string();

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
                        // Only use it if it looks like a URL path
                        if data.starts_with('/') && !data.contains("\"jsonrpc\"") {
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

        // If relative URL, prepend base
        self.post_endpoint = Some(if ep.starts_with("http") {
            ep
        } else {
            format!(
                "{}{}",
                self.base_url.trim_end_matches('/'),
                ep
            )
        });

        log::info!("[{}] POST endpoint resolved to: {}", self.server_name, self.post_endpoint.as_deref().unwrap_or("none"));

        // For traditional SSE pattern: spawn background reader to continue reading from the stream
        // The stream is already established, we just need to keep reading responses
        if self.use_background_reader {
            let pending = self.pending.clone();
            let server_name = self.server_name.clone();

            // Continue reading from the existing stream in the background
            // We need to move the stream into the background task
            let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
            self.stop_signal = Some(stop_tx);

            tokio::spawn(async move {
                use futures_util::StreamExt;
                let mut buffer = String::new();

                loop {
                    tokio::select! {
                        chunk = stream.next() => {
                            match chunk {
                                Some(Ok(bytes)) => {
                                    let text = String::from_utf8_lossy(&bytes);
                                    buffer.push_str(&text);

                                    // Process complete lines
                                    while let Some(newline_pos) = buffer.find('\n') {
                                        let line = buffer[..newline_pos].trim().to_string();
                                        buffer = buffer[newline_pos + 1..].to_string();

                                        // Skip empty lines and event type lines
                                        if line.is_empty() || line.starts_with("event:") {
                                            continue;
                                        }

                                        // Parse data line
                                        let data_str = if let Some(data) = line.strip_prefix("data: ") {
                                            Some(data.trim())
                                        } else if let Some(data) = line.strip_prefix("data:") {
                                            Some(data.trim())
                                        } else {
                                            None
                                        };

                                        if let Some(data) = data_str {
                                            if let Ok(msg) = serde_json::from_str::<Value>(data) {
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
                                }
                                Some(Err(e)) => {
                                    log::warn!("[{}] SSE stream error: {}", server_name, e);
                                    break;
                                }
                                None => {
                                    log::info!("[{}] SSE stream ended", server_name);
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
            });
        } else {
            log::info!("[{}] Using Streamable HTTP mode (no background reader)", self.server_name);
        }

        // MCP initialize
        self.post_request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "mcphub-desktop", "version": "0.1.0" }
            }),
        )
        .await?;

        self.connected = true;
        log::info!("[{}] SSE transport connected", self.server_name);
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.connected = false;
        // Stop the background reader if it's running
        if let Some(stop_tx) = self.stop_signal.take() {
            let _ = stop_tx.send(());
        }
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    async fn list_tools(&self) -> Result<Vec<Tool>> {
        let result = self.post_request("tools/list", json!({})).await?;
        let tools = result["tools"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|t| Tool {
                name: t["name"].as_str().unwrap_or("").to_string(),
                description: t["description"].as_str().map(|s| s.to_string()),
                input_schema: t["inputSchema"].clone(),
                server_name: self.server_name.clone(),
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
        Ok(ToolCallResult { content, is_error })
    }
}
