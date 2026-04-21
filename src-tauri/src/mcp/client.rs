use crate::models::server::{Tool, ToolCallResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

/// Common interface for any MCP transport backend
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Initialize connection and perform MCP handshake
    async fn connect(&mut self) -> Result<()>;
    /// Gracefully disconnect
    async fn disconnect(&mut self) -> Result<()>;
    /// Returns true if the transport is connected and healthy
    fn is_connected(&self) -> bool;
    /// Retrieve the list of tools from the remote server
    async fn list_tools(&self) -> Result<Vec<Tool>>;
    /// Invoke a tool by name with the given arguments
    async fn call_tool(&self, name: &str, arguments: Value) -> Result<ToolCallResult>;
}

/// Thin wrapper that holds a boxed transport and the server name
pub struct McpClient {
    pub server_name: String,
    transport: Box<dyn McpTransport>,
}

impl McpClient {
    pub fn new(server_name: impl Into<String>, transport: Box<dyn McpTransport>) -> Self {
        Self {
            server_name: server_name.into(),
            transport,
        }
    }

    pub async fn connect(&mut self) -> Result<()> {
        self.transport.connect().await
    }

    pub async fn disconnect(&mut self) -> Result<()> {
        self.transport.disconnect().await
    }

    pub fn is_connected(&self) -> bool {
        self.transport.is_connected()
    }

    pub async fn list_tools(&self) -> Result<Vec<Tool>> {
        self.transport.list_tools().await
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<ToolCallResult> {
        self.transport.call_tool(name, arguments).await
    }
}
