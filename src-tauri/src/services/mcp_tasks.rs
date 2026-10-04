//! MCP Tasks receiver-side state machine (2025-11-25 core + 2026-07-28
//! extension shapes).
//!
//! Task terminal transitions (complete/fail/cancel) publish TaskStatus on
//! the hub bus. The rmcp-native subscription bus deliberately drops
//! TaskStatus events (upstream rmcp 3.4.1 does not yet route tasks through
//! `subscriptions/listen` — `SubscriptionFilter` has no taskIds lane), and
//! the legacy hand-written subscriber registry was removed with the rmcp
//! migration, so **task status is poll-only in practice**: clients observe
//! state via `tasks/get` / `tasks/result`.
//!
//! The hub acts as the receiver for downstream clients: a client may
//! *augment* a `tools/call` request with a `task` field
//! (`{ "task": { "ttl": 60000 } }`). The hub immediately returns a
//! `CreateTaskResult` carrying a `taskId` + `working` status, runs the real
//! upstream call in the background, and exposes `tasks/get` | `tasks/result`
//! | `tasks/list` | `tasks/cancel` for polling / retrieval / listing /
//! cancellation.
//!
//! Tasks live in memory (lost on restart, by design). The status state
//! machine follows the spec exactly: `working → {input_required, completed,
//! failed, cancelled}`, `input_required → {working, completed, failed,
//! cancelled}`, terminal states (`completed`/`failed`/`cancelled`) never
//! transition.
//!
//! Cancellation is a status-level concept: the in-flight upstream call itself
//! (reqwest/stdio) cannot be interrupted, so `cancel` marks the task
//! `cancelled` and discards any late result. See the `ponytail:` note in
//! `cancel`.

use anyhow::Result;
use chrono::Utc;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};
use tokio::sync::RwLock;

/// Default polling hint returned to clients (ms).
const DEFAULT_POLL_INTERVAL: u64 = 5000;
/// How long a terminal task is retained after completion/cancellation so the
/// client can still call `tasks/result`. The spec lets the server drop a
/// terminal task once the result has been retrieved; we hold it a fixed
/// window (5 min) regardless, to bound memory — see `spawn_ttl_sweeper`.
const TERMINAL_RETENTION_MS: i64 = 5 * 60 * 1000;
/// Hard backstop for tasks stuck in `working`: a task with no client TTL
/// whose upstream call hangs (or whose background job panicked without
/// calling complete/fail) would otherwise live in the map forever — the TTL
/// branch only fires when a TTL is present, and terminal retention only
/// applies to terminal tasks. 24h from creation is far above any legitimate
/// upstream call (600s max) yet still bounds memory.
const WORKING_MAX_AGE_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TaskStatus {
    Working,
    // Reserved for the `input_required` status (spec: a task may pause to ask
    // the requestor for input). Not produced yet — no upstream path triggers
    // it — but kept so the state machine stays complete.
    #[allow(dead_code)]
    InputRequired,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatus {
    fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Working => "working",
            TaskStatus::InputRequired => "input_required",
            TaskStatus::Completed => "completed",
            TaskStatus::Failed => "failed",
            TaskStatus::Cancelled => "cancelled",
        }
    }

    fn is_terminal(&self) -> bool {
        matches!(self, TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled)
    }
}

#[derive(Clone)]
pub struct Task {
    task_id: String,
    status: TaskStatus,
    status_message: Option<String>,
    created_at: String, // RFC3339 UTC
    last_updated_at: String,
    ttl: Option<u64>, // ms
    /// RFC3339 when the task entered a terminal state (completed/failed/
    /// cancelled). Set on terminal transition; the sweeper uses it to drop
    /// the task (and its stored result/error) after `TERMINAL_RETENTION_MS`
    /// so terminal tasks don't leak across a long-running hub.
    completed_at: Option<String>,
    /// Shaped CallToolResult, filled when the upstream call succeeds. Stored
    /// already shaped by the negotiated strategy so `tasks/result` returns it
    /// verbatim.
    result: Option<Value>,
    /// Error message, filled when the upstream call fails.
    error: Option<String>,
    /// Owning bearer key id. `None` = unauthenticated/local user (visible to
    /// everyone); `Some(k)` = only requests carrying bearer key `k` may
    /// see/get/result/cancel/list this task. Prevents cross-key result
    /// leakage through `tasks/list` + `tasks/get` on multi-key deployments.
    owner: Option<String>,
}

/// Ownership gate: a task is visible to the caller when it has no owner
/// (local/unauthenticated) or the caller carries the same bearer key.
fn authorized(t: &Task, caller: Option<&str>) -> bool {
    match (&t.owner, caller) {
        (None, _) => true,
        (Some(owner), Some(c)) => owner == c,
        (Some(_), None) => false,
    }
}

type TaskMap = HashMap<String, Task>;

static TASKS: OnceLock<Arc<RwLock<TaskMap>>> = OnceLock::new();

fn tasks() -> &'static Arc<RwLock<TaskMap>> {
    TASKS.get_or_init(|| Arc::new(RwLock::new(HashMap::new())))
}

/// Now as an RFC3339 UTC string (used for `createdAt` / `lastUpdatedAt`).
fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

/// Whether a task should be swept. A task is dropped when either:
///  • it passed its TTL (measured from `created_at`), or
///  • it has been terminal (completed/failed/cancelled) for longer than
///    `TERMINAL_RETENTION_MS` — bounds memory so finished tasks don't pile up.
fn is_expired(t: &Task) -> bool {
    let now = Utc::now();
    // u64 TTL can't be represented as i64 → treat as "no expiry in practice"
    // (a client sending ttl >= 2^63 must not wrap negative and insta-sweep
    // the task right after creation).
    let ttl_i64 = t.ttl.and_then(|ms| i64::try_from(ms).ok());
    if !t.status.is_terminal() {
        if let Ok(created) = chrono::DateTime::parse_from_rfc3339(&t.created_at) {
            let age_ms = (now - created.with_timezone(&Utc)).num_milliseconds();
            if let Some(ttl) = ttl_i64 {
                if age_ms > ttl {
                    return true;
                }
            }
            // Backstop: even TTL-less working tasks age out at 24h, so a
            // hung upstream call / dead background job can't leak memory.
            if age_ms > WORKING_MAX_AGE_MS {
                return true;
            }
        }
        return false;
    }
    // Terminal tasks: the module guarantees a fixed retention window
    // regardless of TTL — a client-side ttl is a *suggested* wait time, not a
    // deletion deadline, so the retention clock never expires earlier than
    // max(ttl, TERMINAL_RETENTION) from completion.
    if let Some(ref done) = t.completed_at {
        if let Ok(d) = chrono::DateTime::parse_from_rfc3339(done) {
            let retention = ttl_i64
                .map(|ttl| ttl.max(TERMINAL_RETENTION_MS))
                .unwrap_or(TERMINAL_RETENTION_MS);
            return (now - d.with_timezone(&Utc)).num_milliseconds() > retention;
        }
    }
    false
}

/// Spawn the TTL/retention sweeper. Call once at HTTP server startup. Every
/// 60s drops tasks whose TTL expired OR which have been terminal longer than
/// `TERMINAL_RETENTION_MS`.
/// ponytail: full-table scan; fine for desktop-scale task counts. Switch to
/// a per-deadline heap if the set grows large.
pub fn spawn_ttl_sweeper() {
    // Idempotency guard: start() may run multiple times (port/limit changes
    // restart the HTTP server) — each start previously spawned a fresh 60s
    // interval task and the old one never exited, leaking one loop per restart.
    static SWEEPER_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if SWEEPER_STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            let mut map = tasks().write().await;
            let expired: Vec<String> = map
                .iter()
                .filter(|(_, t)| is_expired(t))
                .map(|(id, _)| id.clone())
                .collect();
            for id in &expired {
                map.remove(id);
            }
            if !expired.is_empty() {
                log::debug!("[tasks] TTL sweeper dropped {} expired task(s)", expired.len());
            }
        }
    });
}

/// Create a task in `working` state (client-directed `tools/call` with a
/// `task` field). Returns the new task id. The caller runs the upstream call
/// in the background and finishes the task via [`complete`] / [`fail`].
/// Bounded: at most [`MAX_TASKS`] live entries (each can hold a full tool
/// result payload) — the sweeper alone allows unbounded memory growth under a
/// high-frequency task-creating caller (review round 8, 2026-10-04). When the
/// cap is hit, the oldest terminal task is evicted; with none evictable the
/// creation is rejected.
pub async fn create(ttl_ms: Option<u64>, owner: Option<String>) -> Result<String, String> {
    const MAX_TASKS: usize = 512;
    let mut map = tasks().write().await;
    if map.len() >= MAX_TASKS {
        // Prefer evicting the oldest terminal (already delivered) task.
        let evict = map
            .iter()
            .filter(|(_, t)| t.status.is_terminal())
            .min_by_key(|(_, t)| t.created_at.clone())
            .map(|(id, _)| id.clone());
        match evict {
            Some(id) => {
                map.remove(&id);
                log::warn!("[tasks] cap {MAX_TASKS} hit — evicted terminal task {id}");
            }
            None => return Err("too many active tasks; retry later".to_string()),
        }
    }
    let task_id = uuid::Uuid::new_v4().to_string();
    let now = now_rfc3339();
    map.insert(
        task_id.clone(),
        Task {
            task_id: task_id.clone(),
            status: TaskStatus::Working,
            status_message: None,
            created_at: now.clone(),
            last_updated_at: now,
            ttl: ttl_ms,
            completed_at: None,
            result: None,
            error: None,
            owner,
        },
    );
    Ok(task_id)
}

/// Fill the stored result and move a task to `completed`.
pub async fn complete(task_id: &str, result: Value) {
    let snapshot = {
        let mut map = tasks().write().await;
        match map.get_mut(task_id) {
            Some(t) if !t.status.is_terminal() => {
                t.result = Some(result);
                t.status = TaskStatus::Completed;
                t.completed_at = Some(now_rfc3339());
                t.last_updated_at = now_rfc3339();
                Some(to_json(t))
            }
            _ => None,
        }
    };
    // Published outside the lock: the hub bus keeps the TaskStatus event for
    // diagnostics/tests; active delivery is poll-only (see module doc).
    if let Some(snapshot) = snapshot {
        super::subscription_hub::notify_task_status(snapshot).await;
    }
}

/// Record an error and move a task to `failed`.
pub async fn fail(task_id: &str, error: String) {
    let snapshot = {
        let mut map = tasks().write().await;
        match map.get_mut(task_id) {
            Some(t) if !t.status.is_terminal() => {
                t.error = Some(error);
                t.status = TaskStatus::Failed;
                t.completed_at = Some(now_rfc3339());
                t.last_updated_at = now_rfc3339();
                Some(to_json(t))
            }
            _ => None,
        }
    };
    if let Some(snapshot) = snapshot {
        super::subscription_hub::notify_task_status(snapshot).await;
    }
}

/// Serialize a Task as the spec `Task` object (no internal fields).
pub fn to_json(t: &Task) -> Value {
    let mut v = json!({
        "taskId": t.task_id,
        "status": t.status.as_str(),
        "createdAt": t.created_at,
        "lastUpdatedAt": t.last_updated_at,
        "pollInterval": DEFAULT_POLL_INTERVAL,
    });
    if let Some(ttl) = t.ttl {
        v["ttl"] = json!(ttl);
    }
    if let Some(ref msg) = t.status_message {
        v["statusMessage"] = json!(msg);
    }
    v
}

/// Serialize a Task as the 2026-07-28 extension `Task` shape: flat fields with
/// `ttlMs` / `pollIntervalMs` naming (2025-11 uses `ttl` / `pollInterval`).
pub fn to_json_ext(t: &Task) -> Value {
    let mut v = json!({
        "taskId": t.task_id,
        "status": t.status.as_str(),
        "createdAt": t.created_at,
        "lastUpdatedAt": t.last_updated_at,
        "pollIntervalMs": DEFAULT_POLL_INTERVAL,
    });
    // ttlMs is optional in the spec — omit when unset instead of emitting an
    // explicit null (which trips strict client validators; the 2025-11 shape
    // already omits it for symmetry).
    if let Some(ttl) = t.ttl {
        v["ttlMs"] = json!(ttl);
    }
    if let Some(ref msg) = t.status_message {
        v["statusMessage"] = json!(msg);
    }
    v
}

/// `tasks/update` (2026-07-28 extension): accept client `inputResponses` for
/// outstanding `inputRequests`. The hub never produces `inputRequests` (its
/// tasks run straight through upstream calls), so per spec we acknowledge with
/// an empty result and ignore the payloads; a task parked in
/// `input_required` would resume to `working`.
pub async fn update_input(
    task_id: &str,
    _input_responses: Value,
    caller: Option<&str>,
) -> Result<(), (i32, String)> {
    let mut map = tasks().write().await;
    let Some(t) = map.get_mut(task_id) else {
        return Err((-32602, format!("Task '{}' not found", task_id)));
    };
    if !authorized(t, caller) {
        return Err((-32602, format!("Task '{}' not found", task_id)));
    }
    if t.status == TaskStatus::InputRequired {
        t.status = TaskStatus::Working;
        t.status_message = Some("Client input received; resuming.".to_string());
        t.last_updated_at = now_rfc3339();
    } else if t.status.is_terminal() {
        // Terminal task: silently acking would make the client believe its
        // input was accepted and the task resumed, when nothing will ever
        // happen. Fail loudly so the client surfaces the mismatch (e.g. a
        // task completed while the client was preparing its input).
        return Err((
            -32602,
            format!("Task '{}' is terminal ({}) and cannot accept input", task_id, t.status.as_str()),
        ));
    }
    Ok(())
}

/// `tasks/get` — return the current Task snapshot, or None if unknown or
/// owned by a different bearer key.
pub async fn get(task_id: &str, caller: Option<&str>) -> Option<Value> {
    let map = tasks().read().await;
    map.get(task_id)
        .filter(|t| authorized(t, caller))
        .map(to_json)
}

/// `tasks/get` (2026 ext shape) with the same ownership gate.
pub async fn get_ext(task_id: &str, caller: Option<&str>) -> Option<Value> {
    let map = tasks().read().await;
    let t = map.get(task_id).filter(|t| authorized(t, caller))?;
    let mut v = to_json_ext(t);
    match t.status {
        TaskStatus::Completed => {
            let mut r = t.result.clone().unwrap_or_else(|| json!({"content": []}));
            // Every 2026 result carries resultType — embed it in the
            // CallToolResult too, not just the tasks/get envelope.
            if let Some(obj) = r.as_object_mut() {
                obj.entry("resultType").or_insert(json!("complete"));
            }
            v["result"] = r;
        }
        TaskStatus::Failed => {
            // Failed requires an `error` object. Prefer the transport error
            // message; if the failure was an isError:true tool result, derive
            // the message from the stored result text.
            let msg = t.error.clone().unwrap_or_else(|| {
                t.result
                    .as_ref()
                    .map(|r| serde_json::to_string(r).unwrap_or_default())
                    .unwrap_or_else(|| "task failed".to_string())
            });
            v["error"] = json!({"code": -32000, "message": msg});
        }
        _ => {}
    }
    Some(v)
}

/// `tasks/result` — return the stored CallToolResult (with
/// `_meta.related-task.taskId`) if terminal, else the current Task snapshot
/// for the client to keep polling. Ownership-gated.
pub async fn result(task_id: &str, caller: Option<&str>, modern: bool) -> Result<Value, (i32, String)> {
    let map = tasks().read().await;
    let Some(t) = map.get(task_id) else {
        return Err((-32602, format!("Task '{}' not found", task_id)));
    };
    if !authorized(t, caller) {
        return Err((-32602, format!("Task '{}' not found", task_id)));
    }
    if t.status.is_terminal() {
        if t.status == TaskStatus::Cancelled {
            return Err((-32602, format!("Task '{}' was cancelled", task_id)));
        }
        if t.status == TaskStatus::Failed {
            if let Some(ref e) = t.error {
                return Ok(json!({
                    "isError": true,
                    "content": [{"type":"text","text": e.clone()}],
                    "_meta": {"io.modelcontextprotocol/related-task": {"taskId": t.task_id}}
                }));
            }
        }
        // Completed (or failed without a stored error): return the stored
        // shaped CallToolResult, annotated with the related-task metadata.
        let mut r = t.result.clone().unwrap_or_else(|| json!({"content": []}));
        if let Some(obj) = r.as_object_mut() {
            obj.entry("_meta")
                .or_insert(json!({}))
                .as_object_mut()
                .map(|m| {
                    m.insert(
                        "io.modelcontextprotocol/related-task".to_string(),
                        json!({"taskId": t.task_id}),
                    );
                });
        }
        Ok(r)
    } else {
        // Still working — return the task snapshot so the client can poll.
        // 2026-07-28 sessions speak the extension field names (pollIntervalMs/
        // ttlMs); legacy 2025-11 sessions keep the core names.
        Ok(if modern { to_json_ext(t) } else { to_json(t) })
    }
}

/// `tasks/list` — all tasks visible to the caller (no pagination; spec allows
/// it, desktop-scale ok). Ownership-gated: tasks owned by another bearer key
/// are hidden; unowned tasks are visible to everyone.
pub async fn list_all(caller: Option<&str>, modern: bool) -> Value {
    let map = tasks().read().await;
    let tasks_json: Vec<Value> = map
        .values()
        .filter(|t| authorized(t, caller))
        .map(|t| if modern { to_json_ext(t) } else { to_json(t) })
        .collect();
    json!({"tasks": tasks_json, "nextCursor": Value::Null})
}

/// `tasks/cancel` — mark the task cancelled (terminal). Returns the updated
/// snapshot. Rejecting an already-terminal task is spec (-32602).
/// ponytail: the in-flight upstream call (reqwest/stdio) cannot be aborted;
/// cancel is status-level only — a late result is discarded in the background
/// task. Upgrading to a truly cancellable upstream would need transport-level
/// cancellation support.
pub async fn cancel(task_id: &str, caller: Option<&str>) -> Result<Value, (i32, String)> {
    let mut map = tasks().write().await;
    let Some(t) = map.get_mut(task_id) else {
        return Err((-32602, format!("Task '{}' not found", task_id)));
    };
    if !authorized(t, caller) {
        return Err((-32602, format!("Task '{}' not found", task_id)));
    }
    if t.status.is_terminal() {
        return Err((
            -32602,
            format!("Task '{}' already in terminal state: {}", task_id, t.status.as_str()),
        ));
    }
    t.status = TaskStatus::Cancelled;
    t.status_message = Some("The task was cancelled by request.".to_string());
    let now = now_rfc3339();
    t.last_updated_at = now.clone();
    t.completed_at = Some(now);
    let snapshot = to_json(t);
    drop(map);
    super::subscription_hub::notify_task_status(snapshot.clone()).await;
    Ok(snapshot)
}

#[cfg(test)]
mod ownership_tests {
    use super::*;

    fn task(owner: Option<&str>) -> Task {
        Task {
            task_id: "t".into(),
            status: TaskStatus::Working,
            status_message: None,
            created_at: now_rfc3339(),
            last_updated_at: now_rfc3339(),
            ttl: None,
            completed_at: None,
            result: None,
            error: None,
            owner: owner.map(|s| s.to_string()),
        }
    }

    #[test]
    fn unowned_task_visible_to_everyone() {
        assert!(authorized(&task(None), None));
        assert!(authorized(&task(None), Some("key-a")));
        assert!(authorized(&task(None), Some("key-b")));
    }

    #[test]
    fn owned_task_visible_only_to_owner() {
        assert!(authorized(&task(Some("key-a")), Some("key-a")));
        assert!(!authorized(&task(Some("key-a")), Some("key-b")));
        // Unauthenticated callers never see keyed tasks.
        assert!(!authorized(&task(Some("key-a")), None));
    }
}

#[cfg(test)]
mod ttl_expiry_tests {
    use super::*;

    fn task_with(ttl: Option<u64>, status: TaskStatus, completed_ago_ms: i64) -> Task {
        let now = Utc::now();
        Task {
            task_id: "t".into(),
            status,
            status_message: None,
            created_at: (now - chrono::Duration::milliseconds(1000)).to_rfc3339(),
            last_updated_at: now.to_rfc3339(),
            ttl,
            completed_at: Some((now - chrono::Duration::milliseconds(completed_ago_ms)).to_rfc3339()),
            result: None,
            error: None,
            owner: None,
        }
    }

    #[test]
    fn huge_ttl_does_not_wrap_negative() {
        // ttl >= 2^63 must not wrap to a negative i64 and insta-sweep the task
        let t = task_with(Some(u64::MAX), TaskStatus::Working, 0);
        assert!(!is_expired(&t));
    }

    #[test]
    fn terminal_task_outlives_short_ttl() {
        // Terminal retention: a 1s TTL must not cut the 5-min terminal window
        let t = task_with(Some(1_000), TaskStatus::Completed, 60_000);
        assert!(!is_expired(&t));
        // ...but after max(ttl, TERMINAL_RETENTION) it is swept
        let t2 = task_with(Some(1_000), TaskStatus::Completed, TERMINAL_RETENTION_MS + 1_000);
        assert!(is_expired(&t2));
    }

    #[test]
    fn terminal_task_with_long_ttl_uses_ttl() {
        let t = task_with(Some((TERMINAL_RETENTION_MS * 2) as u64), TaskStatus::Completed, TERMINAL_RETENTION_MS + 1_000);
        assert!(!is_expired(&t));
    }

    #[test]
    fn working_task_with_no_ttl_never_expires() {
        let t = task_with(None, TaskStatus::Working, 0);
        assert!(!is_expired(&t));
    }
}
