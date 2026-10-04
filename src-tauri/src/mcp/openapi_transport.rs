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
            log::info!("[{}] Using inline OpenAPI schema", self.server_name);
            return Ok(schema.clone());
        }

        if let Some(ref url) = self.config.spec_url {
            // Fetch spec from URL (bounded: 30s total + 32MB body cap).
            log::info!("[{}] Fetching OpenAPI spec from URL: {}", self.server_name, url);
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new());
            let mut req = client.get(url);

            // Add any configured headers (values redacted: they routinely
            // carry Authorization/apiKey and app_logger persists info/debug
            // lines into the log database)
            for (k, v) in &self.config.headers {
                // Validate name/value: RequestBuilder::header PANICS on invalid
                // HeaderName/HeaderValue (non-ASCII, control chars) — a bad
                // user config would crash the connect path. Same guard as
                // build_default_headers.
                let name = reqwest::header::HeaderName::try_from(k.as_str());
                let value = reqwest::header::HeaderValue::try_from(v.as_str());
                let (name, value) = match (name, value) {
                    (Ok(n), Ok(val)) => (n, val),
                    (Err(e), _) => {
                        log::warn!("[{}] Skipping invalid spec-fetch header name '{}': {}", self.server_name, k, e);
                        continue;
                    }
                    (_, Err(e)) => {
                        log::warn!("[{}] Skipping invalid spec-fetch header '{}': {}", self.server_name, k, e);
                        continue;
                    }
                };
                log::debug!("[{}] Adding header: {} (len={})", self.server_name, k, v.len());
                req = req.header(name, value);
            }

            let resp = req.send().await.map_err(|e| {
                log::error!("[{}] Failed to fetch OpenAPI spec: {}", self.server_name, e);
                e
            })?;

            log::info!("[{}] OpenAPI spec response: status={}", self.server_name, resp.status());

            if !resp.status().is_success() {
                return Err(anyhow!(
                    "Failed to fetch OpenAPI spec from {}: HTTP {}",
                    url,
                    resp.status()
                ));
            }

            // Cap the download size: specs come from user-configured URLs —
            // an unbounded body read is a memory-exhaustion hazard.
            const MAX_SPEC_BYTES: usize = 32 * 1024 * 1024;
            if let Some(len) = resp.content_length() {
                if len as usize > MAX_SPEC_BYTES {
                    return Err(anyhow!(
                        "OpenAPI spec from {} exceeds the {}MB size limit ({} bytes)",
                        url,
                        MAX_SPEC_BYTES / (1024 * 1024),
                        len
                    ));
                }
            }
            // Stream with a hard cap: `bytes()` buffers the WHOLE body before
            // the length check runs — a chunked (no content-length) malicious
            // server could allocate gigabytes inside the 30s timeout window.
            let mut body: Vec<u8> = Vec::new();
            let mut stream = resp;
            while let Some(chunk) = stream.chunk().await? {
                if body.len() + chunk.len() > MAX_SPEC_BYTES {
                    return Err(anyhow!(
                        "OpenAPI spec from {} exceeds the {}MB size limit (>{} bytes streamed)",
                        url,
                        MAX_SPEC_BYTES / (1024 * 1024),
                        body.len() + chunk.len()
                    ));
                }
                body.extend_from_slice(&chunk);
            }
            let spec: Value = serde_json::from_slice(&body)?;
            log::info!("[{}] OpenAPI spec fetched successfully", self.server_name);
            return Ok(spec);
        }

        Err(anyhow!(
            "OpenAPI server '{}' has no spec_url or spec_schema configured",
            self.server_name
        ))
    }

    /// Build the outbound default headers: user headers + passthrough headers
    /// + configured security credentials.
    ///
    /// rmcp-openapi and the desktop now share reqwest 0.13 (unified during the
    /// rmcp migration), so `HeaderMap` types are compatible and the credentials
    /// can be applied for real — previously this was impossible (version
    /// mismatch) and `config.security` was dead config (logged, never used).
    /// Query/cookie-placed apiKey cannot ride default headers; those fall back
    /// to whatever the spec's security schemes define.
    fn build_default_headers(&self) -> anyhow::Result<reqwest::header::HeaderMap> {
        let mut map = reqwest::header::HeaderMap::new();
        let mut insert = |name: &str, value: &str| {
            if let (Ok(n), Ok(v)) = (
                reqwest::header::HeaderName::try_from(name),
                reqwest::header::HeaderValue::from_str(value),
            ) {
                map.insert(n, v);
            } else {
                log::warn!("[{}] skipping invalid header '{}' (value encoding)", self.server_name, name);
            }
        };
        for (k, v) in &self.config.headers {
            insert(k, v);
        }
        for (k, v) in &self.config.passthrough_headers {
            insert(k, v);
        }
        if let Some(sec) = &self.config.security {
            // Malformed config fallback (pool.rs): a security variant with all
            // empty fields means "no usable credentials" — skip instead of
            // emitting empty header values that would mask spec-level schemes.
            let malformed = match sec {
                OpenApiSecurity::ApiKey { name, value, .. } => name.is_empty() && value.is_empty(),
                OpenApiSecurity::Http { credentials, .. } => credentials.is_empty(),
                OpenApiSecurity::OAuth2 { token } => token.is_empty(),
                OpenApiSecurity::OpenIdConnect { token, .. } => token.is_empty(),
            };
            if malformed {
                log::warn!(
                    "[{}] security config malformed (missing payload); skipping auth headers, relying on spec security schemes",
                    self.server_name
                );
            } else {
                match sec {
                    OpenApiSecurity::ApiKey { name, location, value } => {
                        if location.eq_ignore_ascii_case("header") {
                            insert(name, value);
                        } else {
                            // rmcp-openapi 0.32 NEVER applies spec securitySchemes
                            // to outgoing requests (verified in vendored source:
                            // only default_headers + Authorization passthrough
                            // inject credentials). The old "relying on spec
                            // security schemes" log pointed users at a fallback
                            // that does not exist — every call went out
                            // unauthenticated and failed 401 downstream. Fail
                            // loudly at connect instead of silently unauthenticated.
                            return Err(anyhow!(
                                "server '{}': apiKey security scheme with location '{}' is not supported (only 'header'); configure the credential as a header or add it to the tool's parameters explicitly",
                                self.server_name, location
                            ));
                        }
                    }
                    OpenApiSecurity::Http { scheme, credentials } => {
                        let v = if scheme.eq_ignore_ascii_case("basic") {
                            format!("Basic {}", credentials)
                        } else {
                            format!("Bearer {}", credentials)
                        };
                        insert("Authorization", &v);
                    }
                    OpenApiSecurity::OAuth2 { token } => {
                        insert("Authorization", &format!("Bearer {}", token));
                    }
                    OpenApiSecurity::OpenIdConnect { token, .. } => {
                        insert("Authorization", &format!("Bearer {}", token));
                    }
                }
            }
        }
        Ok(map)
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
                        // OpenAPI server URLs may contain {variable} templates that must be
                        // substituted with the variable's `default` before use (e.g. a spec
                        // declares `url: '{server}/api/v1'` with `variables.server.default`).
                        // Without substitution the literal '{server}/api/v1' is misclassified
                        // as a relative path and glued onto the spec source host, 404-ing
                        // every tool call. Mirrors upstream fix (#959/#960).
                        let mut resolved_url = url.to_string();
                        if let Some(variables) = first.get("variables").and_then(|v| v.as_object()) {
                            for (name, variable) in variables {
                                if let Some(default) = variable.get("default").and_then(|v| v.as_str()) {
                                    resolved_url = resolved_url.replace(&format!("{{{}}}", name), default);
                                }
                            }
                        }

                        // Handle relative URLs (e.g. "/api/v1")
                        if resolved_url.starts_with('/') {
                            return Url::parse("http://localhost").ok()
                                .and_then(|mut u| { u.set_path(&resolved_url); Some(u) });
                        }
                        if let Ok(parsed) = Url::parse(&resolved_url) {
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
        log::info!(
            "[{}] OpenAPI config: version={}, headers={}, security={}",
            self.server_name,
            self.config.version,
            format!("{} header(s) [values redacted]", self.config.headers.len()),
            self.config.security.as_ref().map_or("none".to_string(), |s| match s {
                OpenApiSecurity::ApiKey { .. } => "apiKey",
                OpenApiSecurity::Http { .. } => "http",
                OpenApiSecurity::OAuth2 { .. } => "oauth2",
                OpenApiSecurity::OpenIdConnect { .. } => "openIdConnect",
            }
            .to_string())
        );

        // 1. Fetch the spec
        let spec_value = self.fetch_spec().await?;

        // 2. Extract base URL
        let base_url = Self::extract_base_url(&spec_value)
            .ok_or_else(|| anyhow!(
                "OpenAPI spec for '{}' has no base URL. Add one of:\n  - servers: [{{url: \"https://api.example.com\"}}]\n  - host + basePath (Swagger 2.0)\n  - x-base-url extension",
                self.server_name
            ))?;
        log::info!("[{}] OpenAPI base_url: {}", self.server_name, base_url);

        // 3. Create the rmcp-openapi server with real outbound credentials:
        // reqwest 0.13 is shared with rmcp-openapi, so configured security
        // (apiKey-in-header / bearer / basic / oauth2 / oidc) + user headers
        // apply to every API call. Previously default_headers was forced to
        // None (version-mismatch workaround) and `config.security` was dead.
        let default_headers = self.build_default_headers()?;
        log::info!(
            "[{}] outbound default headers: {} entrie(s), security applied: {}",
            self.server_name,
            default_headers.len(),
            self.config.security.is_some()
        );

        let mut server = OpenApiServer::new(
            spec_value,
            base_url.clone(),
            Some(default_headers),
            None, // filters
            false, // skip_tool_descriptions
            false, // skip_parameter_descriptions
            false, // insecure
        );

        // 5. Load the spec and generate tools — spec snapshot/restore is
        // CPU-bound ($ref lattice can explode); keep it off the async worker.
        let load = tokio::task::spawn_blocking(move || {
            server.load_openapi_spec().map(|_| server)
        })
        .await
        .map_err(|e| anyhow!("OpenAPI spec load task panicked for '{}': {}", self.server_name, e))?
        .map_err(|e| anyhow!("Failed to load OpenAPI spec for '{}': {}", self.server_name, e))?;
        let server = load;

        let tool_count = server.tool_count();
        log::info!(
            "[{}] OpenAPI transport connected ({} tools, base_url={})",
            self.server_name,
            tool_count,
            base_url
        );

        // Log tool names for debugging
        let mcp_tools = server.tool_collection.to_mcp_tools();
        for t in &mcp_tools {
            log::info!("[{}] OpenAPI tool: {}", self.server_name, t.name);
        }

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
                    enabled: true,
                    // OpenAPI tools are synthesized, not from a real tools/list,
                    // so they carry no MCP annotations / outputSchema.
                    annotations: None,
                    output_schema: None,
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

        // Log to database so it shows in the app's log viewer.
        // Arguments routinely carry secrets (API keys/tokens as tool params)
        // and app_logger PERSISTS info lines to the log DB — log key names +
        // a size summary instead of the full JSON (same reason header values
        // are redacted in this file).
        let arg_keys: Vec<&str> = arguments.as_object().map(|m| m.keys().map(String::as_str).collect()).unwrap_or_default();
        let start_msg = format!(
            "[{}] OpenAPI call_tool: name={}, base_url={:?}, arg_keys={:?}, arg_bytes={}",
            self.server_name,
            name,
            self.base_url,
            arg_keys,
            arguments.to_string().len()
        );
        log::info!("{}", start_msg);
        app_logger::log_to_db("info", &start_msg);

        // Execute the tool call with no authorization (auth is handled by headers)
        let result = tool.call(&arguments, Authorization::None, None).await
            .map_err(|e| {
                let err_msg = format!(
                    "[{}] OpenAPI call_tool FAILED: name={}, error={:#}",
                    self.server_name, name, e
                );
                log::warn!("{}", err_msg);
                app_logger::log_to_db("warn", &err_msg);
                anyhow!("Tool '{}' call failed: {:#}", name, e)
            })?;

        // Convert rmcp CallToolResult to our ToolCallResult
        let content: Vec<Value> = result.content
            .into_iter()
            .map(|c| serde_json::to_value(c).unwrap_or(json!(null)))
            .collect();

        let is_error = result.is_error.unwrap_or(false);

        // Log the full response including content for debugging
        let content_summary = if content.len() <= 3 {
            format!("{:?}", content)
        } else {
            format!("{:?}... ({} items)", &content[..3], content.len())
        };
        let ok_msg = format!(
            "[{}] OpenAPI call_tool OK: name={}, is_error={}, content={}",
            self.server_name, name, is_error, content_summary
        );
        if is_error {
            log::warn!("{}", ok_msg);
            app_logger::log_to_db("warn", &ok_msg);
        } else {
            log::info!("{}", ok_msg);
        }

        Ok(ToolCallResult { content, is_error, structured_content: None, raw_meta: None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_base_url_substitutes_server_variables() {
        // Mirrors upstream #959: a server URL with a {variable} template must be
        // resolved against the variable's `default` before parsing.
        let spec = json!({
            "servers": [{
                "url": "{server}/api/v1",
                "variables": {
                    "server": { "default": "https://api.example.com" }
                }
            }]
        });
        let url = OpenapiTransport::extract_base_url(&spec).expect("should resolve");
        assert_eq!(url.as_str(), "https://api.example.com/api/v1");
    }

    #[test]
    fn extract_base_url_handles_plain_server_url() {
        let spec = json!({
            "servers": [{ "url": "https://api.example.com/v2" }]
        });
        let url = OpenapiTransport::extract_base_url(&spec).expect("should resolve");
        assert_eq!(url.as_str(), "https://api.example.com/v2");
    }

    #[test]
    fn extract_base_url_handles_relative_server_url() {
        let spec = json!({
            "servers": [{ "url": "/api/v1" }]
        });
        let url = OpenapiTransport::extract_base_url(&spec).expect("should resolve");
        assert_eq!(url.path(), "/api/v1");
    }

    #[test]
    fn extract_base_url_falls_back_to_swagger_2_host() {
        let spec = json!({
            "host": "api.example.com",
            "basePath": "/v3",
            "schemes": ["https"]
        });
        let url = OpenapiTransport::extract_base_url(&spec).expect("should resolve");
        assert_eq!(url.as_str(), "https://api.example.com/v3");
    }
}
