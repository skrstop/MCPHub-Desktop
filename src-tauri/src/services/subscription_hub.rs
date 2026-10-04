//! MCP 2026-07-28 subscriptions hub.
//!
//! One place that owns every live `subscriptions/listen` stream. Callers:
//!  • `http_server.rs` — on a `subscriptions/listen` request, parses the
//!    client's opt-in filter, registers a subscriber, and drains the
//!    returned channel as the SSE stream body.
//!  • change points (tool enable/disable, server add/delete/reload, prompt
//!    create/update/delete, resource create/update/delete, RAG doc writes) —
//!    fire-and-forget `notify_*` calls.
//!  • `mcp_tasks.rs` — task terminal transitions push
//!    `notifications/tasks` to subscribers that opted in via the tasks
//!    extension (A4).
//!
//! The filter is honored strictly: a subscriber only ever receives
//! notification types it explicitly requested (spec MUST NOT). Notifications
//! carry `io.modelcontextprotocol/subscriptionId` in `_meta` (= the JSON-RPC
//! id of the originating `subscriptions/listen` request) so clients can
//! demultiplex concurrent subscriptions.

use serde_json::{json, Value};
use std::sync::OnceLock;
use tokio::sync::mpsc;

/// One live subscription. `tx` is the sender half of the stream the SSE
/// response body drains; when the client disconnects the receiver drops and
/// `send` fails, at which point we prune the entry.
struct Subscriber {
    /// The JSON-RPC id of the originating `subscriptions/listen` request —
    /// every notification for this subscription carries it in `_meta`
    /// (spec: io.modelcontextprotocol/subscriptionId).
    subscription_id: Value,
    filter: Value,
    tasks_opted_in: bool,
    /// Exact `taskIds` the subscriber opted into (empty = NOT opted in).
    /// Matching is strictly per-id so a subscriber of t1 never sees t2's state.
    task_ids: Vec<String>,
    tx: mpsc::UnboundedSender<String>,
}

type Subs = Vec<Subscriber>;

static SUBSCRIBERS: OnceLock<tokio::sync::RwLock<Subs>> = OnceLock::new();

/// Typed change events, consumed by the rmcp-native `subscriptions/listen`
/// streams (see `rmcp_bridge::listen`). The raw-string `Subs` registry above
/// serves the legacy hand-written SSE path semantics (kept for parity/tests);
/// the rmcp path forwards these into the typed `SubscriptionSink`.
#[derive(Debug, Clone)]
pub enum HubEvent {
    ToolsListChanged,
    PromptsListChanged,
    ResourcesListChanged,
    ResourceUpdated(String),
    TaskStatus(Value),
}

fn event_bus() -> &'static tokio::sync::broadcast::Sender<HubEvent> {
    static BUS: OnceLock<tokio::sync::broadcast::Sender<HubEvent>> = OnceLock::new();
    BUS.get_or_init(|| {
        let (tx, _) = tokio::sync::broadcast::channel(256);
        tx
    })
}

/// Subscribe to typed hub events (one receiver per live listen stream).
pub fn subscribe_events() -> tokio::sync::broadcast::Receiver<HubEvent> {
    event_bus().subscribe()
}

fn publish(ev: HubEvent) {
    // No receivers (or slow receivers) are fine — fire-and-forget.
    let _ = event_bus().send(ev);
}

fn subscribers() -> &'static tokio::sync::RwLock<Subs> {
    SUBSCRIBERS.get_or_init(|| tokio::sync::RwLock::new(Vec::new()))
}

/// What the server will honor for a given requested filter. Everything in the
/// filter schema is supported by the hub; the honored set simply echoes back
/// the requested booleans/URIs.
fn honored_filter(filter: &Value) -> Value {
    let mut v = json!({});
    if filter.get("toolsListChanged").and_then(|b| b.as_bool()) == Some(true) {
        v["toolsListChanged"] = json!(true);
    }
    if filter.get("promptsListChanged").and_then(|b| b.as_bool()) == Some(true) {
        v["promptsListChanged"] = json!(true);
    }
    if filter.get("resourcesListChanged").and_then(|b| b.as_bool()) == Some(true) {
        v["resourcesListChanged"] = json!(true);
    }
    if let Some(uris) = filter.get("resourceSubscriptions").and_then(|a| a.as_array()) {
        let uris: Vec<Value> = uris.iter().filter(|u| u.is_string()).cloned().collect();
        if !uris.is_empty() {
            v["resourceSubscriptions"] = Value::Array(uris);
        }
    }
    v
}

/// Register a new subscriber. Returns (acknowledged-filter, receiver). The
/// caller sends the `acknowledged` notification as the FIRST message on the
/// stream (spec MUST), then drains the receiver.
///
/// ⚠️ Production note: after the rmcp migration the hand-written
/// `subscriptions/listen` route is gone and the only live listen path is
/// `rmcp_bridge::HubBridge::listen` (bus-driven). No production code calls
/// this `register` today — the registry survives for unit tests and as the
/// drop-in delivery lane if a future non-rmcp listen route is added.
pub async fn register(
    subscription_id: Value,
    requested_filter: Value,
) -> (Value, mpsc::UnboundedReceiver<String>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let filter = honored_filter(&requested_filter);
    let task_ids: Vec<String> = requested_filter
        .get("taskIds")
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let tasks_opted_in = !task_ids.is_empty();
    let ack = filter_with_id(&filter, subscription_id.clone());
    subscribers().write().await.push(Subscriber {
        subscription_id,
        filter,
        tasks_opted_in,
        task_ids,
        tx,
    });
    (ack, rx)
}

fn filter_with_id(filter: &Value, id: Value) -> Value {
    json!({
        "_meta": {"io.modelcontextprotocol/subscriptionId": id},
        "notifications": filter,
    })
}

/// Remove dead subscribers (send fails ⇒ receiver dropped ⇒ client gone).
async fn prune(map: &mut Subs) {
    map.retain(|s| !s.tx.is_closed());
}

/// Broadcast a list-changed notification. Delivered only to subscribers whose
/// filter opted in to this exact notification type (spec: MUST NOT send types
/// the client did not request).
async fn broadcast_filtered(method: &str, filter_key: &str, params: Value) {
    let mut map = subscribers().write().await;
    prune(&mut map).await;
    for s in map.iter() {
        let opted_in = s
            .filter
            .get(filter_key)
            .and_then(|b| b.as_bool())
            .unwrap_or(false);
        if !opted_in {
            continue;
        }
        let mut p = params.clone();
        // Tag with this subscription's real request id (spec: every
        // notification on the stream carries
        // io.modelcontextprotocol/subscriptionId).
        if let Some(obj) = p.get_mut("_meta").and_then(|m| m.as_object_mut()) {
            obj.insert(
                "io.modelcontextprotocol/subscriptionId".to_string(),
                s.subscription_id.clone(),
            );
        }
        let body = json!({"jsonrpc": "2.0", "method": method, "params": p});
        let _ = s.tx.send(serde_json::to_string(&body).unwrap_or_default());
    }
}

/// tools/list content changed (tool enable/disable, server add/delete/reload
/// that changes the exposed tool set).
pub async fn notify_tools_list_changed() {
    super::rmcp_bridge::spawn_native_notify("tools");
    publish(HubEvent::ToolsListChanged);
    broadcast_filtered(
        "notifications/tools/list_changed",
        "toolsListChanged",
        json!({"_meta": {}}),
    )
    .await;
}

pub async fn notify_prompts_list_changed() {
    super::rmcp_bridge::spawn_native_notify("prompts");
    publish(HubEvent::PromptsListChanged);
    broadcast_filtered(
        "notifications/prompts/list_changed",
        "promptsListChanged",
        json!({"_meta": {}}),
    )
    .await;
}

pub async fn notify_resources_list_changed() {
    super::rmcp_bridge::spawn_native_notify("resources");
    publish(HubEvent::ResourcesListChanged);
    broadcast_filtered(
        "notifications/resources/list_changed",
        "resourcesListChanged",
        json!({"_meta": {}}),
    )
    .await;
}

/// A subscribed resource's content changed. `uri` must be in the subscriber's
/// `resourceSubscriptions` list to be delivered.
pub async fn notify_resource_updated(uri: &str) {
    publish(HubEvent::ResourceUpdated(uri.to_string()));
    // Legacy (2025-style) sessions subscribed via resources/subscribe get the
    // native notifications/resources/updated on their session stream.
    let uri_owned = uri.to_string();
    tokio::spawn(async move {
        super::rmcp_bridge::fan_out_legacy_resource_updated(&uri_owned).await;
    });
    let mut map = subscribers().write().await;
    prune(&mut map).await;
    for s in map.iter() {
        let subscribed = s
            .filter
            .get("resourceSubscriptions")
            .and_then(|a| a.as_array())
            .map(|a| a.iter().any(|u| u.as_str() == Some(uri)))
            .unwrap_or(false);
        if subscribed {
            let body = json!({
                "jsonrpc": "2.0",
                "method": "notifications/resources/updated",
                "params": {
                    "_meta": {"io.modelcontextprotocol/subscriptionId": s.subscription_id},
                    "uri": uri,
                },
            });
            let _ = s.tx.send(serde_json::to_string(&body).unwrap_or_default());
        }
    }
}

/// Task terminal state (A4). Delivered only to subscribers that opted in via
/// the tasks extension (`taskIds` in their filter).
pub async fn notify_task_status(task_json: Value) {
    publish(HubEvent::TaskStatus(task_json.clone()));
    let task_id = task_json.get("taskId").and_then(|t| t.as_str());
    let mut map = subscribers().write().await;
    prune(&mut map).await;
    for s in map.iter() {
        if s.tasks_opted_in
            && task_id.is_some_and(|id| s.task_ids.iter().any(|f| f == id))
        {
            let body = json!({
                "jsonrpc": "2.0",
                "method": "notifications/tasks",
                "params": {
                    "_meta": {"io.modelcontextprotocol/subscriptionId": s.subscription_id},
                    "task": task_json,
                },
            });
            let _ = s.tx.send(serde_json::to_string(&body).unwrap_or_default());
        }
    }
}

/// Total live subscriptions (diagnostics/logging).
pub async fn live_count() -> usize {
    let mut map = subscribers().write().await;
    prune(&mut map).await;
    map.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All filter/routing semantics verified in ONE test: the registry is a
    /// process-global, so concurrent #[tokio::test]s would receive each
    /// other's broadcasts and flake. Sequential assertions inside a single
    /// test keep it deterministic.
    #[tokio::test]
    async fn filter_routing_and_optin_semantics() {
        // S1: tools+prompts only
        let (f1, mut rx1) = register(
            json!(1),
            json!({"toolsListChanged": true, "promptsListChanged": true}),
        )
        .await;
        assert_eq!(f1["notifications"]["toolsListChanged"], json!(true));
        assert_eq!(f1["notifications"]["promptsListChanged"], json!(true));
        assert!(f1["notifications"].get("resourcesListChanged").is_none());
        assert_eq!(f1["_meta"]["io.modelcontextprotocol/subscriptionId"], json!(1));

        // S2: one resource uri
        let (_f2, mut rx2) = register(
            json!(2),
            json!({"resourceSubscriptions": ["file:///a.txt"]}),
        )
        .await;

        // S3: tasks opt-in via taskIds
        let (_f3, mut rx3) = register(json!(3), json!({"taskIds": ["t1"]})).await;

        // S4: no filter at all (empty listen) — receives nothing
        let (_f4, mut rx4) = register(json!(4), json!({})).await;

        // Fire everything.
        notify_tools_list_changed().await;
        notify_prompts_list_changed().await;
        notify_resources_list_changed().await;
        notify_resource_updated("file:///b.txt").await; // not subscribed
        notify_resource_updated("file:///a.txt").await; // S2 only
        notify_task_status(json!({"taskId": "t1", "status": "completed"})).await;

        // S1: exactly its two requested list-changed notifications.
        let a = rx1.recv().await.unwrap();
        let b = rx1.recv().await.unwrap();
        assert!(a.contains("notifications/tools/list_changed"), "{a}");
        assert!(a.contains(r#""io.modelcontextprotocol/subscriptionId":1"#), "{a}");
        assert!(b.contains("notifications/prompts/list_changed"), "{b}");
        assert!(rx1.try_recv().is_err(), "S1 must not receive anything else");

        // S2: only the subscribed URI update.
        let m = rx2.recv().await.unwrap();
        assert!(m.contains("notifications/resources/updated") && m.contains("file:///a.txt"), "{m}");
        assert!(rx2.try_recv().is_err(), "unrelated uri must not arrive");

        // S3: only the tasks notification.
        let m = rx3.recv().await.unwrap();
        assert!(m.contains("notifications/tasks") && m.contains("t1"), "{m}");
        assert!(rx3.try_recv().is_err());

        // S4: empty filter — nothing at all.
        assert!(rx4.try_recv().is_err());

        assert_eq!(live_count().await, 4);
    }

    /// Typed event bus (rmcp-native listen source): publishes reach
    /// subscribers that registered before the event.
    #[tokio::test]
    async fn event_bus_publishes_typed_events() {
        let mut rx = subscribe_events();
        // publish() directly: notify_* would also touch the global Subs
        // registry, perturbing the other test's ordering assumptions.
        publish(HubEvent::ToolsListChanged);
        publish(HubEvent::ResourceUpdated("file:///x.txt".into()));
        assert!(matches!(
            rx.try_recv(),
            Ok(HubEvent::ToolsListChanged)
        ));
        assert!(matches!(
            rx.try_recv(),
            Ok(HubEvent::ResourceUpdated(ref u)) if u == "file:///x.txt"
        ));
        assert!(rx.try_recv().is_err());
    }
}
