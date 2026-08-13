//! MCP protocol version strategies.
//!
//! The hub must serve clients speaking any of several MCP protocol revisions
//! (2024-11-05 legacy HTTP+SSE, 2025-03-26 Streamable HTTP, 2025-06-18,
//! 2025-11-25). Each revision's differences live in a [`VersionStrategy`]
//! impl. `dispatch_mcp` resolves the strategy for a session once (during
//! `initialize`) and delegates version-specific shaping to it — there is no
//! per-request version if/else branching. Adding a future revision = write a
//! struct, implement the trait (overriding only what differs), and append it
//! to [`strategies`].
//!
//! All strategies are zero-sized statics stored as `&'static dyn` in the
//! registry; no allocation per lookup.

use serde_json::{json, Value};

/// Transport a revision uses to deliver responses to the downstream client.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TransportMode {
    /// 2025-03-26+: single POST endpoint, JSON (or SSE-upgraded) responses.
    StreamableHttp,
    /// 2024-11-05: GET opens an SSE stream that carries all server→client
    /// messages; client POSTs requests to a separate endpoint.
    LegacySse,
}

/// Outcome of a version-specific method handler. `dispatch_mcp` converts this
/// to an axum `Response` (so this module stays free of axum/HTTP types).
pub enum MethodOutcome {
    /// Success — becomes a JSON-RPC result with this value.
    Result(Value),
    /// Error — becomes a JSON-RPC error with this code/message.
    Error(i32, String),
}

/// Shared context handed to a strategy's [`VersionStrategy::handle_extra_method`]
/// for methods the common dispatcher does not know (e.g. 2025-11 `tasks/*`).
/// Carries the per-request state a future strategy might need.
pub struct MethodCtx {
    pub scope: String,
    pub session_id: Option<String>,
    pub params: Value,
    pub name_sep: String,
    pub client_ip: String,
}

/// One MCP protocol revision. Default impls describe 2025-03-26 behavior;
/// older/newer revisions override the hooks that differ.
pub trait VersionStrategy: Send + Sync {
    /// Protocol version string this strategy serves, e.g. `"2025-03-26"`.
    fn version(&self) -> &'static str;

    /// How this revision delivers responses to the downstream client.
    fn transport(&self) -> TransportMode {
        TransportMode::StreamableHttp
    }

    /// Whether subsequent requests MUST carry `MCP-Protocol-Version` (2025-06+).
    fn requires_version_header(&self) -> bool {
        false
    }

    /// Capabilities advertised in the `initialize` result.
    fn capabilities(&self) -> Value {
        json!({"tools": {}, "prompts": {}, "resources": {}})
    }

    /// Shape a tool entry for `tools/list`. Default = passthrough (keep 2025
    /// fields like `annotations`/`outputSchema`). 2024-11-05 strips them.
    fn shape_tool(&self, tool: Value) -> Value {
        tool
    }

    /// Shape a `tools/call` result. Default = passthrough (keep
    /// `structuredContent`). 2024-11-05 strips it.
    fn shape_tool_call_result(&self, result: Value) -> Value {
        result
    }

    /// Handle a method the common dispatcher does not recognise (e.g. 2025-11
    /// `tasks/*`). Return `Some(outcome)` if handled, `None` to fall through
    /// to the standard `-32601 Method not found`.
    fn handle_extra_method(&self, _method: &str, _ctx: &MethodCtx) -> Option<MethodOutcome> {
        None
    }
}

// ---------------------------------------------------------------------------
// Built-in strategies
// ---------------------------------------------------------------------------

/// 2024-11-05 — first stable revision. Legacy HTTP+SSE transport; no tool
/// annotations / structuredContent / outputSchema (those were added later).
struct V2024_11_05;
static V2024: V2024_11_05 = V2024_11_05;

impl VersionStrategy for V2024_11_05 {
    fn version(&self) -> &'static str {
        "2024-11-05"
    }
    fn transport(&self) -> TransportMode {
        TransportMode::LegacySse
    }
    fn capabilities(&self) -> Value {
        json!({"tools": {}, "prompts": {}, "resources": {}})
    }
    fn shape_tool(&self, mut tool: Value) -> Value {
        if let Some(obj) = tool.as_object_mut() {
            obj.remove("annotations");
            obj.remove("outputSchema");
        }
        tool
    }
    fn shape_tool_call_result(&self, mut result: Value) -> Value {
        if let Some(obj) = result.as_object_mut() {
            obj.remove("structuredContent");
        }
        result
    }
}

/// 2025-03-26 — Streamable HTTP, tool annotations, structured output, audio.
/// This is the hub's baseline behavior; all defaults already describe it.
struct V2025_03_26;
static V2025_03: V2025_03_26 = V2025_03_26;
impl VersionStrategy for V2025_03_26 {
    fn version(&self) -> &'static str {
        "2025-03-26"
    }
}

/// 2025-06-18 — same surface as 2025-03-26 plus: subsequent requests MUST carry
/// `MCP-Protocol-Version` (enforced in dispatch_mcp), structuredContent in
/// tool results, `_meta` on more types, elicitation (not advertised — the hub
/// is a proxy that never elicits). JSON-RPC batching removed (the hub never
/// accepted batches anyway).
struct V2025_06_18;
static V2025_06: V2025_06_18 = V2025_06_18;
impl VersionStrategy for V2025_06_18 {
    fn version(&self) -> &'static str {
        "2025-06-18"
    }
    fn requires_version_header(&self) -> bool {
        true
    }
}

/// 2025-11-25 — experimental Tasks (`tasks/get|result|list|cancel`), icons,
/// OIDC discovery, titled enums. Tasks are implemented receiver-side by the
/// hub (see `services/mcp_tasks`): a client augments `tools/call` with a
/// `task` field and polls via `tasks/*`. The hub declares the `tasks`
/// capability for `tools/call` augmentation + `list`/`cancel`.
struct V2025_11_25;
static V2025_11: V2025_11_25 = V2025_11_25;
impl VersionStrategy for V2025_11_25 {
    fn version(&self) -> &'static str {
        "2025-11-25"
    }
    fn requires_version_header(&self) -> bool {
        true
    }
    fn capabilities(&self) -> Value {
        json!({
            "tools": {},
            "prompts": {},
            "resources": {},
            "tasks": {
                "list": {},
                "cancel": {},
                "requests": {"tools": {"call": {}}}
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

static STRATEGIES: std::sync::OnceLock<Vec<&'static dyn VersionStrategy>> =
    std::sync::OnceLock::new();

/// All known strategies, in registration order. Append here to add a revision.
fn strategies() -> &'static Vec<&'static dyn VersionStrategy> {
    STRATEGIES.get_or_init(|| vec![&V2024, &V2025_03, &V2025_06, &V2025_11])
}

/// The default strategy when a client requests an unknown/missing version
/// (spec: server responds with the version it will use). 2025-03-26 is the
/// baseline.
pub fn default_strategy() -> &'static dyn VersionStrategy {
    &V2025_03
}

/// Resolve a strategy by requested protocol version. Falls back to
/// [`default_strategy`] when the requested version is unknown.
pub fn strategy_for(requested: &str) -> &'static dyn VersionStrategy {
    strategies()
        .iter()
        .copied()
        .find(|s| s.version() == requested)
        .unwrap_or_else(default_strategy)
}

/// All versions this hub advertises as supported (for diagnostics/logging).
pub fn supported_versions() -> Vec<&'static str> {
    strategies().iter().map(|s| s.version()).collect()
}

/// Whether a given protocol version string is one this hub serves (used by
/// the `MCP-Protocol-Version` header check in 2025-06+ sessions).
pub fn is_supported(version: &str) -> bool {
    strategies().iter().any(|s| s.version() == version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiation_picks_exact_or_defaults() {
        assert_eq!(strategy_for("2024-11-05").version(), "2024-11-05");
        assert_eq!(strategy_for("2025-03-26").version(), "2025-03-26");
        assert_eq!(strategy_for("2025-06-18").version(), "2025-06-18");
        assert_eq!(strategy_for("2025-11-25").version(), "2025-11-25");
        // unknown → default 2025-03-26
        assert_eq!(strategy_for("2099-01-01").version(), "2025-03-26");
        assert_eq!(strategy_for("").version(), "2025-03-26");
    }

    #[test]
    fn version_header_required_only_after_2025_03() {
        assert!(!strategy_for("2024-11-05").requires_version_header());
        assert!(!strategy_for("2025-03-26").requires_version_header());
        assert!(strategy_for("2025-06-18").requires_version_header());
        assert!(strategy_for("2025-11-25").requires_version_header());
    }

    #[test]
    fn v2025_11_advertises_tasks_capability() {
        let s = strategy_for("2025-11-25");
        let caps = s.capabilities();
        // tasks capability with tools/call augmentation + list + cancel
        assert!(caps.get("tasks").is_some());
        assert!(caps["tasks"].get("list").is_some());
        assert!(caps["tasks"].get("cancel").is_some());
        assert!(caps["tasks"]["requests"]["tools"]["call"].is_object());
        // handle_extra_method no longer claims tasks/* (real impl in dispatch_mcp)
        let ctx = MethodCtx {
            scope: String::new(),
            session_id: None,
            params: Value::Null,
            name_sep: "-".to_string(),
            client_ip: "127.0.0.1".to_string(),
        };
        assert!(s.handle_extra_method("tasks/get", &ctx).is_none());
        assert!(s.handle_extra_method("somefuturemethod", &ctx).is_none());
    }

    #[test]
    fn is_supported_checks_registry() {
        assert!(is_supported("2024-11-05"));
        assert!(is_supported("2025-03-26"));
        assert!(is_supported("2025-06-18"));
        assert!(is_supported("2025-11-25"));
        assert!(!is_supported("2099-01-01"));
    }

    #[test]
    fn v2024_strips_2025_fields() {
        let s = strategy_for("2024-11-05");
        let tool = json!({"name":"t","annotations":{"readOnlyHint":true},"outputSchema":{"type":"object"}});
        let shaped = s.shape_tool(tool);
        assert!(shaped.get("annotations").is_none());
        assert!(shaped.get("outputSchema").is_none());
        assert_eq!(shaped["name"], "t");
    }

    #[test]
    fn v2025_keeps_fields() {
        let s = strategy_for("2025-03-26");
        let tool = json!({"name":"t","annotations":{"readOnlyHint":true}});
        let shaped = s.shape_tool(tool);
        assert!(shaped.get("annotations").is_some());
    }

    #[test]
    fn transport_mode_differs() {
        assert_eq!(strategy_for("2024-11-05").transport(), TransportMode::LegacySse);
        assert_eq!(strategy_for("2025-03-26").transport(), TransportMode::StreamableHttp);
    }
}
