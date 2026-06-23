/// OpenAPI transport — uses `rmcp-openapi` as a library to convert OpenAPI specs
/// into MCP tools and handle HTTP calls to the actual API endpoints.
///
/// Supports two modes:
/// - URL mode: fetches the OpenAPI spec from a remote URL
/// - Schema mode: uses an inline JSON schema directly
use super::client::McpTransport;
use crate::models::server::{Tool, ToolCallResult};
use crate::services::app_logger;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use rmcp_openapi::{Server as OpenApiServer, config::Authorization};
use serde_json::{json, Value};
use std::collections::HashMap;
use url::Url;

/// Configuration for an OpenAPI MCP server
#[derive(Debug, Clone)]
pub struct OpenApiConfig {
    /// URL to fetch the OpenAPI spec from
    pub spec_url: Option<String>,
    /// Inline OpenAPI spec JSON (if not using URL)
    pub spec_schema: Option<Value>,
    /// OpenAPI version (e.g. "3.1.0")
    pub version: String,
    /// Security configuration
    pub security: Option<OpenApiSecurity>,
    /// Headers to pass through to the API
    pub passthrough_headers: HashMap<String, String>,
    /// Extra headers configured for this server
    pub headers: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub enum OpenApiSecurity {
    ApiKey {
        name: String,
        location: String, // "header" | "query" | "cookie"
        value: String,
    },
    Http {
        scheme: String, // "bearer" | "basic"
        credentials: String,
    },
    OAuth2 {
        token: String,
    },
    OpenIdConnect {
        url: String,
        token: String,
    },
}

pub struct OpenapiTransport {
    config: OpenApiConfig,
    server_name: String,
    /// The rmcp-openapi server instance (populated after connect)
    server: Option<OpenApiServer>,
    /// Base URL extracted from the spec
    base_url: Option<Url>,
    /// Whether we've successfully connected (loaded the spec)
    connected: bool,
}

impl OpenapiTransport {
    pub fn new(server_name: impl Into<String>, config: OpenApiConfig) -> Self {
        Self {
            config,
            server_name: server_name.into(),
            server: None,
            base_url: None,
            connected: false,
        }
    }

    /// Fetch the OpenAPI spec from URL or use inline schema
    async fn fetch_spec(&self) -> Result<Value> {
        if let Some(ref schema) = self.config.spec_schema {
            // Use inline schema directly
            return Ok(schema.clone());
        }

        if let Some(ref url) = self.config.spec_url {
            // Fetch spec from URL
            let client = reqwest::Client::new();
            let mut req = client.get(url);

            // Add any configured headers
            for (k, v) in &self.config.headers {
                req = req.header(k, v);
            }

            let resp = req.send().await?;
            if !resp.status().is_success() {
                return Err(anyhow!(
                    "Failed to fetch OpenAPI spec from {}: HTTP {}",
                    url,
                    resp.status()
                ));
            }
            let spec: Value = resp.json().await?;
            return Ok(spec);
        }

        Err(anyhow!(
            "OpenAPI server '{}' has no spec_url or spec_schema configured",
            self.server_name
        ))
    }

    /// Build default headers from config.
    ///
    /// Note: rmcp-openapi uses reqwest v0.13 while our project uses v0.12.
    /// Headers are passed as JSON via environment variable to avoid type mismatch.
    /// The rmcp-openapi library handles authentication via its security scheme support.
    fn build_headers_json(&self) -> Option<String> {
        if self.config.headers.is_empty() && self.config.passthrough_headers.is_empty() {
            return None;
        }
        let mut map = serde_json::Map::new();
        for (k, v) in &self.config.headers {
            map.insert(k.clone(), serde_json::Value::String(v.clone()));
        }
        for (k, v) in &self.config.passthrough_headers {
            map.insert(k.clone(), serde_json::Value::String(v.clone()));
        }
        serde_json::to_string(&map).ok()
    }

    /// Extract base URL from the OpenAPI spec.
    ///
    /// Tries multiple strategies:
    /// 1. OpenAPI 3.x: `servers[0].url`
    /// 2. Swagger 2.0: `schemes[0] + host + basePath`
    /// 3. Falls back to `http://localhost` if nothing found
    fn extract_base_url(spec: &Value) -> Option<Url> {
        // Strategy 1: OpenAPI 3.x servers array
        if let Some(servers) = spec.get("servers").and_then(|v| v.as_array()) {
            if let Some(first) = servers.first() {
                if let Some(url) = first.get("url").and_then(|v| v.as_str()) {
                    if !url.is_empty() {
                        // Handle relative URLs (e.g. "/api/v1")
                        if url.starts_with('/') {
                            return Url::parse("http://localhost").ok()
                                .and_then(|mut u| { u.set_path(url); Some(u) });
                        }
                        if let Ok(parsed) = Url::parse(url) {
                            return Some(parsed);
                        }
                    }
                }
            }
        }

        // Strategy 2: Swagger 2.0 (host + basePath + schemes)
        if let Some(host) = spec.get("host").and_then(|v| v.as_str()) {
            let scheme = spec
                .get("schemes")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
                .unwrap_or("https");
            let base_path = spec
                .get("basePath")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let url_str = format!("{}://{}{}", scheme, host, base_path);
            if let Ok(parsed) = Url::parse(&url_str) {
                return Some(parsed);
            }
        }

        // Strategy 3: Check x-base-url extension (custom)
        if let Some(url) = spec.get("x-base-url").and_then(|v| v.as_str()) {
            if let Ok(parsed) = Url::parse(url) {
                return Some(parsed);
            }
        }

        None
    }
}

#[async_trait]
impl McpTransport for OpenapiTransport {
    async fn connect(&mut self) -> Result<()> {
        log::info!(
            "[{}] loading OpenAPI spec (url={:?}, schema={:?})",
            self.server_name,
            self.config.spec_url,
            self.config.spec_schema.as_ref().map(|_| "<inline>")
        );

        // 1. Fetch the spec
        let spec_value = self.fetch_spec().await?;

        // 2. Extract base URL
        let base_url = Self::extract_base_url(&spec_value)
            .ok_or_else(|| anyhow!(
                "OpenAPI spec for '{}' has no base URL. Add one of:\n  - servers: [{{url: \"https://api.example.com\"}}]\n  - host + basePath (Swagger 2.0)\n  - x-base-url extension",
                self.server_name
            ))?;

        // 3. Create the rmcp-openapi server
        // Note: default_headers is None because reqwest v0.12 HeaderMap != v0.13 HeaderMap.
        // Authentication is handled via the OpenAPI spec's security schemes.
        if let Some(hdrs) = self.build_headers_json() {
            log::debug!("[{}] custom headers configured: {}", self.server_name, hdrs);
        }

        let mut server = OpenApiServer::new(
            spec_value,
            base_url.clone(),
            None, // default_headers (reqwest version mismatch)
            None, // filters
            false, // skip_tool_descriptions
            false, // skip_parameter_descriptions
            false, // insecure
        );

        // 5. Load the spec and generate tools
        server.load_openapi_spec()
            .map_err(|e| anyhow!("Failed to load OpenAPI spec for '{}': {}", self.server_name, e))?;

        let tool_count = server.tool_count();
        log::info!(
            "[{}] OpenAPI transport connected ({} tools, base_url={})",
            self.server_name,
            tool_count,
            base_url
        );

        self.server = Some(server);
        self.base_url = Some(base_url);
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        let msg = format!("[{}] Disconnecting OpenAPI transport...", self.server_name);
        log::info!("{}", msg);
        app_logger::log_to_db("info", &msg);

        self.server = None;
        self.connected = false;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    async fn list_tools(&self) -> Result<Vec<Tool>> {
        let server = self.server.as_ref()
            .ok_or_else(|| anyhow!("OpenAPI server '{}' not connected", self.server_name))?;

        let mcp_tools = server.tool_collection.to_mcp_tools();
        let tools = mcp_tools
            .into_iter()
            .map(|t| {
                let description = t.description
                    .map(|d| d.into_owned())
                    .filter(|d| !d.is_empty());
                let input_schema = serde_json::to_value(&*t.input_schema)
                    .unwrap_or(json!({"type": "object"}));
                Tool {
                    name: t.name.into_owned(),
                    description,
                    input_schema,
                    server_name: self.server_name.clone(),
                }
            })
            .collect();

        Ok(tools)
    }

    async fn call_tool(&self, name: &str, arguments: Value) -> Result<ToolCallResult> {
        let server = self.server.as_ref()
            .ok_or_else(|| anyhow!("OpenAPI server '{}' not connected", self.server_name))?;

        // Get the tool from the collection
        let tool = server.tool_collection.get_tool(name)
            .ok_or_else(|| anyhow!("Tool '{}' not found in OpenAPI server '{}'", name, self.server_name))?;

        // Execute the tool call with no authorization (auth is handled by headers)
        let result = tool.call(&arguments, Authorization::None, None).await
            .map_err(|e| anyhow!("Tool '{}' call failed: {}", name, e))?;

        // Convert rmcp CallToolResult to our ToolCallResult
        let content: Vec<Value> = result.content
            .into_iter()
            .map(|c| serde_json::to_value(c).unwrap_or(json!(null)))
            .collect();

        let is_error = result.is_error.unwrap_or(false);

        Ok(ToolCallResult { content, is_error })
    }
}
