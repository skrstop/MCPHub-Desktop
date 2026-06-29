/// Streamable HTTP transport — MCP over plain HTTP POST with streaming responses.
use super::client::McpTransport;
use crate::models::server::{Tool, ToolCallResult};
use crate::services::app_logger;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, atomic::{AtomicU64, Ordering}},
};
use tokio::sync::Mutex;

static HTTP_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    HTTP_REQUEST_ID.fetch_add(1, Ordering::SeqCst)
}

pub struct HttpTransport {
    url: String,
    headers: HashMap<String, String>,
    client: Client,
    connected: bool,
    server_name: String,
    session_id: Arc<Mutex<String>>,
}

impl HttpTransport {
    pub fn new(
        server_name: impl Into<String>,
        url: impl Into<String>,
        headers: HashMap<String, String>,
    ) -> Self {
        let client = Client::builder()
            .build()
            .expect("Failed to build reqwest client");
        // mcp-session-id priority: server response > user-provided > generated UUID
        // Start with user-provided or empty (will be set from server response)
        let session_id = headers
            .get("mcp-session-id")
            .cloned()
            .unwrap_or_default();
        Self {
            server_name: server_name.into(),
            url: url.into(),
            headers,
            client,
            connected: false,
            session_id: Arc::new(Mutex::new(session_id)),
        }
    }

    async fn post(&self, method: &str, params: Value) -> Result<Value> {
        let id = next_id();
        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
            "options": {
                "resetTimeoutOnProgress": true
            },
        });

        let sid = self.session_id.lock().await.clone();
        let mut req = self.client.post(&self.url)
            .header("Content-Type", "application/json");
        if !sid.is_empty() {
            req = req.header("mcp-session-id", &sid);
        }
        req = req.json(&body);
        for (k, v) in &self.headers {
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

        // Retry on recoverable 4xx errors (408, 429)
        if status.is_client_error() && (status.as_u16() == 408 || status.as_u16() == 429) {
            log::warn!(
                "[{}] Received recoverable HTTP {}, retrying...",
                self.server_name,
                status
            );
            tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

            let sid = self.session_id.lock().await.clone();
            let mut req = self.client.post(&self.url)
                .header("Content-Type", "application/json");
            if !sid.is_empty() {
                req = req.header("mcp-session-id", &sid);
            }
            req = req.json(&body);
            for (k, v) in &self.headers {
                req = req.header(k, v);
            }
            let resp = req.send().await?;
            if !resp.status().is_success() {
                return Err(anyhow!("HTTP error after retry: {}", resp.status()));
            }
            let json: Value = resp.json().await?;
            if let Some(err) = json.get("error") {
                return Err(anyhow!("MCP error: {}", err));
            }
            return Ok(json["result"].clone());
        }

        if !status.is_success() {
            return Err(anyhow!("HTTP error: {}", status));
        }

        let json: Value = resp.json().await?;
        if let Some(err) = json.get("error") {
            return Err(anyhow!("MCP error: {}", err));
        }
        Ok(json["result"].clone())
    }
}

#[async_trait]
impl McpTransport for HttpTransport {
    async fn connect(&mut self) -> Result<()> {
        let conn_msg = format!("[{}] Connecting to HTTP endpoint: {}", self.server_name, self.url);
        log::info!("{}", conn_msg);
        app_logger::log_to_db("info", &conn_msg);

        self.post(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "mcphub-desktop", "version": "0.1.0" }
            }),
        )
        .await?;
        self.connected = true;

        let ok_msg = format!("[{}] HTTP transport connected", self.server_name);
        log::info!("{}", ok_msg);
        app_logger::log_to_db("info", &ok_msg);
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.connected = false;
        let msg = format!("[{}] HTTP transport disconnected", self.server_name);
        log::info!("{}", msg);
        app_logger::log_to_db("info", &msg);
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    async fn list_tools(&self) -> Result<Vec<Tool>> {
        let result = self.post("tools/list", json!({})).await?;
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
                enabled: true,
            })
            .collect();
        Ok(tools)
    }

    async fn call_tool(&self, name: &str, arguments: Value) -> Result<ToolCallResult> {
        let result = self
            .post("tools/call", json!({ "name": name, "arguments": arguments }))
            .await?;
        let content = result["content"].as_array().cloned().unwrap_or_default();
        let is_error = result["isError"].as_bool().unwrap_or(false);
        Ok(ToolCallResult { content, is_error })
    }
}
