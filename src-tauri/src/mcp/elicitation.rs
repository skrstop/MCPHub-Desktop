//! Elicitation forwarding for isolated (`perSessionClient`) upstream
//! connections — desktop mirror of origin dd05c30 (#1370).
//!
//! Scope (matches origin): only tool calls on per-session isolated upstream
//! connections forward `elicitation/create` to the downstream client. Shared
//! pool connections never register a relay, so an upstream elicitation there
//! fails with the default "Elicitation requires an active tool call" error.
//!
//! Mechanics: the rmcp client handler (`BridgeClientHandler` in the stdio /
//! HTTP transports) reads a per-connection [`ElicitationSlot`]. The session
//! pool registers a per-call [`ElicitationRelay`] (built by the bridge from
//! the downstream `Peer<RoleServer>` + the downstream client's declared
//! elicitation modes) just before the tool call and clears it when the call
//! future completes or is dropped (timeout / cancel). Calls on one isolated
//! client serialize on the client mutex, so the slot always holds exactly the
//! relay of the single in-flight call.

use std::collections::HashSet;
use std::sync::Arc;

use rmcp::model::{ElicitRequestParams, ElicitResult};
use rmcp::service::ElicitationMode;

/// Error returned when an upstream elicitation arrives with no active tool
/// call (no relay registered) — mirrors origin's elicitationService message.
pub const NO_ACTIVE_TOOL_CALL_MSG: &str = "Elicitation requires an active tool call";

/// Error returned when the downstream client did not declare the requested
/// elicitation mode — mirrors origin's elicitationService message.
pub const UNSUPPORTED_MODE_MSG: &str = "Client does not support this elicitation mode";

pub fn elicitation_error(message: impl Into<std::borrow::Cow<'static, str>>) -> rmcp::ErrorData {
    rmcp::ErrorData::new(rmcp::model::ErrorCode::INTERNAL_ERROR, message, None)
}

/// The elicitation mode of a request params value.
pub fn elicitation_mode_of(params: &ElicitRequestParams) -> ElicitationMode {
    match params {
        ElicitRequestParams::FormElicitationParams { .. } => ElicitationMode::Form,
        ElicitRequestParams::UrlElicitationParams { .. } => ElicitationMode::Url,
        // #[non_exhaustive] forward-compat: treat unknown future modes as form
        // (the mode gate in `forward` still applies).
        _ => ElicitationMode::Form,
    }
}

pub type ForwardFuture = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<ElicitResult, rmcp::ErrorData>> + Send>,
>;

/// Callback invoked for each upstream `elicitation/create` during the active
/// call. Built by the bridge around the downstream `Peer<RoleServer>`.
pub type ElicitForwardFn =
    Arc<dyn Fn(ElicitRequestParams) -> ForwardFuture + Send + Sync>;

/// Fire-and-forget forwarder for `notifications/elicitation/complete`
/// (URL mode) to the owning downstream session.
pub type ElicitNotifyFn = Arc<dyn Fn(Option<serde_json::Value>) + Send + Sync>;

/// Per-call relay: the downstream client's declared modes plus the forward
/// callback. Cloned into the connection's elicitation slot.
#[derive(Clone)]
pub struct ElicitationRelay {
    modes: Arc<HashSet<ElicitationMode>>,
    forward: ElicitForwardFn,
    notify_complete: Option<ElicitNotifyFn>,
}

impl ElicitationRelay {
    pub fn new(
        modes: HashSet<ElicitationMode>,
        forward: ElicitForwardFn,
        notify_complete: Option<ElicitNotifyFn>,
    ) -> Self {
        Self {
            modes: Arc::new(modes),
            forward,
            notify_complete,
        }
    }

    /// Forward an upstream elicitation to the downstream client, gated on the
    /// modes the downstream client declared at initialize.
    pub async fn forward(&self, params: ElicitRequestParams) -> Result<ElicitResult, rmcp::ErrorData> {
        if !self.modes.contains(&elicitation_mode_of(&params)) {
            return Err(elicitation_error(UNSUPPORTED_MODE_MSG));
        }
        (self.forward)(params).await
    }

    /// Forward a `notifications/elicitation/complete` notification (URL mode)
    /// to the owning downstream session, if a notify callback is installed.
    pub fn notify_complete(&self, params: Option<serde_json::Value>) {
        if let Some(notify) = &self.notify_complete {
            notify(params);
        }
    }

    /// The connection-lifetime notify callback, if this relay carries one.
    pub fn notify_fn(&self) -> Option<ElicitNotifyFn> {
        self.notify_complete.clone()
    }
}

/// Per-connection slot holding the relay of the currently active call.
/// Shared between the transport (owner) and the rmcp client handler (reader).
pub type ElicitationSlot = Arc<std::sync::Mutex<Option<ElicitationRelay>>>;

pub fn new_slot() -> ElicitationSlot {
    Arc::new(std::sync::Mutex::new(None))
}

/// Connection-lifetime slot for URL-mode `notifications/elicitation/complete`
/// forwarding. Unlike the per-call slot, it survives the call's completion —
/// origin installs its completion handler for the client's lifetime, and the
/// common URL-mode timing delivers the completion AFTER the tool call
/// returned. Set when a relay registers; cleared on transport disconnect.
pub type ElicitNotifySlot = Arc<std::sync::Mutex<Option<ElicitNotifyFn>>>;

pub fn new_notify_slot() -> ElicitNotifySlot {
    Arc::new(std::sync::Mutex::new(None))
}

/// Read the active relay out of a slot (handler side).
pub fn take_active(slot: &ElicitationSlot) -> Option<ElicitationRelay> {
    slot.lock().ok()?.clone()
}

/// Clear a slot when the owning call future completes or is dropped.
/// Synchronous on purpose: Drop runs during future cancellation, where no
/// async context is available.
pub struct SlotClearGuard {
    slot: ElicitationSlot,
}

impl SlotClearGuard {
    pub fn set(slot: &ElicitationSlot, relay: ElicitationRelay) -> Self {
        if let Ok(mut guard) = slot.lock() {
            *guard = Some(relay);
        }
        Self {
            slot: slot.clone(),
        }
    }
}

impl Drop for SlotClearGuard {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.slot.lock() {
            *guard = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;

    fn form_params() -> ElicitRequestParams {
        ElicitRequestParams::FormElicitationParams {
            meta: None,
            message: "pick one".to_string(),
            requested_schema: serde_json::from_value(serde_json::json!({
                "type": "object",
                "properties": { "choice": { "type": "string" } }
            }))
            .expect("valid elicitation schema"),
        }
    }

    fn url_params() -> ElicitRequestParams {
        ElicitRequestParams::UrlElicitationParams {
            meta: None,
            message: "authorize".to_string(),
            url: "https://example.com/authorize".to_string(),
            elicitation_id: "el-1".to_string(),
        }
    }

    fn relay_with(modes: &[ElicitationMode]) -> (ElicitationRelay, Arc<std::sync::Mutex<Vec<String>>>) {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_c = seen.clone();
        let relay = ElicitationRelay::new(
            modes.iter().copied().collect(),
            Arc::new(move |params: ElicitRequestParams| {
                let seen = seen_c.clone();
                Box::pin(async move {
                    seen.lock().unwrap().push(format!("{:?}", elicitation_mode_of(&params)));
                    Ok(ElicitResult::new(rmcp::model::ElicitationAction::Accept))
                }) as ForwardFuture
            }),
            None,
        );
        (relay, seen)
    }

    fn block_on<F: Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }

    #[test]
    fn mode_mapping() {
        assert!(matches!(elicitation_mode_of(&form_params()), ElicitationMode::Form));
        assert!(matches!(elicitation_mode_of(&url_params()), ElicitationMode::Url));
    }

    #[test]
    fn forward_gates_on_declared_modes() {
        let (relay, seen) = relay_with(&[ElicitationMode::Form]);
        let ok = block_on(relay.forward(form_params())).expect("form allowed");
        assert!(matches!(ok.action, rmcp::model::ElicitationAction::Accept));
        let err = block_on(relay.forward(url_params())).expect_err("url not declared");
        assert!(err.message.contains(UNSUPPORTED_MODE_MSG));
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn empty_modes_reject_everything() {
        let (relay, seen) = relay_with(&[]);
        let err = block_on(relay.forward(form_params())).expect_err("no modes declared");
        assert!(err.message.contains(UNSUPPORTED_MODE_MSG));
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn slot_guard_clears_on_drop() {
        let slot = new_slot();
        let (relay, _) = relay_with(&[ElicitationMode::Form]);
        {
            let _guard = SlotClearGuard::set(&slot, relay);
            assert!(take_active(&slot).is_some());
        }
        assert!(take_active(&slot).is_none());
    }
}
