//! Per-session upstream client isolation — Rust mirror of origin `d74d1be` (#985).
//!
//! When a server config has `perSessionClient: true`, each downstream HTTP MCP
//! session (`mcp-session-id`) gets its **own dedicated upstream client /
//! connection / child process** instead of sharing the pool's single client.
//! This is intended for stateful servers (e.g. Playwright) where concurrent
//! sessions would otherwise trample each other's state.
//!
//! Scope (matches origin):
//! - Only the HTTP MCP JSON-RPC `tools/call` path has a session, so only that
//!   path is isolated here. REST endpoints and Tauri `call_tool` commands have
//!   no session and keep using the shared pool.
//! - `tools/list` always uses the shared pool's cached list (same tools for
//!   every session; isolation is unnecessary).
//!
//! Storage: a process-global `RwLock<HashMap<(session_id, server_name), Arc<Mutex<McpClient>>>>`.
//! A separate creation-lock map mirrors origin's `isolatedClientCreationLocks`
//! so concurrent first-calls for the same (session, server) don't spawn
//! duplicate connections. The client is wrapped in `Mutex` so `disconnect`
//! (which takes `&mut self`) is possible at cleanup time; calls within a single
//! session serialize on it, which is the natural (and safer) behavior for a
//! stateful server anyway.

use super::client::McpClient;
use super::pool;
use crate::models::server::ToolCallResult;
use crate::services::{app_logger, server_service};
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};
use tokio::sync::{Mutex, RwLock};
use tokio::time::{timeout, Duration};

/// Connect timeout for a freshly created isolated client. Matches the shared
/// pool's 120s budget (npx/uvx first-run package downloads can be slow).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(120);

type SessionKey = (String, String); // (session_id, server_name)

type Store = Arc<RwLock<HashMap<SessionKey, Arc<Mutex<McpClient>>>>>;

static SESSION_CLIENTS: OnceLock<Store> = OnceLock::new();

fn store() -> &'static Store {
    SESSION_CLIENTS.get_or_init(|| Arc::new(RwLock::new(HashMap::new())))
}

/// Per-(session, server) creation locks — prevents concurrent duplicate
/// connects for the same key (mirrors origin's `isolatedClientCreationLocks`).
type CreateLocks = Arc<Mutex<HashMap<SessionKey, Arc<Mutex<()>>>>>;

static CREATE_LOCKS: OnceLock<CreateLocks> = OnceLock::new();

fn create_locks() -> &'static CreateLocks {
    CREATE_LOCKS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

/// Get-or-create an isolated client for (session_id, server_name) and call
/// `tool` with `arguments`. On success the result is returned; on a
/// connection-class failure the stale entry is evicted so the next call
/// rebuilds it (basic reconnect — origin's fine-grained HTTP 40x/SSE retry is
/// not mirrored here).
pub async fn call_tool_isolated(
    session_id: &str,
    server_name: &str,
    tool: &str,
    arguments: Value,
) -> Result<ToolCallResult> {
    let key = (session_id.to_string(), server_name.to_string());

    // Fast path: a cached client exists. Clone the Arc out under a read lock,
    // then call_tool outside the map lock so other sessions aren't blocked.
    {
        let map = store().read().await;
        if let Some(client_arc) = map.get(&key) {
            let client_arc = client_arc.clone();
            drop(map);
            log::debug!(
                "[session-pool] Reusing isolated client for session {} -> {} (tool '{}')",
                session_id, server_name, tool
            );
            return run_call(&key, &client_arc, tool, arguments).await;
        }
    }

    // Slow path: acquire (or reuse) a per-key creation lock so concurrent
    // first-calls serialize instead of each building a duplicate client.
    let create_lock = {
        let mut locks = create_locks().lock().await;
        locks
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    };
    let _guard = create_lock.lock().await;

    // Double-check after acquiring the lock: another holder may have just
    // finished creating the client.
    {
        let map = store().read().await;
        if let Some(client_arc) = map.get(&key) {
            let client_arc = client_arc.clone();
            drop(map);
            log::debug!(
                "[session-pool] Isolated client created by a concurrent call for session {} -> {} (tool '{}')",
                session_id, server_name, tool
            );
            return run_call(&key, &client_arc, tool, arguments).await;
        }
    }

    log::info!(
        "[session-pool] No cached isolated client for session {} -> {}, creating new one (tool '{}')",
        session_id, server_name, tool
    );
    app_logger::log_to_db(
        "info",
        &format!(
            "[session-pool] Creating isolated upstream client for session {} -> server '{}' (tool '{}')",
            session_id, server_name, tool
        ),
    );

    // Build + connect a fresh client. The DB read happens only here (once per
    // session+server), not on every call.
    let cfg = server_service::get_by_name(server_name)
        .await
        .map_err(|e| {
            let msg = format!(
                "[session-pool] Failed to load config for isolated server '{}' (session {}): {}",
                server_name, session_id, e
            );
            log::error!("{}", msg);
            app_logger::log_to_db("error", &msg);
            anyhow!("Failed to load config for isolated server '{}': {}", server_name, e)
        })?
        .ok_or_else(|| {
            let msg = format!(
                "[session-pool] Server '{}' not found for isolated call (session {})",
                server_name, session_id
            );
            log::error!("{}", msg);
            app_logger::log_to_db("error", &msg);
            anyhow!("Server '{}' not found for isolated call", server_name)
        })?;

    let client = match build_and_connect(&cfg, session_id).await {
        Ok(c) => c,
        Err(e) => {
            let msg = format!(
                "[session-pool] Isolated client connect failed for session {} -> {}: {}",
                session_id, server_name, e
            );
            log::warn!("{}", msg);
            app_logger::log_to_db("warn", &msg);
            return Err(e);
        }
    };

    let client_arc = Arc::new(Mutex::new(client));
    {
        let mut map = store().write().await;
        map.insert(key.clone(), client_arc.clone());
    }
    let msg = format!(
        "[session-pool] Created isolated client for session {} -> {}",
        session_id, server_name
    );
    log::info!("{}", msg);
    app_logger::log_to_db("info", &msg);

    run_call(&key, &client_arc, tool, arguments).await
}

/// Build a client from config and connect it within `CONNECT_TIMEOUT`.
///
/// On ANY failure (handshake error or timeout) the half-built client is
/// explicitly `disconnect()`-ed before returning the error. For stdio this runs
/// `kill_process_tree` (SIGTERM to the process group), which also reaps
/// grandchildren spawned by npx/uvx wrappers — relying on `kill_on_drop` alone
/// would only SIGKILL the direct child and could orphan those grandchildren.
async fn build_and_connect(
    cfg: &crate::models::server::ServerConfig,
    session_id: &str,
) -> Result<McpClient> {
    log::info!(
        "[session-pool] Building + connecting isolated client for session {} -> {} (type={:?})",
        session_id, cfg.name, cfg.server_type
    );
    let mut client = pool::build_client(cfg)?;
    match timeout(CONNECT_TIMEOUT, client.connect()).await {
        Ok(Ok(())) => Ok(client),
        Ok(Err(e)) => {
            // Handshake failed after (possibly) spawning a child process. Tear
            // it down so the child tree is reaped, not orphaned.
            log::warn!(
                "[session-pool] Isolated connect handshake failed for session {} -> {}, disconnecting half-built client: {}",
                session_id, cfg.name, e
            );
            let _ = client.disconnect().await;
            Err(e)
        }
        Err(_) => {
            log::warn!(
                "[session-pool] Isolated connect timed out for session {} -> {} ({}s), disconnecting half-built client",
                session_id, cfg.name, CONNECT_TIMEOUT.as_secs()
            );
            let _ = client.disconnect().await;
            Err(anyhow!(
                "Isolated client connect timed out after {}s",
                CONNECT_TIMEOUT.as_secs()
            ))
        }
    }
}

/// Run `call_tool` on a cached isolated client. On a connection-class error the
/// entry is evicted so the next call rebuilds (basic reconnect).
async fn run_call(
    key: &SessionKey,
    client_arc: &Arc<Mutex<McpClient>>,
    tool: &str,
    arguments: Value,
) -> Result<ToolCallResult> {
    let call_start = std::time::Instant::now();
    let result = {
        let client = client_arc.lock().await;
        client.call_tool(tool, arguments).await
    };
    match result {
        Ok(r) => {
            let status = if r.is_error { "error" } else { "success" };
            log::debug!(
                "[session-pool] Isolated tool '{}' on session {} -> {} {} ({}ms)",
                tool, key.0, key.1, status, call_start.elapsed().as_millis()
            );
            Ok(r)
        }
        Err(e) => {
            // Heuristic: treat any call failure as a stale connection. Evict
            // the entry so the next call rebuilds. (Origin does fine-grained
            // HTTP 40x / SSE retry; we keep it simple — a single reconnect on
            // the next call.)
            let mut map = store().write().await;
            map.remove(key);
            drop(map);
            let msg = format!(
                "[session-pool] Isolated tool '{}' call failed for session {} -> {} ({}ms), evicted client: {}",
                tool, key.0, key.1, call_start.elapsed().as_millis(), e
            );
            log::warn!("{}", msg);
            app_logger::log_to_db("warn", &msg);
            Err(e)
        }
    }
}

/// Remove and disconnect all isolated clients belonging to a session.
///
/// Called when a downstream session ends (HTTP `DELETE /mcp`). No-op if the
/// session had no isolated clients. Uses a read-lock fast path first so the
/// common case (shared-pool mode, empty store) never takes the write lock.
/// Disconnect I/O happens after the write lock is released so other sessions
/// aren't blocked.
pub async fn cleanup_session(session_id: &str) {
    // Fast path: read-lock, bail out if nothing to clean for this session.
    // Avoids write-lock contention on DELETE in shared-pool mode (the store is
    // empty there).
    {
        let map = store().read().await;
        if !map.keys().any(|(sid, _)| sid == session_id) {
            drop(map);
            // Still drop any leftover creation locks for this session.
            drop_creation_locks_for_session(session_id).await;
            return;
        }
    }

    // Collect this session's client arcs under the write lock.
    let removed: Vec<Arc<Mutex<McpClient>>> = {
        let mut map = store().write().await;
        let keys: Vec<SessionKey> = map
            .keys()
            .filter(|(sid, _)| sid == session_id)
            .cloned()
            .collect();
        if keys.is_empty() {
            drop(map);
            drop_creation_locks_for_session(session_id).await;
            return;
        }
        let removed = keys
            .iter()
            .filter_map(|k| map.remove(k))
            .collect();
        drop(map);
        drop_creation_locks_for_session(session_id).await;
        removed
    };

    let msg = format!(
        "[session-pool] Cleaning up {} isolated client(s) for session {}",
        removed.len(),
        session_id
    );
    log::info!("{}", msg);
    app_logger::log_to_db("info", &msg);

    for client_arc in removed {
        let mut client = client_arc.lock().await;
        if let Err(e) = client.disconnect().await {
            log::warn!(
                "[session-pool] Error disconnecting isolated client for session {}: {}",
                session_id,
                e
            );
        }
    }
}

/// Remove and disconnect all isolated clients for a given server, across every
/// session.
///
/// Called from `pool::disconnect_server(name)` so that server lifecycle
/// operations (delete / disable / reload / update / reinstall) tear down the
/// per-session isolated clients too — otherwise those child processes leak
/// until a downstream DELETE or a failed call.
///
/// No-op when the store has no entry for `server_name` (the common case for
/// shared-pool servers): read-lock fast path, never takes the write lock.
pub async fn cleanup_server(server_name: &str) {
    // Fast path: read-lock, bail if no entry references this server.
    {
        let map = store().read().await;
        if !map.keys().any(|(_, sname)| sname == server_name) {
            return;
        }
    }

    // Collect the (session, server) keys to remove under the write lock.
    let removed: Vec<(SessionKey, Arc<Mutex<McpClient>>)> = {
        let mut map = store().write().await;
        let keys: Vec<SessionKey> = map
            .keys()
            .filter(|(_, sname)| sname == server_name)
            .cloned()
            .collect();
        if keys.is_empty() {
            return;
        }
        keys.into_iter()
            .filter_map(|k| map.remove(&k).map(|c| (k, c)))
            .collect()
    };

    // Also drop any leftover creation locks for these (session, server) pairs —
    // a connect may be in flight right now and would otherwise re-insert a
    // client for a server being deleted/reloaded. We clean by server name
    // across all sessions (matches the scope above).
    drop_creation_locks_for_server(server_name).await;

    let msg = format!(
        "[session-pool] Cleaning up {} isolated client(s) for server '{}' (lifecycle disconnect)",
        removed.len(),
        server_name
    );
    log::info!("{}", msg);
    app_logger::log_to_db("info", &msg);

    for ((sid, sname), client_arc) in removed {
        let mut client = client_arc.lock().await;
        if let Err(e) = client.disconnect().await {
            log::warn!(
                "[session-pool] Error disconnecting isolated client for session {} -> {}: {}",
                sid,
                sname,
                e
            );
        } else {
            log::info!(
                "[session-pool] Disconnected isolated client for session {} -> {} (server lifecycle)",
                sid, sname
            );
        }
    }
}

/// Remove leftover creation locks for a session (mirrors origin's lock cleanup
/// in `cleanupIsolatedSession`).
async fn drop_creation_locks_for_session(session_id: &str) {
    let mut locks = create_locks().lock().await;
    let keys: Vec<SessionKey> = locks
        .keys()
        .filter(|(sid, _)| sid == session_id)
        .cloned()
        .collect();
    for k in keys {
        locks.remove(&k);
    }
}

/// Remove and disconnect **every** isolated client across all sessions.
///
/// Called from `pool::disconnect_all` at application shutdown so the per-session
/// child processes are reaped via `kill_process_tree` rather than relying solely
/// on `kill_on_drop` at process exit. Best-effort: disconnect errors are logged,
/// not propagated, so one stuck client doesn't block the rest.
pub async fn cleanup_all() {
    // Fast path: read-lock, bail if the store is empty (common — most users
    // never enable perSessionClient).
    {
        let map = store().read().await;
        if map.is_empty() {
            return;
        }
    }

    // Drain the entire store under one write lock.
    let removed: Vec<(SessionKey, Arc<Mutex<McpClient>>)> = {
        let mut map = store().write().await;
        map.drain().collect()
    };
    // Also clear all creation locks.
    {
        let mut locks = create_locks().lock().await;
        locks.clear();
    }

    let msg = format!(
        "[session-pool] Cleaning up {} isolated client(s) (shutdown)",
        removed.len()
    );
    log::info!("{}", msg);
    app_logger::log_to_db("info", &msg);

    for ((sid, sname), client_arc) in removed {
        let mut client = client_arc.lock().await;
        if let Err(e) = client.disconnect().await {
            log::warn!(
                "[session-pool] Error disconnecting isolated client for session {} -> {} (shutdown): {}",
                sid,
                sname,
                e
            );
        } else {
            log::info!(
                "[session-pool] Disconnected isolated client for session {} -> {} (shutdown)",
                sid,
                sname
            );
        }
    }
}

/// Remove leftover creation locks for a server (across all sessions). Called
/// from `cleanup_server` so an in-flight connect for a server being
/// deleted/reloaded cannot re-insert a client after we just tore it down.
///
/// Note: a connect task already holding one of these locks will continue to
/// completion — `call_tool_isolated` re-checks the store under its own logic,
/// but does NOT re-validate that the server still exists after `get_by_name`.
/// The single in-flight connect is bounded (≤ CONNECT_TIMEOUT) and, if it
/// succeeds, its client is inserted into an already-cleaned store; the next
/// lifecycle op on that server will clean it again. This is acceptable — it
/// bounds leakage to at most one short-lived client per server lifecycle edge.
async fn drop_creation_locks_for_server(server_name: &str) {
    let mut locks = create_locks().lock().await;
    let keys: Vec<SessionKey> = locks
        .keys()
        .filter(|(_, sname)| sname == server_name)
        .cloned()
        .collect();
    for k in keys {
        locks.remove(&k);
    }
}
