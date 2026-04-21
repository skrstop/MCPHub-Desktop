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

        let (tx, rx) = oneshot::channel::<Value>();
        {
            let mut map = self.pending.lock().await;
            map.insert(id, tx);
        }

        let mut req = self.client.post(endpoint).json(&body);
        for (k, v) in &self.headers {
            req = req.header(k, v);
        }
        req.send().await?;

        let response = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            rx,
        )
        .await
        .map_err(|_| anyhow!("Request timeout"))?
        .map_err(|_| anyhow!("Response channel closed"))?;

        if let Some(err) = response.get("error") {
            return Err(anyhow!("MCP error: {}", err));
        }
        Ok(response["result"].clone())
    }
}

#[async_trait]
impl McpTransport for SseTransport {
    async fn connect(&mut self) -> Result<()> {
        let sse_url = format!("{}/sse", self.base_url.trim_end_matches('/'));
        let mut req = self.client.get(&sse_url);
        for (k, v) in &self.headers {
            req = req.header(k, v);
        }

        let response = req.send().await?;
        if !response.status().is_success() {
            return Err(anyhow!("SSE connect failed: {}", response.status()));
        }

        // The first SSE event contains the endpoint URL for JSON-RPC POSTs
        let mut stream = response.bytes_stream();
        use futures_util::StreamExt;
        let mut endpoint: Option<String> = None;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            let text = String::from_utf8_lossy(&chunk);
            for line in text.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    if let Ok(v) = serde_json::from_str::<Value>(data) {
                        if let Some(ep) = v.get("endpoint").and_then(|e| e.as_str()) {
                            endpoint = Some(ep.to_string());
                            break;
                        }
                    }
                }
            }
            if endpoint.is_some() {
                break;
            }
        }

        let ep = endpoint.ok_or_else(|| anyhow!("SSE handshake: no endpoint received"))?;
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

        // Spawn background reader for SSE events (responses)
        let pending = self.pending.clone();
        let server_name = self.server_name.clone();
        let client = self.client.clone();
        let url = sse_url.clone();
        let headers = self.headers.clone();
        tokio::spawn(async move {
            loop {
                let mut req = client.get(&url);
                for (k, v) in &headers {
                    req = req.header(k, v);
                }
                match req.send().await {
                    Ok(resp) => {
                        use futures_util::StreamExt;
                        let mut s = resp.bytes_stream();
                        while let Some(Ok(chunk)) = s.next().await {
                            let text = String::from_utf8_lossy(&chunk);
                            for line in text.lines() {
                                if let Some(data) = line.strip_prefix("data: ") {
                                    if let Ok(msg) = serde_json::from_str::<Value>(data) {
                                        if let Some(id) = msg.get("id").and_then(|v| v.as_u64()) {
                                            let mut map = pending.lock().await;
                                            if let Some(tx) = map.remove(&id) {
                                                let _ = tx.send(msg);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("[{}] SSE reconnect failed: {}", server_name, e);
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
            }
        });

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
