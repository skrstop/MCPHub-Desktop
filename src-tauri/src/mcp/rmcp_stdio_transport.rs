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
    atomic::{AtomicBool, AtomicU64, Ordering},
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

/// Downstream-driven MRTR retry fields (hub wire convention: inside `_meta`).
/// Mirrors rmcp_http_transport::downstream_mrtr_retry.
fn downstream_mrtr_retry(
    meta: &Value,
) -> Option<(std::collections::BTreeMap<String, Value>, Option<String>)> {
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
    /// Bumped on every connect so an old drain task (pipe closing late after
    /// an intentional kill + reconnect reset) can tell it is stale and skip
    /// the "child stderr closed" warn.
    drain_generation: Arc<AtomicU64>,
    /// Upstream-advertised TTL from the most recent list_tools fetch
    /// (only positive single-page TTLs recorded — origin #1277 freshness).
    last_list_ttl: std::sync::Mutex<Option<u64>>,
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
            drain_generation: Arc::new(AtomicU64::new(0)),
            last_list_ttl: std::sync::Mutex::new(None),
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
        let generation = self.drain_generation.clone();
        let my_gen = generation.load(Ordering::SeqCst);
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
                    let mut tail = stderr_tail.lock().unwrap_or_else(|p| p.into_inner());
                    tail.push_str(&line);
                    tail.push('\n');
                    const CAP: usize = 32_768;
                    if tail.len() > CAP {
                        // Byte-offset truncation: advance to the next char
                        // boundary so drain() never panics on multi-byte UTF-8
                        // (a panic here would poison the mutex and break all
                        // future reconnections for this server).
                        let mut start = tail.len() - CAP;
                        while start < tail.len() && !tail.is_char_boundary(start) {
                            start += 1;
                        }
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
            // Suppress the warn for an intentional kill (flag set before the
            // tree is killed) or a stale drain (a reconnect already bumped the
            // generation — the fresh process owns the diagnostics now).
            if !intentional.load(Ordering::SeqCst) && generation.load(Ordering::SeqCst) == my_gen {
                app_logger::log_to_db("warn", &format!("[{}] child stderr closed (process exited?)", server_name));
            }
        });
    }
}

#[async_trait]
impl McpTransport for RmcpStdioTransport {
    async fn connect(&mut self) -> Result<()> {
        let connect_start = std::time::Instant::now();
        // Fresh tail per connect: the previous process's drain task may still
        // be flushing buffered lines into the shared Arc — swapping in a new
        // Arc keeps this connection's tail (and handshake-failure error)
        // free of stale lines (also makes a reused-instance reconnect sane).
        *self.stderr_tail.lock().unwrap_or_else(|p| p.into_inner()) = String::new();
        // Swap in a fresh Arc so a still-flushing previous drain task cannot
        // interleave stale lines into this connection's tail.
        self.stderr_tail = Arc::new(Mutex::new(String::new()));
        // A prior disconnect() on this instance must not suppress this
        // connection's crash diagnostics. Bump the drain generation so a
        // still-draining previous task (pipe closing late) knows it is stale.
        self.intentional_disconnect.store(false, Ordering::SeqCst);
        self.drain_generation.fetch_add(1, Ordering::SeqCst);

        // Resolve command to bundled binary if available (node, npx, uv, uvx, python…)
        let (resolved_cmd, resolved_args) = runtime_env::resolve_command(&self.command, &self.args);
        let resolve_msg = format!(
            "[{}] Command resolved: '{}' -> '{}', args: {:?}",
            self.server_name, self.command, resolved_cmd, resolved_args
        );
        log::info!("{}", resolve_msg);
        app_logger::log_to_db("info", &resolve_msg);

        // Merged environment: parent process + runtime overrides + user env.
        // PATH: user PATH is PREPENDED (user entries win) so users can pin a
        // specific node/python ahead of the bundled runtimes; resolve_command
        // already short-circuits to the bundled binary for known commands,
        // so this only affects PATH-only lookups (e.g. bare `npx` with no
        // bundled runtime installed).
        let mut merged_env: HashMap<String, String> = std::env::vars().collect();
        for (k, v) in runtime_env::env_overrides(&self.command, &self.server_name) {
            merged_env.insert(k, v);
        }
        for (k, v) in &self.env {
            if k.to_ascii_uppercase() == "PATH" {
                if let Some(existing) = merged_env.get("PATH") {
                    // Windows PATH separator is ';' — mirror resolve_in_path's
                    // platform handling (a hardcoded ':' fuses both sides into
                    // one bogus entry and breaks command resolution).
                    #[cfg(windows)]
                    let sep = ";";
                    #[cfg(not(windows))]
                    let sep = ":";
                    merged_env.insert(k.clone(), format!("{v}{sep}{existing}", v = v, sep = sep, existing = existing));
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
        // ⚠️ 必须在 builder 上声明 stderr(Stdio::piped())：rmcp 的
        // TokioChildProcessBuilder::spawn 会无条件用 builder 自己的 stdio
        // 配置覆盖 Command 的设置（child_process.rs:150-157），builder 默认
        // stderr=inherit —— 若只在 Command 上 piped，stderr 捕获为 None，
        // 下载进度事件 / 32KB stderr_tail / 日志截断全部静默失效。
        let (child_transport, stderr) =
            TokioChildProcess::builder(cmd).stderr(Stdio::piped()).spawn().map_err(|e| {
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
                // Kill the process tree BEFORE clearing the pid: handshake
                // failure ≠ process exit (invalid initialize response /
                // version mismatch leave the child running), and once the pid
                // is cleared the pool's error-path disconnect becomes a no-op
                // (pid.filter(>0)) — npx/uvx grandchildren would orphan.
                let pid_now = *self.pid.lock().unwrap();
                if let Some(pid) = pid_now.filter(|p| *p > 0) {
                    super::stdio_transport::kill_process_tree(pid);
                }
                *self.pid.lock().unwrap() = None;
                let tail = self.stderr_tail.lock().unwrap_or_else(|p| p.into_inner()).trim_end().to_string();
                if !tail.is_empty() {
                    anyhow!("{}\n--- upstream stderr ---\n{}", msg, tail)
                } else {
                    anyhow!(msg)
                }
            })?;

        // Capture server-reported version for the update-available check.
        // Empty version string → None so the update check doesn't treat
        // "unknown" as a real recorded version.
        if let Some(info) = service.peer_info() {
            self.server_version = info
                .server_info
                .as_ref()
                .map(|i| i.version.clone())
                .filter(|v| !v.is_empty());
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
            // Bounded cancel: a wedged worker must not stall disconnect
            // forever (the process tree was already killed above).
            if tokio::time::timeout(std::time::Duration::from_secs(5), service.cancel())
                .await
                .is_err()
            {
                log::warn!("[{}] rmcp service cancel timed out (5s)", self.server_name);
            }
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
        // Downstream-driven MRTR retry: inputResponses/requestState travel in
        // _meta on the hub wire but rmcp servers read the typed params fields —
        // lift them out (mirrors rmcp_http_transport).
        let mut meta_for_wire = request_meta.clone();
        if let Some(meta) = &request_meta {
            if let Some((input_responses, request_state)) = downstream_mrtr_retry(meta) {
                params.input_responses = Some(input_responses);
                params.request_state = request_state;
                if let Some(obj) = meta_for_wire.as_mut().and_then(|v| v.as_object_mut()) {
                    // Remove both spellings: the helper accepts the bare
                    // fallbacks too — leaving them on the wire would make an
                    // upstream rmcp server see the retry payload twice.
                    obj.remove("io.modelcontextprotocol/inputResponses");
                    obj.remove("io.modelcontextprotocol/requestState");
                    obj.remove("inputResponses");
                    obj.remove("requestState");
                }
            }
        }
        params.meta = request_meta_from_value(meta_for_wire);
        // Use call_tool_once + explicit response mapping (mirrors the HTTP
        // transport contract): Peer::call_tool hard-errors on
        // CallToolResponse::Task (service/client.rs UnexpectedResponse), but a
        // 2026 upstream MAY answer a plain tools/call with a task
        // (SEP-2663 server's choice) — poll it to terminal like HTTP does.
        let response = service
            .peer()
            .call_tool_once(params)
            .await
            .map_err(|e| anyhow!("tools/call failed: {}", e))?;
        match response {
            rmcp::model::CallToolResponse::Complete(r) => {
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
            rmcp::model::CallToolResponse::InputRequired(ir) => {
                let mut raw = serde_json::to_value(&ir).unwrap_or(Value::Object(Default::default()));
                if let Some(obj) = raw.as_object_mut() {
                    obj.insert(
                        "io.modelcontextprotocol/resultType".to_string(),
                        Value::String("input_required".into()),
                    );
                }
                Ok(ToolCallResult {
                    content: vec![serde_json::json!({"type":"text","text":"task requires input"})],
                    is_error: false,
                    structured_content: None,
                    raw_meta: Some(raw),
                })
            }
            rmcp::model::CallToolResponse::Task(create) => {
                // Upstream-supplied interval is clamped: 200ms floor (poll
                // storm) / 30s cap (a hostile huge value would blow the
                // 10min deadline in one sleep).
                let poll_interval =
                    create.task.poll_interval_ms.unwrap_or(1000).clamp(200, 30_000);
                let task_id = create.task.task_id.clone();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
                let mut transient_failures = 0u32;
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(poll_interval)).await;
                    // Recheck deadline AFTER the sleep — a clamped 30s sleep
                    // must not overshoot the deadline check.
                    if std::time::Instant::now() > deadline {
                        return Err(anyhow!("modern task '{}' poll timed out (10min)", task_id));
                    }
                    // Bounded tolerance for transient errors (review round 9).
                    let snap = match service
                        .peer()
                        .get_task(rmcp::model::GetTaskParams::new(task_id.clone()))
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
                            let mut raw = serde_json::to_value(&snap)
                                .unwrap_or(Value::Object(Default::default()));
                            if let Some(obj) = raw.as_object_mut() {
                                obj.insert(
                                    "io.modelcontextprotocol/inputRequests".to_string(),
                                    serde_json::to_value(input_requests).unwrap_or_default(),
                                );
                                // Override GetTaskResult's flattened
                                // resultType:"complete" so the bridge's MRTR
                                // gate (resultType == "input_required") fires —
                                // same as the direct-path InputRequired arm.
                                obj.insert(
                                    "io.modelcontextprotocol/resultType".to_string(),
                                    Value::String("input_required".into()),
                                );
                            }
                            return Ok(ToolCallResult {
                                content: vec![serde_json::json!(
                                    {"type":"text","text":"task requires input"}
                                )],
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
                            let structured_content = result
                                .get("structuredContent")
                                .cloned()
                                .filter(|v| !v.is_null());
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
            _ => Err(anyhow!("unexpected call_tool response")),
        }
    }
}
