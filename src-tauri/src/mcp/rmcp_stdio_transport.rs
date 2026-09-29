//! rmcp-backed stdio transport — replaces the hand-written JSON-RPC framing in
//! `stdio_transport.rs` with the official rmcp client while keeping every
//! desktop-specific behaviour byte-for-byte:
//!
//! - `runtime_env::resolve_command` / `env_overrides` (bundled Node/Python)
//! - merged environment (inherit + runtime overrides + user env, PATH priority)
//! - process-group spawn (Unix) + CREATE_NO_WINDOW (Windows)
//! - stderr drain: download-progress events for npx/uvx, 32KB rolling tail
//!   (§3.9 upstream stderr in handshake errors), 2000-char line cap
//! - process-tree kill on disconnect (npx/uvx wrapper children)
//! - `serverInfo.version` capture for the update-available check
//!
//! What changes: request framing, pending-request map, initialize handshake
//! and response correlation are delegated to rmcp (`serve_client` +
//! `Peer<RoleClient>`). MRTR `input_required` retry is native
//! (`Peer::call_tool` drives `inputResponses` round-trips).

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use rmcp::model::{CallToolRequestParams, RequestMetaObject};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::child_process::TokioChildProcess;
use serde_json::Value;
use tokio::io::AsyncBufReadExt;
use tokio::process::Command;
use std::process::Stdio;

use crate::mcp::client::McpTransport;
use crate::models::server::{Tool, ToolCallResult};
use crate::services::{app_logger, runtime_env};

/// Empty client handler — the hub only makes requests upstream, it never
/// handles server-initiated requests/notifications.
#[derive(Default)]
struct BridgeClientHandler;

impl rmcp::ClientHandler for BridgeClientHandler {}

fn request_meta_from_value(meta: Option<Value>) -> Option<RequestMetaObject> {
    meta.and_then(|m| serde_json::from_value(m).ok())
}

pub struct RmcpStdioTransport {
    server_name: String,
    command: String,
    args: Vec<String>,
    env: HashMap<String, String>,
    service: Option<RunningService<RoleClient, BridgeClientHandler>>,
    pid: Mutex<Option<u32>>,
    connected: bool,
    /// Rolling stderr tail (~32KB) so handshake failures include the upstream
    /// stderr (mirrors origin #1015 behaviour of StdioTransport).
    stderr_tail: Arc<Mutex<String>>,
    server_version: Option<String>,
    intentional_disconnect: Arc<AtomicBool>,
}

impl RmcpStdioTransport {
    pub fn new(server_name: impl Into<String>, command: String, args: Vec<String>, env: HashMap<String, String>) -> Self {
        Self {
            server_name: server_name.into(),
            command,
            args,
            env,
            service: None,
            pid: Mutex::new(None),
            connected: false,
            stderr_tail: Arc::new(Mutex::new(String::new())),
            server_version: None,
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Drain stderr with the same behaviour as StdioTransport: log lines,
    /// accumulate the tail, emit throttled download-progress events for
    /// npx/uvx. Runs until the pipe closes.
    fn spawn_stderr_drain(
        &self,
        stderr: tokio::process::ChildStderr,
        is_pkg_mgr: bool,
    ) {
        let server_name = self.server_name.clone();
        let stderr_tail = self.stderr_tail.clone();
        let intentional = self.intentional_disconnect.clone();
        tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(stderr).lines();
            let mut last_emit: Option<std::time::Instant> = None;
            let mut last_pct: Option<u8> = None;
            let mut last_msg = String::new();
            while let Ok(Some(line)) = lines.next_line().await {
                let shown = if line.len() > 2000 {
                    let mut end = 2000;
                    while !line.is_char_boundary(end) {
                        end += 1;
                    }
                    &line[..end]
                } else {
                    &line[..]
                };
                log::info!("[{}] stderr: {}", server_name, shown);
                app_logger::log_to_db("info", &format!("[stderr] {}", shown));
                {
                    let mut tail = stderr_tail.lock().unwrap();
                    tail.push_str(&line);
                    tail.push('\n');
                    const CAP: usize = 32_768;
                    if tail.len() > CAP {
                        let start = tail.len() - CAP;
                        tail.drain(..start);
                    }
                }
                if is_pkg_mgr && super::stdio_transport::looks_like_download_progress(&line) {
                    let pct = super::stdio_transport::parse_progress_pct(&line);
                    let now = std::time::Instant::now();
                    let time_ok = last_emit
                        .map(|t| now.duration_since(t) >= std::time::Duration::from_millis(300))
                        .unwrap_or(true);
                    let pct_changed = pct != last_pct;
                    let msg_changed = line != last_msg;
                    if pct_changed || (pct.is_none() && time_ok && msg_changed) {
                        last_emit = Some(now);
                        last_pct = pct;
                        last_msg = line.clone();
                        crate::mcp::progress::emit_install_progress(&crate::mcp::progress::ServerInstallProgress {
                            server: server_name.clone(),
                            phase: "downloading".to_string(),
                            progress: pct,
                            message: Some(line.clone()),
                        });
                    }
                }
            }
            if !intentional.load(Ordering::SeqCst) {
                app_logger::log_to_db("warn", &format!("[{}] child stderr closed (process exited?)", server_name));
            }
        });
    }
}

#[async_trait]
impl McpTransport for RmcpStdioTransport {
    async fn connect(&mut self) -> Result<()> {
        let connect_start = std::time::Instant::now();

        // Resolve command to bundled binary if available (node, npx, uv, uvx, python…)
        let (resolved_cmd, resolved_args) = runtime_env::resolve_command(&self.command, &self.args);
        let resolve_msg = format!(
            "[{}] Command resolved: '{}' -> '{}', args: {:?}",
            self.server_name, self.command, resolved_cmd, resolved_args
        );
        log::info!("{}", resolve_msg);
        app_logger::log_to_db("info", &resolve_msg);

        // Merged environment: parent process + runtime overrides + user env
        // (PATH appended after our prepended dirs so bundled binaries win).
        let mut merged_env: HashMap<String, String> = std::env::vars().collect();
        for (k, v) in runtime_env::env_overrides(&self.command, &self.server_name) {
            merged_env.insert(k, v);
        }
        for (k, v) in &self.env {
            if k.to_ascii_uppercase() == "PATH" {
                if let Some(existing) = merged_env.get("PATH") {
                    merged_env.insert(k.clone(), format!("{v}{sep}{existing}", v = v, sep = ":", existing = existing));
                } else {
                    merged_env.insert(k.clone(), v.clone());
                }
            } else {
                merged_env.insert(k.clone(), v.clone());
            }
        }

        // Resolve to a full path via the merged PATH.
        let final_cmd = if resolved_cmd.contains('/') || resolved_cmd.contains('\\') {
            resolved_cmd.clone()
        } else {
            match super::stdio_transport::resolve_in_path(
                &resolved_cmd,
                merged_env.get("PATH").map(|s| s.as_str()).unwrap_or(""),
            ) {
                Some(full_path) => full_path,
                None => resolved_cmd.clone(),
            }
        };
        log::info!(
            "[{}] Spawning process: {} {} (original: {} {})",
            self.server_name, final_cmd, resolved_args.join(" "), self.command, self.args.join(" ")
        );

        let mut cmd = Command::new(&final_cmd);
        cmd.args(&resolved_args)
            .envs(&merged_env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        #[cfg(unix)]
        {
            cmd.process_group(0);
        }
        if let Ok(cwd) = std::env::current_dir() {
            cmd.current_dir(cwd);
        }

        let is_pkg_mgr = self.command == "npx" || self.command == "uvx";
        let (child_transport, stderr) =
            TokioChildProcess::builder(cmd).spawn().map_err(|e| {
                let msg = format!(
                    "[{}] Failed to spawn process: {}\n  command: {}\n  args: {:?}",
                    self.server_name, e, resolved_cmd, resolved_args
                );
                log::error!("{}", msg);
                app_logger::log_to_db("error", &msg);
                anyhow!(msg)
            })?;
        let pid = child_transport.id().unwrap_or(0);
        *self.pid.lock().unwrap() = Some(pid);
        app_logger::log_to_db(
            "info",
            &format!("[{}] Process spawned (pid={}) in {:.1}s", self.server_name, pid, connect_start.elapsed().as_secs_f64()),
        );

        if let Some(stderr) = stderr {
            self.spawn_stderr_drain(stderr, is_pkg_mgr);
        }

        // rmcp runs the initialize handshake inside `serve_client`.
        let service = rmcp::service::serve_client(BridgeClientHandler, child_transport)
            .await
            .map_err(|e| {
                let elapsed = connect_start.elapsed();
                let msg = format!(
                    "[{}] MCP initialize handshake failed after {:.1}s: {}",
                    self.server_name,
                    elapsed.as_secs_f64(),
                    e
                );
                log::error!("{}", msg);
                app_logger::log_to_db("error", &msg);
                let tail = self.stderr_tail.lock().unwrap().trim_end().to_string();
                if !tail.is_empty() {
                    anyhow!("{}\n--- upstream stderr ---\n{}", msg, tail)
                } else {
                    anyhow!(msg)
                }
            })?;

        // Capture server-reported version for the update-available check.
        if let Some(info) = service.peer_info() {
            self.server_version = Some(info.server_info.as_ref().map(|i| i.version.clone()).unwrap_or_default());
        }

        self.service = Some(service);
        self.connected = true;
        app_logger::log_to_db(
            "info",
            &format!(
                "[{}] Connected | total={:.1}s (rmcp stdio)",
                self.server_name,
                connect_start.elapsed().as_secs_f64()
            ),
        );
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.connected = false;
        self.intentional_disconnect.store(true, Ordering::SeqCst);
        // Kill the process tree first (npx/uvx wrapper children survive plain
        // kill-on-drop), then cancel the rmcp worker.
        let pid = *self.pid.lock().unwrap();
        if let Some(pid) = pid.filter(|p| *p > 0) {
            app_logger::log_to_db(
                "info",
                &format!("[{}] Killing process tree (pid={})...", self.server_name, pid),
            );
            super::stdio_transport::kill_process_tree(pid);
        }
        if let Some(service) = self.service.take() {
            service.cancel().await.ok();
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        *self.pid.lock().unwrap() = None;
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
        let result = service.peer().list_tools(None).await?;
        Ok(result
            .tools
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
            })
            .collect())
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
        params.arguments = Some(
            arguments
                .as_object()
                .cloned()
                .unwrap_or_default(),
        );
        params.meta = request_meta_from_value(request_meta);
        // Peer::call_tool natively drives MRTR input_required retries
        // (inputResponses/request_state round-trips, bounded rounds).
        let result = service.peer().call_tool(params).await?;
        let r = result;
        let structured_content = r.structured_content.clone().filter(|v| !v.is_null());
        Ok(ToolCallResult {
            content: r
                .content
                .iter()
                .filter_map(|c| serde_json::to_value(c).ok())
                .collect(),
            is_error: r.is_error.unwrap_or(false),
            structured_content,
            raw_meta: r.meta.as_ref().and_then(|m| serde_json::to_value(m).ok()),
        })
    }
}
