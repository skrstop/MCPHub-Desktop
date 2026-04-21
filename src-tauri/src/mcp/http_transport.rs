/// Streamable HTTP transport — MCP over plain HTTP POST with streaming responses.
use super::client::McpTransport;
use crate::models::server::{Tool, ToolCallResult};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};

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
        Self {
            server_name: server_name.into(),
            url: url.into(),
            headers,
            client,
            connected: false,
        }
    }

    async fn post(&self, method: &str, params: Value) -> Result<Value> {
        let id = next_id();
        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let mut req = self.client.post(&self.url).json(&body);
        for (k, v) in &self.headers {
            req = req.header(k, v);
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Err(anyhow!("HTTP error: {}", resp.status()));
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
        log::info!("[{}] HTTP transport connected", self.server_name);
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
