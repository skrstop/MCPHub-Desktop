//! MCP 2025-11-25 Tasks (experimental) — receiver-side state machine.
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

use crate::{
    mcp::{pool, session_pool},
    services::mcp_version::VersionStrategy,
};
use anyhow::Result;
use chrono::Utc;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};
use tokio::sync::RwLock;
use uuid::Uuid;

/// Default polling hint returned to clients (ms).
const DEFAULT_POLL_INTERVAL: u64 = 5000;
/// How long a terminal task is retained after completion/cancellation so the
/// client can still call `tasks/result`. The spec lets the server drop a
/// terminal task once the result has been retrieved; we hold it a fixed
/// window (5 min) regardless, to bound memory — see `spawn_ttl_sweeper`.
const TERMINAL_RETENTION_MS: i64 = 5 * 60 * 1000;

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
    if let Some(ttl_ms) = t.ttl {
        if let Ok(created) = chrono::DateTime::parse_from_rfc3339(&t.created_at) {
            if (now - created.with_timezone(&Utc)).num_milliseconds() > ttl_ms as i64 {
                return true;
            }
        }
    }
    if t.status.is_terminal() {
        if let Some(ref done) = t.completed_at {
            if let Ok(d) = chrono::DateTime::parse_from_rfc3339(done) {
                return (now - d.with_timezone(&Utc)).num_milliseconds() > TERMINAL_RETENTION_MS;
            }
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

/// Create a task wrapping a `tools/call`. Spawns the background upstream call;
/// returns a `working` snapshot immediately.
#[allow(clippy::too_many_arguments)]
pub async fn create_tool_task(
    server_name: String,
    tool_name: String,
    args: Value,
    session_id: Option<String>,
    is_isolated: bool,
    client_ip: String,
    strategy: &'static dyn VersionStrategy,
    ttl: Option<u64>,
) -> Value {
    let task_id = Uuid::new_v4().to_string();
    let now = now_rfc3339();
    let task = Task {
        task_id: task_id.clone(),
        status: TaskStatus::Working,
        status_message: Some("The operation is now in progress.".to_string()),
        created_at: now.clone(),
        last_updated_at: now,
        ttl,
        completed_at: None,
        result: None,
        error: None,
    };
    tasks().write().await.insert(task_id.clone(), task);
    let snapshot = to_json(&tasks().read().await.get(&task_id).cloned().unwrap());

    // Run the real upstream call in the background, then record the outcome.
    tokio::spawn(async move {
        let call_result = if is_isolated {
            let sid = session_id.as_deref().unwrap_or("");
            session_pool::call_tool_isolated(sid, &server_name, &tool_name, args.clone()).await
        } else {
            pool::call_tool(&server_name, &tool_name, args.clone()).await
        };

        // Record the outcome under a short-lived write lock, cloning out what
        // the activity log needs so the lock is not held across the DB write.
        let activity = {
            let mut map = tasks().write().await;
            let now = now_rfc3339();
            let Some(entry) = map.get_mut(&task_id) else {
                return; // swept by TTL before completion
            };
            // If the client already cancelled, discard the late result and
            // stay terminal. Spec: terminal states never transition.
            if entry.status.is_terminal() {
                return;
            }
            match call_result {
                Ok(r) => {
                    let mut call_resp = json!({"content": r.content, "isError": r.is_error});
                    if let Some(sc) = &r.structured_content {
                        call_resp["structuredContent"] = sc.clone();
                    }
                    entry.result = Some(strategy.shape_tool_call_result(call_resp));
                    entry.status = if r.is_error {
                        TaskStatus::Failed
                    } else {
                        TaskStatus::Completed
                    };
                }
                Err(e) => {
                    entry.error = Some(e.to_string());
                    entry.status = TaskStatus::Failed;
                }
            }
            entry.status_message = None;
            entry.last_updated_at = now.clone();
            entry.completed_at = Some(now);
            (
                if entry.status == TaskStatus::Completed { "success" } else { "error" },
                entry.result.clone(),
                entry.error.clone(),
            )
        };

        // Best-effort activity log for traceability in the log panel
        // (lock released; write_activity takes Option<&str> for error/source_ip).
        let _ = crate::services::log_service::write_activity(
            &server_name,
            &tool_name,
            None,
            activity.0,
            Some(args),
            activity.1,
            activity.2.as_deref(),
            Some(&client_ip),
        )
        .await;
    });

    snapshot
}

/// `tasks/get` — return the current Task snapshot, or None if unknown.
pub async fn get(task_id: &str) -> Option<Value> {
    tasks().read().await.get(task_id).map(|t| to_json(t))
}

/// `tasks/result` — return the stored CallToolResult (with
/// `_meta.related-task.taskId`) if terminal, else the current Task snapshot
/// for the client to keep polling.
pub async fn result(task_id: &str) -> Result<Value, (i32, String)> {
    let map = tasks().read().await;
    let Some(t) = map.get(task_id) else {
        return Err((-32602, format!("Task '{}' not found", task_id)));
    };
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
        Ok(to_json(t))
    }
}

/// `tasks/list` — all tasks (no pagination; spec allows it, desktop-scale ok).
pub async fn list_all() -> Value {
    let map = tasks().read().await;
    let tasks_json: Vec<Value> = map.values().map(to_json).collect();
    json!({"tasks": tasks_json, "nextCursor": Value::Null})
}

/// `tasks/cancel` — mark the task cancelled (terminal). Returns the updated
/// snapshot. Rejecting an already-terminal task is spec (-32602).
/// ponytail: the in-flight upstream call (reqwest/stdio) cannot be aborted;
/// cancel is status-level only — a late result is discarded in the background
/// task. Upgrading to a truly cancellable upstream would need transport-level
/// cancellation support.
pub async fn cancel(task_id: &str) -> Result<Value, (i32, String)> {
    let mut map = tasks().write().await;
    let Some(t) = map.get_mut(task_id) else {
        return Err((-32602, format!("Task '{}' not found", task_id)));
    };
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
    Ok(to_json(t))
}
