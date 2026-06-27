/// stdio transport — spawns a child process and communicates via stdin/stdout
/// using the MCP JSON-RPC protocol framing.
use super::client::McpTransport;
use crate::models::server::{Tool, ToolCallResult};
use crate::services::{app_logger, runtime_env};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::{oneshot, Mutex},
};

static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// Resolve a command name to a full path by searching in PATH.
/// On Windows, also checks for .exe, .cmd, .bat extensions.
fn resolve_in_path(cmd: &str, path_env: &str) -> Option<String> {
    let sep = if cfg!(target_os = "windows") { ';' } else { ':' };
    let extensions: &[&str] = if cfg!(target_os = "windows") {
        &["", ".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };

    let dirs: Vec<&str> = path_env.split(sep).filter(|d| !d.is_empty()).collect();
    log::info!("[resolve_in_path] Searching for '{}' in {} PATH dirs", cmd, dirs.len());

    for dir in &dirs {
        let dir_path = std::path::Path::new(dir);
        // Check if directory exists first
        if !dir_path.exists() {
            continue;
        }
        for ext in extensions {
            let candidate = dir_path.join(format!("{}{}", cmd, ext));
            if candidate.exists() {
                log::info!("[resolve_in_path] Found: {}", candidate.display());
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }

    log::warn!("[resolve_in_path] '{}' not found in any PATH directory", cmd);
    // Log all dirs for debugging
    for (i, dir) in dirs.iter().enumerate() {
        let exists = std::path::Path::new(dir).exists();
        log::warn!("[resolve_in_path]   PATH[{}]: {} (exists={})", i, dir, exists);
    }
    None
}

/// Kill the entire process tree of a stdio transport's child process.
/// When the server is launched through a wrapper like `npx` / `npm exec`,
/// the wrapper does not forward signals to its descendants, so the real
/// server process is left running as an orphan. Walk the whole tree and
/// force-kill it.
fn kill_process_tree(pid: u32) {
    #[cfg(unix)]
    {
        // Send SIGTERM to the process group first
        unsafe {
            libc::kill(-(pid as i32), libc::SIGTERM);
        }
    }
    #[cfg(windows)]
    {
        // On Windows, use taskkill /F /T /PID to kill the process tree
        // CREATE_NO_WINDOW prevents a visible console window from flashing
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .creation_flags(0x0800_0000)
            .spawn();
    }
}

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
        // Resolve command to bundled binary if available (node, npx, uv, uvx, python…)
        let (resolved_cmd, resolved_args) =
            runtime_env::resolve_command(&self.command, &self.args);

        // Build the merged environment:
        //   1. Inherit the parent process environment (so child keeps HOME, USER, etc.)
        //   2. Apply runtime overrides (PATH prepend + cache dirs)
        //   3. Merge user-supplied server env; if user also supplies PATH, append it
        //      after our prepended dirs so our bundled binaries still take priority.
        let sep = if cfg!(target_os = "windows") { ";" } else { ":" };
        let mut merged_env: HashMap<String, String> = std::env::vars().collect();

        // Apply runtime overrides (PATH is prepended with bundled binary dirs)
        for (k, v) in runtime_env::env_overrides(&self.command) {
            merged_env.insert(k, v);
        }

        for (k, v) in &self.env {
            if k.to_ascii_uppercase() == "PATH" {
                if let Some(existing) = merged_env.get("PATH") {
                    merged_env.insert("PATH".to_string(), format!("{}{}{}", existing, sep, v));
                } else {
                    merged_env.insert(k.clone(), v.clone());
                }
            } else {
                merged_env.insert(k.clone(), v.clone());
            }
        }

        // Debug: log PATH for troubleshooting
        if let Some(path) = merged_env.get("PATH") {
            let path_entries: Vec<&str> = path.split(sep).filter(|d| !d.is_empty()).collect();
            log::warn!("[{}] Merged PATH has {} entries:", self.server_name, path_entries.len());
            for (i, entry) in path_entries.iter().enumerate() {
                log::warn!("[{}]   PATH[{}]: {}", self.server_name, i, entry);
            }
        }

        // Resolve the command to a full path using the merged PATH.
        // Command::new() uses the current process PATH, but we need to use
        // the merged PATH (which includes bundled dirs + user-added paths).
        let final_cmd = if resolved_cmd.contains('/') || resolved_cmd.contains('\\') {
            // Already a full path, use as-is
            log::info!("[{}] Command is already a full path: {}", self.server_name, resolved_cmd);
            resolved_cmd.clone()
        } else {
            // Search in merged PATH
            match resolve_in_path(&resolved_cmd, merged_env.get("PATH").map(|s| s.as_str()).unwrap_or("")) {
                Some(full_path) => {
                    log::info!("[{}] Resolved '{}' to '{}'", self.server_name, resolved_cmd, full_path);
                    full_path
                }
                None => {
                    log::warn!("[{}] Command '{}' not found in PATH, using as-is", self.server_name, resolved_cmd);
                    resolved_cmd.clone()
                }
            }
        };

        let spawn_msg = format!("[{}] spawning: {} {:?}", self.server_name, final_cmd, resolved_args);
        log::info!("{}", spawn_msg);
        app_logger::log_to_db("info", &spawn_msg);

        let mut cmd = Command::new(&final_cmd);
        cmd.args(&resolved_args)
            .envs(&merged_env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        // On Windows, suppress the console window that would otherwise flash on screen.
        // CREATE_NO_WINDOW (0x08000000) keeps stdio handles intact but prevents a visible window.
        #[cfg(windows)]
        {
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }

        // On Unix, create a new process group so we can kill the entire tree
        // (including npx/uvx wrapper children) on disconnect.
        #[cfg(unix)]
        {
            cmd.process_group(0);
        }

        // Use current working directory for the child process
        if let Ok(cwd) = std::env::current_dir() {
            cmd.current_dir(cwd);
        }

        let mut child = cmd.spawn()
            .map_err(|e| {
                let cmd_exists = std::path::Path::new(&resolved_cmd).exists();
                let err_msg = format!(
                    "[{}] Failed to spawn process: {}\n  command: {}\n  args: {:?}\n  cmd_exists: {}",
                    self.server_name, e, resolved_cmd, resolved_args, cmd_exists
                );
                log::error!("{}", err_msg);
                app_logger::log_to_db("error", &err_msg);
                e
            })?;
        let pid = child.id().unwrap_or(0);
        let spawn_ok_msg = format!("[{}] Process spawned (pid={})", self.server_name, pid);
        log::info!("{}", spawn_ok_msg);
        app_logger::log_to_db("info", &spawn_ok_msg);

        let stdin = child.stdin.take().ok_or_else(|| anyhow!("Failed to get stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("Failed to get stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| anyhow!("Failed to get stderr"))?;

        // Spawn stderr drain task — without this, the child process blocks
        // when the ~4KB pipe buffer fills up (npx/uvx write progress to stderr).
        let stderr_name = self.server_name.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                log::debug!("[{}] stderr: {}", stderr_name, line);
            }
        });

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
            let pid = child.id().unwrap_or(0);
            let msg = format!("[{}] Killing process (pid={})...", self.server_name, pid);
            log::info!("{}", msg);
            app_logger::log_to_db("info", &msg);

            // Kill the entire process group to ensure child processes spawned
            // by wrappers like npx/uvx are also terminated.
            if pid > 0 {
                kill_process_tree(pid);
            }
            child.kill().await.ok();

            let done_msg = format!("[{}] Process killed (pid={})", self.server_name, pid);
            log::info!("{}", done_msg);
            app_logger::log_to_db("info", &done_msg);
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
                enabled: true,
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
