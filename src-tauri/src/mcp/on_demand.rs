//! On-demand stdio server spawning - Rust mirror of origin PR #1012.
//!
//! When a stdio server config has `startOnDemand: true`, the shared pool
//! **does not** connect it at startup. Instead it sits "sleeping" (a pool
//! placeholder with `client: None`, `connected: false`, `start_on_demand: true`)
//! until a tool call arrives. The first tool call lazily builds + connects the
//! client here, caches it, runs the call, and arms an idle-shutdown timer. After
//! `idleTimeoutMs` (default 5 min) with no further calls the process is torn down
//! but the cached tool list is preserved in the pool placeholder so the server
//! stays discoverable and re-wakes on the next call.
//!
//! Scope (matches origin):
//! - Only stdio servers benefit (HTTP/SSE servers have no heavy process to keep
//!   alive). `pool::connect_server` gates the sleeping placeholder on
//!   `ServerType::Stdio`.
//! - Applies to the shared-pool call path (Tauri `call_tool` command + HTTP
//!   non-isolated `tools/call`). `perSessionClient` + `startOnDemand` is
//!   rejected at the service layer (mutually exclusive).
//!
//! Storage: a process-global `RwLock<HashMap<server_name, OnDemandEntry>>`. The
//! live client lives here (not in `PoolEntry.client`, which stays `None` for
//! on-demand servers); the pool entry is a "shadow" carrying status + cached
//! tools + the `start_on_demand` flag. A per-server creation lock mirrors
//! `session_pool::CREATE_LOCKS` so concurrent first-calls don't double-spawn.

use super::client::McpClient;
use super::pool;
use crate::models::server::{Tool, ToolCallResult};
use crate::services::{app_logger, server_service};
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
    time::Instant,
};
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time::{timeout, Duration};

/// Connect timeout for a freshly spawned on-demand client. Matches the shared
/// pool's 120s budget (npx/uvx first-run package downloads can be slow).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(120);

/// Default idle-shutdown delay when `idle_timeout_ms` is unset (5 min).
const DEFAULT_IDLE_MS: u64 = 300_000;

struct OnDemandEntry {
    client: Arc<Mutex<McpClient>>,
    /// Timestamp of the last successful tool call. Captured into idle-timer
    /// tasks as a "generation" so a stale timer can detect a newer call arrived.
    last_used: Instant,
    /// Cached idle-shutdown delay (ms) from the server config.
    idle_ms: u64,
    /// Handle to the pending idle-shutdown task. Aborted + replaced on every
    /// successful call to push the shutdown out.
    idle_handle: Mutex<Option<JoinHandle<()>>>,
}

type Store = Arc<RwLock<HashMap<String, OnDemandEntry>>>;

static ON_DEMAND_CLIENTS: OnceLock<Store> = OnceLock::new();

fn store() -> &'static Store {
    ON_DEMAND_CLIENTS.get_or_init(|| Arc::new(RwLock::new(HashMap::new())))
}

/// Per-server creation locks - prevents concurrent duplicate spawns for the
/// same server (mirrors `session_pool::CREATE_LOCKS`).
type CreateLocks = Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>;

static CREATE_LOCKS: OnceLock<CreateLocks> = OnceLock::new();

fn create_locks() -> &'static CreateLocks {
    CREATE_LOCKS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

/// Get-or-spawn the on-demand client for `server_name` and call `tool` with
/// `arguments`. On a connection-class failure the entry is evicted so the next
/// call rebuilds it. After a successful call the idle-shutdown timer is reset.
pub async fn call_tool_on_demand(
    server_name: &str,
    tool: &str,
    arguments: Value,
) -> Result<ToolCallResult> {
    // Fast path: a cached client exists. Clone the Arc + read idle_ms under a
    // read lock, then run the call outside the store lock.
    {
        let map = store().read().await;
        if let Some(entry) = map.get(server_name) {
            let client_arc = entry.client.clone();
            let idle_ms = entry.idle_ms;
            drop(map);
            log::debug!(
                "[on-demand] Reusing spawned client for '{}' (tool '{}')",
                server_name, tool
            );
            return run_call(server_name, &client_arc, tool, arguments, idle_ms).await;
        }
    }

    // Slow path: acquire (or reuse) a per-server creation lock so concurrent
    // first-calls serialize instead of each spawning a duplicate process.
    let create_lock = {
        let mut locks = create_locks().lock().await;
        locks
            .entry(server_name.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    };
    let _guard = create_lock.lock().await;

    // Double-check after acquiring the lock: another holder may have just
    // finished spawning the client.
    {
        let map = store().read().await;
        if let Some(entry) = map.get(server_name) {
            let client_arc = entry.client.clone();
            let idle_ms = entry.idle_ms;
            drop(map);
            log::debug!(
                "[on-demand] Spawned client created by a concurrent call for '{}' (tool '{}')",
                server_name, tool
            );
            return run_call(server_name, &client_arc, tool, arguments, idle_ms).await;
        }
    }

    log::info!(
        "[on-demand] No cached client for '{}', cold-starting (tool '{}')",
        server_name, tool
    );
    app_logger::log_to_db(
        "info",
        &format!(
            "[on-demand] Cold-starting on-demand server '{}' (tool '{}')",
            server_name, tool
        ),
    );

    // Build + connect a fresh client. The DB read happens only here (once per
    // wake), not on every call.
    let cfg = server_service::get_by_name(server_name)
        .await
        .map_err(|e| {
            let msg = format!("[on-demand] Failed to load config for '{}': {}", server_name, e);
            log::error!("{}", msg);
            app_logger::log_to_db("error", &msg);
            anyhow!("Failed to load config for on-demand server '{}': {}", server_name, e)
        })?
        .ok_or_else(|| {
            let msg = format!("[on-demand] Server '{}' not found", server_name);
            log::error!("{}", msg);
            app_logger::log_to_db("error", &msg);
            anyhow!("Server '{}' not found for on-demand call", server_name)
        })?;

    let idle_ms = cfg.idle_timeout_ms.unwrap_or(DEFAULT_IDLE_MS);

    let (client, tools, server_version) = match build_and_connect(&cfg).await {
        Ok(t) => t,
        Err(e) => {
            let msg = format!(
                "[on-demand] Cold-start connect failed for '{}': {}",
                server_name, e
            );
            log::warn!("{}", msg);
            app_logger::log_to_db("warn", &msg);
            // Reflect the failure on the pool placeholder so the frontend can
            // surface it (sleeping + error).
            pool::mark_on_demand_error(server_name, e.to_string()).await;
            return Err(e);
        }
    };

    let client_arc = Arc::new(Mutex::new(client));
    {
        let mut map = store().write().await;
        map.insert(
            server_name.to_string(),
            OnDemandEntry {
                client: client_arc.clone(),
                last_used: Instant::now(),
                idle_ms,
                idle_handle: Mutex::new(None),
            },
        );
    }
    // Mark the pool placeholder awake so status + cached tools reflect reality.
    pool::mark_on_demand_awake(server_name, tools, server_version).await;

    let msg = format!(
        "[on-demand] Server '{}' cold-started, ready for tool '{}'",
        server_name, tool
    );
    log::info!("{}", msg);
    app_logger::log_to_db("info", &msg);

    run_call(server_name, &client_arc, tool, arguments, idle_ms).await
}

/// Run `call_tool` on a cached on-demand client. On a connection-class error
/// the entry is evicted so the next call rebuilds. On success the idle-shutdown
/// timer is reset.
async fn run_call(
    server_name: &str,
    client_arc: &Arc<Mutex<McpClient>>,
    tool: &str,
    arguments: Value,
    idle_ms: u64,
) -> Result<ToolCallResult> {
    let call_start = Instant::now();
    let result = {
        let client = client_arc.lock().await;
        client.call_tool(tool, arguments).await
    };
    match result {
        Ok(r) => {
            let status = if r.is_error { "error" } else { "success" };
            log::debug!(
                "[on-demand] Tool '{}' on '{}' {} ({}ms)",
                tool,
                server_name,
                status,
                call_start.elapsed().as_millis()
            );
            // Update last_used (brief write lock) and reset the idle timer.
            {
                let mut map = store().write().await;
                if let Some(entry) = map.get_mut(server_name) {
                    entry.last_used = Instant::now();
                }
            }
            schedule_idle(server_name, idle_ms).await;
            Ok(r)
        }
        Err(e) => {
            // Heuristic: treat any call failure as a stale connection. Evict
            // the entry so the next call rebuilds (basic reconnect).
            let client_to_disconnect = {
                let mut map = store().write().await;
                map.remove(server_name).map(|e| e.client)
            };
            if let Some(arc) = client_to_disconnect {
                let mut client = arc.lock().await;
                let _ = client.disconnect().await;
            }
            // Mark the pool placeholder sleeping (keep cached tools) so the
            // server re-wakes on the next call.
            pool::mark_on_demand_sleeping(server_name).await;
            let msg = format!(
                "[on-demand] Tool '{}' call failed on '{}' ({}ms), evicted client: {}",
                tool,
                server_name,
                call_start.elapsed().as_millis(),
                e
            );
            log::warn!("{}", msg);
            app_logger::log_to_db("warn", &msg);
            Err(e)
        }
    }
}

/// Build a client from config, connect it within `CONNECT_TIMEOUT`, and fetch
/// its tool list + server version. On ANY failure (handshake error or timeout)
/// the half-built client is explicitly `disconnect()`-ed before returning the
/// error so the child process tree is reaped (not orphaned).
async fn build_and_connect(
    cfg: &crate::models::server::ServerConfig,
) -> Result<(McpClient, Vec<Tool>, Option<String>)> {
    log::info!(
        "[on-demand] Building + connecting on-demand client for '{}' (type={:?})",
        cfg.name, cfg.server_type
    );
    let mut client = pool::build_client(cfg)?;
    match timeout(CONNECT_TIMEOUT, client.connect()).await {
        Ok(Ok(())) => {
            let tools = client.list_tools().await.unwrap_or_default();
            let server_version = client.server_version();
            Ok((client, tools, server_version))
        }
        Ok(Err(e)) => {
            log::warn!(
                "[on-demand] Connect handshake failed for '{}', disconnecting half-built client: {}",
                cfg.name, e
            );
            let _ = client.disconnect().await;
            Err(e)
        }
        Err(_) => {
            log::warn!(
                "[on-demand] Connect timed out for '{}' ({}s), disconnecting half-built client",
                cfg.name,
                CONNECT_TIMEOUT.as_secs()
            );
            let _ = client.disconnect().await;
            Err(anyhow!(
                "On-demand client connect timed out after {}s",
                CONNECT_TIMEOUT.as_secs()
            ))
        }
    }
}

/// (Re)arm the idle-shutdown timer for `server_name`. Aborts any pending timer
/// first so each successful call pushes shutdown out by `idle_ms`. The timer
/// task captures the current `last_used` as a generation; if a newer call
/// resets `last_used` before the timer fires, the shutdown is skipped.
async fn schedule_idle(server_name: &str, idle_ms: u64) {
    let snapshot = {
        let map = store().read().await;
        map.get(server_name).map(|e| e.last_used)
    };
    let snapshot = match snapshot {
        Some(s) => s,
        None => return, // entry evicted; nothing to schedule
    };
    let name = server_name.to_string();
    let new_handle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(idle_ms)).await;
        shutdown_on_demand_idle(&name, snapshot).await;
    });
    // Swap the new handle in, aborting the previous one.
    let old_handle = {
        let map = store().read().await;
        if let Some(entry) = map.get(server_name) {
            let mut handle = entry.idle_handle.lock().await;
            handle.replace(new_handle)
        } else {
            // Entry evicted between the snapshot read and now: abort the timer
            // we just spawned (its shutdown will no-op anyway).
            new_handle.abort();
            None
        }
    };
    if let Some(old) = old_handle {
        old.abort();
    }
}

/// Idle-timer callback: shut the server down (disconnect process, keep cached
/// tools) unless a newer call reset `last_used` since this timer was armed.
pub async fn shutdown_on_demand_idle(server_name: &str, snapshot: Instant) {
    // Re-check last_used under the write lock. If it changed, a newer call
    // arrived after this timer was armed - abort the shutdown.
    let client_to_disconnect = {
        let mut map = store().write().await;
        let remove = match map.get(server_name) {
            Some(entry) => entry.last_used == snapshot,
            None => false,
        };
        if remove {
            map.remove(server_name).map(|e| e.client)
        } else {
            None
        }
    };
    let client_arc = match client_to_disconnect {
        Some(c) => c,
        None => {
            log::debug!(
                "[on-demand] Idle shutdown for '{}' skipped (newer call reset timer)",
                server_name
            );
            return;
        }
    };
    {
        let mut client = client_arc.lock().await;
        if let Err(e) = client.disconnect().await {
            log::warn!("[on-demand] Error disconnecting idle client for '{}': {}", server_name, e);
        }
    }
    // Mark the pool placeholder sleeping but KEEP cached tools discoverable.
    pool::mark_on_demand_sleeping(server_name).await;
    let msg = format!(
        "[on-demand] Server '{}' shut down after idle (tools cached for next wake-up)",
        server_name
    );
    log::info!("{}", msg);
    app_logger::log_to_db("info", &msg);
}

/// Lifecycle teardown (disable / reload / delete / update): remove the on-demand
/// client and disconnect its process. Does NOT touch the pool placeholder - the
/// caller (`pool::disconnect_server`) owns that. No-op if no client is cached.
pub async fn shutdown_on_demand_lifecycle(server_name: &str) {
    let client_to_disconnect = tear_down_on_demand_client(server_name).await;
    if let Some(client_arc) = client_to_disconnect {
        let mut client = client_arc.lock().await;
        if let Err(e) = client.disconnect().await {
            log::warn!(
                "[on-demand] Error disconnecting client for '{}' (lifecycle): {}",
                server_name, e
            );
        } else {
            log::info!("[on-demand] Disconnected client for '{}' (lifecycle)", server_name);
        }
    }
}

/// Remove the on-demand entry for `server_name` under the store write lock and
/// return its client Arc (caller does the disconnect I/O outside the lock).
/// Also drops the per-server creation lock so a later re-enable creates fresh.
async fn tear_down_on_demand_client(server_name: &str) -> Option<Arc<Mutex<McpClient>>> {
    let removed = {
        let mut map = store().write().await;
        map.remove(server_name).map(|e| e.client)
    };
    {
        let mut locks = create_locks().lock().await;
        locks.remove(server_name);
    }
    removed
}

/// Remove and disconnect **every** on-demand client. Called from
/// `pool::disconnect_all` at application shutdown so child processes are reaped
/// via `kill_process_tree` rather than relying solely on `kill_on_drop`. Best
/// effort: disconnect errors are logged, not propagated.
pub async fn cleanup_all_on_demand() {
    let removed: Vec<(String, Arc<Mutex<McpClient>>)> = {
        let mut map = store().write().await;
        map.drain().map(|(name, e)| (name, e.client)).collect()
    };
    {
        let mut locks = create_locks().lock().await;
        locks.clear();
    }
    if removed.is_empty() {
        return;
    }
    let msg = format!("[on-demand] Cleaning up {} on-demand client(s) (shutdown)", removed.len());
    log::info!("{}", msg);
    app_logger::log_to_db("info", &msg);
    for (name, client_arc) in removed {
        let mut client = client_arc.lock().await;
        if let Err(e) = client.disconnect().await {
            log::warn!("[on-demand] Error disconnecting client for '{}' (shutdown): {}", name, e);
        }
    }
}
