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
/// Cap on the post-connect tools/list sweep (rmcp peer requests have no
/// default timeout); creation lock is held until this completes.
const TOOLS_LIST_TIMEOUT: Duration = Duration::from_secs(60);

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
    /// Number of tool calls currently in flight (COUNTER, not a bool). The
    /// idle timer must not tear the client down while this is > 0 — a call can
    /// legally run longer than `idle_ms` (pool upstream calls may take up to
    /// 600s), and the pre-call `last_used` bump otherwise keeps the timer's
    /// generation snapshot matching for the entire call duration, causing a
    /// deterministic teardown right after the call returns. A bool was wrong
    /// for CONCURRENT calls: the first finisher cleared the flag while the
    /// second was still running, letting the timer disconnect a live call.
    in_flight: u32,
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

    // Serialize with connect/disconnect on the same server so the cold start
    // cannot interleave with a disable/reload/delete cycle: without this, a
    // `disconnect_server` running concurrently would clean the pool shadow and
    // terminate lifecycle management while we spawn a fresh client — producing
    // an orphaned client that keeps its process alive until the idle timer
    // fires (and the late timer would then stomp on whatever entry replaced
    // the shadow). Holding the connect lock spans config load + spawn +
    // insert; mark_* mutators now also guard on entry identity.
    let _connect_guard = pool::pool_connect_lock(server_name).await;

    // Re-check the store after acquiring the connect lock: a racing cold
    // start that held the lock before us may have already spawned.
    {
        let map = store().read().await;
        if let Some(entry) = map.get(server_name) {
            let client_arc = entry.client.clone();
            let idle_ms = entry.idle_ms;
            drop(map);
            log::debug!(
                "[on-demand] Client for '{}' appeared while waiting for connect lock (tool '{}')",
                server_name, tool
            );
            return run_call(server_name, &client_arc, tool, arguments, idle_ms).await;
        }
    }

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
    // Mirror session_pool round-8: a disable/delete may land between the
    // shadow check above and connect completion (connect can take minutes for
    // slow npx/uvx installs) — inserting would keep a live process serving a
    // disabled server until the idle timer recycles it.
    match server_service::get_by_name(server_name).await {
        Ok(Some(c)) if c.enabled => {}
        _ => {
            let msg = format!(
                "[on-demand] Server '{}' disabled/removed during cold start; discarding fresh client (tool '{}')",
                server_name, tool
            );
            log::warn!("{}", msg);
            app_logger::log_to_db("warn", &msg);
            let _ = client_arc.lock().await.disconnect().await;
            pool::mark_on_demand_error(server_name, "server is disabled or removed".to_string())
                .await;
            return Err(anyhow!("Server '{}' is disabled or removed", server_name));
        }
    }
    {
        let mut map = store().write().await;
        map.insert(
            server_name.to_string(),
            OnDemandEntry {
                client: client_arc.clone(),
                last_used: Instant::now(),
                idle_ms,
                idle_handle: Mutex::new(None),
                in_flight: 0,
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
/// RAII guard for the `in_flight` busy flag (see `run_call`). On Drop —
/// including future cancellation — spawns a detached task that clears the
/// flag (generation-guarded) and re-arms the idle timer.
struct InFlightGuard {
    server_name: String,
    client_arc: Arc<Mutex<McpClient>>,
    idle_ms: u64,
    /// Set false on the normal completion path so Drop is a no-op.
    armed: bool,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let server_name = self.server_name.clone();
        let client_arc = self.client_arc.clone();
        let idle_ms = self.idle_ms;
        let _ = tokio::spawn(async move {
            let mut map = store().write().await;
            if let Some(entry) = map.get_mut(&server_name) {
                if Arc::ptr_eq(&entry.client, &client_arc) {
                    entry.in_flight = entry.in_flight.saturating_sub(1);
                    entry.last_used = Instant::now();
                }
            }
            drop(map);
            schedule_idle(&server_name, idle_ms).await;
        });
    }
}

async fn run_call(
    server_name: &str,
    client_arc: &Arc<Mutex<McpClient>>,
    tool: &str,
    arguments: Value,
    idle_ms: u64,
) -> Result<ToolCallResult> {
    let call_start = Instant::now();
    // Mark the entry busy + bump `last_used` before the call starts. The busy
    // flag is what actually protects long calls: a tool can legally run longer
    // than `idle_ms` (upstream calls may take up to 600s while the default
    // idle is 300s), and during the call `last_used` does not change — so the
    // timer's generation snapshot would keep matching and the entry would be
    // torn down the moment the call returned. The idle timer skips removal
    // while `in_flight` is set and re-arms instead.
    {
        let mut map = store().write().await;
        if let Some(entry) = map.get_mut(server_name) {
            // Generation guard (mirrors the clear path below): if this call's
            // entry was torn down (lifecycle) and cold-start-rebuilt before we
            // even got here, the map now holds the NEW generation — blindly
            // setting in_flight=true on it would stick forever (the stale
            // call's clear is ptr_eq-guarded), and the new generation's idle
            // timer would then never tear it down (process leak).
            if !Arc::ptr_eq(&entry.client, client_arc) {
                // The entry was lifecycle-torn-down and cold-start-rebuilt
                // while we were setting up. The comment above explains why we
                // must NOT touch the new generation here — but surfacing this
                // avoidable race to the downstream client is worse than
                // transparently re-entering the cold-start path against the
                // rebuilt entry (bounded: rebuild is rare, and the recursive
                // call re-runs the full generation checks).
                log::debug!("[on-demand] '{}' entry rebuilt during call setup; retrying", server_name);
                return Box::pin(call_tool_on_demand(server_name, tool, arguments)).await;
            }
            entry.last_used = Instant::now();
            entry.in_flight += 1;
        }
    }
    // NOTE: no await between the increment above and the guard construction
    // below — a cancellation in that window would leak the counter (neither
    // the manual clear nor the guard Drop would run). schedule_idle moved
    // after the guard for exactly this reason.
    // Cancellation guard: if this future is dropped mid-await (e.g. the 600s
    // `timeout_tool_call` wrapper fires while we're parked on the client
    // mutex or the upstream call), the manual clear below never runs and
    // `in_flight` would stick true forever — the idle timer would then never
    // tear the entry down (process leak). The guard's Drop spawns a detached
    // clear (ptr_eq-guarded) that runs even on cancellation. Disarmed on the
    // normal path so the manual clear below is the only actor.
    let mut guard = InFlightGuard {
        server_name: server_name.to_string(),
        client_arc: client_arc.clone(),
        idle_ms,
        armed: true,
    };
    // Re-arm AFTER the guard exists (moved from before it): the store
    // read/entry-mutex awaits are now covered — a cancellation there runs the
    // guard's Drop decrement instead of leaking the counter.
    schedule_idle(server_name, idle_ms).await;
    let result = {
        let client = client_arc.lock().await;
        client.call_tool(tool, arguments).await
    };
    guard.armed = false;
    // Clear the busy flag (both Ok and Err paths) before re-arming the timer.
    // Generation guard: if this call's entry was torn down (lifecycle) and
    // cold-start-rebuilt during the call, `map[server_name]` now holds the NEW
    // generation whose own run_call has set in_flight=true — blindly clearing
    // it by name would let the new generation's idle timer tear down a live
    // in-flight call.
    {
        let mut map = store().write().await;
        if let Some(entry) = map.get_mut(server_name) {
            if Arc::ptr_eq(&entry.client, client_arc) {
                entry.in_flight = entry.in_flight.saturating_sub(1);
                if result.is_ok() {
                    entry.last_used = Instant::now();
                }
            }
        }
    }
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
            // Reset the idle timer (last_used + in_flight already updated above).
            schedule_idle(server_name, idle_ms).await;
            Ok(r)
        }
        Err(e) => {
            // Evict ONLY on a genuinely broken connection (see session_pool
            // run_call): upstream JSON-RPC application errors keep the client
            // alive so the on-demand process (and any state it holds) survives
            // tool-level failures. NOTE: `is_connected()` only flips on
            // explicit connect/disconnect — upstream process death does not
            // lower it (best-effort check; the idle timer is the real reaper).
            let still_connected = client_arc.lock().await.is_connected();
            if still_connected {
                log::warn!(
                    "[on-demand] Tool '{}' failed on '{}' but connection healthy, keeping client: {}",
                    tool, server_name, e
                );
                return Err(e);
            }
            let client_to_disconnect = {
                let mut map = store().write().await;
                // Only evict THIS client (ptr_eq) — a concurrent cold start
                // may have replaced the entry between our read and now.
                let evict = map
                    .get(server_name)
                    .map(|e| Arc::ptr_eq(&e.client, client_arc))
                    .unwrap_or(false);
                if evict {
                    map.remove(server_name).map(|e| e.client)
                } else {
                    None
                }
            };
            let evicted = client_to_disconnect.is_some();
            if let Some(arc) = client_to_disconnect {
                let mut client = arc.lock().await;
                let _ = client.disconnect().await;
            }
            // Mark the pool placeholder sleeping (keep cached tools) only when
            // we actually evicted OUR entry — if the ptr_eq guard refused
            // (a concurrent cold start already replaced it), the store holds a
            // live awake client and the pool placeholder must not flip to
            // sleeping.
            if evicted {
                pool::mark_on_demand_sleeping(server_name).await;
            }
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
            // 60s cap on tools/list: rmcp peer requests have no default
            // timeout, and a server that completes initialize then stalls on
            // tools/list would hang here while the per-server creation lock is
            // still held — queuing every subsequent first-call forever.
            // Same guard as pool::connect_server; timeout counts as failure so
            // the half-built client is reaped.
            let tools = match timeout(TOOLS_LIST_TIMEOUT, client.list_tools_with_ttl()).await {
                Ok(Ok((ts, ttl))) => {
                    crate::services::list_freshness::record(&cfg.name, "tools", ttl);
                    ts
                }
                Ok(Err(e)) => {
                    log::warn!("[on-demand] '{}' tools/list failed: {}", cfg.name, e);
                    crate::services::list_freshness::record(&cfg.name, "tools", None);
                    Vec::new()
                }
                Err(_) => {
                    log::warn!(
                        "[on-demand] '{}' tools/list timed out after {}s; disconnecting half-built client",
                        cfg.name,
                        TOOLS_LIST_TIMEOUT.as_secs()
                    );
                    let _ = client.disconnect().await;
                    return Err(anyhow::anyhow!(
                        "tools/list timed out after {}s",
                        TOOLS_LIST_TIMEOUT.as_secs()
                    ));
                }
            };
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
/// Non-async timer spawner: keeps the schedule/shutdown future types from
/// forming a recursive opaque-type cycle (shutdown re-arms through this).
/// Re-arm path used by `shutdown_on_demand_idle`'s "newer call arrived"
/// branch: spawn the timer AND register its handle in the entry's
/// `idle_handle` slot, so a later `schedule_idle` can abort it via the
/// normal handle swap. An unregistered fire-and-forget re-arm would dodge
/// that mechanism for the rest of the entry's life.
#[allow(clippy::type_complexity)]
fn spawn_idle_timer_registered(
    server_name: String,
    idle_ms: u64,
    snapshot: Instant,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    // Returned as a type-erased boxed future: an `async fn` here would create
    // a compile-time type cycle with `shutdown_on_demand_idle` (shutdown →
    // re-arm → shutdown) that also fails the `Send` bound inference.
    Box::pin(async move {
        let map_key = server_name.clone();
        let new_handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(idle_ms)).await;
            shutdown_on_demand_idle(&server_name, snapshot).await;
        });
        let old = match store().write().await.get_mut(&map_key) {
            Some(entry) => entry.idle_handle.lock().await.replace(new_handle),
            None => {
                // Entry evicted between snapshot and now: abort the orphan
                new_handle.abort();
                None
            }
        };
        if let Some(old) = old {
            old.abort();
        }
    })
}

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
    // arrived after this timer was armed - abort the shutdown. A call in
    // flight (`in_flight`) also blocks removal: it may legally outlast
    // idle_ms, and tearing down right after it returns would drop a healthy
    // client and lose stateful server state.
    let client_to_disconnect = {
        let mut map = store().write().await;
        let remove = match map.get(server_name) {
            Some(entry) => entry.last_used == snapshot && entry.in_flight == 0,
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
            // A newer call reset last_used — but through the handle-swap race
            // in schedule_idle the surviving timer may be THIS (stale) one,
            // leaving NO live timer for the newer generation. Re-arm here so
            // idle shutdown can never be lost permanently.
            let idle_ms = {
                let map = store().read().await;
                map.get(server_name).map(|e| (e.idle_ms, e.last_used))
            };
            if let Some((idle_ms, last_used)) = idle_ms {
                log::debug!(
                    "[on-demand] Idle shutdown for '{}' skipped (newer call); re-arming timer",
                    server_name
                );
                // Re-arm via the registered helper: awaiting schedule_idle
                // here would form a recursive future (schedule → spawned timer
                // → this fn → schedule) whose opaque type never computes.
                spawn_idle_timer_registered(server_name.to_string(), idle_ms, last_used).await;
            }
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
/// The per-server creation lock is intentionally KEPT in the map: a cold-start
/// holds that lock across connect(120s)+list_tools(60s), and removing it
/// mid-flight lets a concurrent call create a second lock and a second
/// cold-start — the loser's client then overwrites the winner's store entry
/// and is abandoned without disconnect (orphaned stdio grandchildren). The
/// map is bounded by the server count; reuse across re-enables is harmless.
async fn tear_down_on_demand_client(server_name: &str) -> Option<Arc<Mutex<McpClient>>> {
    let removed = {
        let mut map = store().write().await;
        map.remove(server_name).map(|e| e.client)
    };
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
