/// stdio transport — spawns a child process and communicates via stdin/stdout
/// using the MCP JSON-RPC protocol framing.
use super::client::McpTransport;
use crate::models::server::{Tool, ToolCallResult};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::{oneshot, Mutex},
};

static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    REQUEST_ID.fetch_add(1, Ordering::SeqCst)
}

pub struct StdioTransport {
    command: String,
    args: Vec<String>,
    env: HashMap<String, String>,
    child: Option<Child>,
    stdin: std::sync::Arc<Mutex<Option<tokio::process::ChildStdin>>>,
    pending: std::sync::Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
    connected: bool,
    server_name: String,
}

impl StdioTransport {
    pub fn new(
        server_name: impl Into<String>,
        command: impl Into<String>,
        args: Vec<String>,
        env: HashMap<String, String>,
    ) -> Self {
        Self {
            command: command.into(),
            args,
            env,
            child: None,
            stdin: std::sync::Arc::new(Mutex::new(None)),
            pending: std::sync::Arc::new(Mutex::new(HashMap::new())),
            connected: false,
            server_name: server_name.into(),
        }
    }

    /// Send a JSON-RPC request and wait for the response
    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = next_id();
        let msg = json!({
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

        // Write the message to stdin
        {
            let mut guard = self.stdin.lock().await;
            if let Some(stdin) = guard.as_mut() {
                let line = serde_json::to_string(&msg)? + "\n";
                stdin.write_all(line.as_bytes()).await?;
                stdin.flush().await?;
            } else {
                return Err(anyhow!("stdin not available"));
            }
        }

        let response = rx.await.map_err(|_| anyhow!("Response channel closed"))?;
        if let Some(err) = response.get("error") {
            return Err(anyhow!("MCP error: {}", err));
        }
        Ok(response["result"].clone())
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn connect(&mut self) -> Result<()> {
        let mut cmd = Command::new(&self.command);
        cmd.args(&self.args)
            .envs(&self.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("Failed to get stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("Failed to get stdout"))?;

        // Spawn reader task
        let pending = self.pending.clone();
        let server_name = self.server_name.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(msg) = serde_json::from_str::<Value>(&line) {
                    if let Some(id) = msg.get("id").and_then(|v| v.as_u64()) {
                        let mut map = pending.lock().await;
                        if let Some(tx) = map.remove(&id) {
                            let _ = tx.send(msg);
                        }
                    }
                } else {
                    log::debug!("[{}] stdout: {}", server_name, line);
                }
            }
            log::info!("[{}] stdout reader exited", server_name);
        });

        *self.stdin.lock().await = Some(stdin);
        self.child = Some(child);

        // MCP initialize handshake
        self.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "mcphub-desktop", "version": "0.1.0" }
            }),
        )
        .await?;

        self.connected = true;
        log::info!("[{}] stdio transport connected", self.server_name);
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.connected = false;
        if let Some(mut child) = self.child.take() {
            child.kill().await.ok();
        }
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    async fn list_tools(&self) -> Result<Vec<Tool>> {
        let result = self.request("tools/list", json!({})).await?;
        let tools_raw = result["tools"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let tools = tools_raw
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
            .request("tools/call", json!({ "name": name, "arguments": arguments }))
            .await?;

        let content = result["content"].as_array().cloned().unwrap_or_default();
        let is_error = result["isError"].as_bool().unwrap_or(false);
        Ok(ToolCallResult { content, is_error })
    }
}
