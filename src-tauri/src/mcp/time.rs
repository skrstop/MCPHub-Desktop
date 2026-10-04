//! Shared call-timeout helpers for upstream MCP tool invocations.
//!
//! Only the SSE transport carries a per-request timeout internally; stdio,
//! rmcp-http and openapi transports rely on this wrapper so a hung upstream
//! process/connection cannot park a session handler (or a SEP-2663 task)
//! forever. The budget is generous (10 minutes) — it only exists to catch
//! truly dead transports, not to cap legitimately slow tools.

use std::time::Duration;
use tokio::time::timeout;

/// Upstream tool-call budget: 10 minutes.
pub const TOOL_CALL_TIMEOUT: Duration = Duration::from_secs(600);

/// Run a tool-call future under [`TOOL_CALL_TIMEOUT`].
pub async fn timeout_tool_call<F, T>(fut: F) -> anyhow::Result<T>
where
    F: std::future::Future<Output = anyhow::Result<T>>,
{
    match timeout(TOOL_CALL_TIMEOUT, fut).await {
        Ok(r) => r,
        Err(_) => Err(anyhow::anyhow!(
            "upstream tool call timed out after {}s",
            TOOL_CALL_TIMEOUT.as_secs()
        )),
    }
}
