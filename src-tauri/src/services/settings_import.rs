/// Import mcp_settings.json from the original MCPHub format into SQLite.
use crate::{
    models::{
        server::{ServerConfig, ServerType},
        user::{UserPayload, UserRole},
    },
    services::{server_service, user_service},
};
use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpSettings {
    mcp_servers: Option<HashMap<String, RawServerConfig>>,
    users: Option<Vec<RawUser>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawServerConfig {
    #[serde(rename = "type")]
    server_type: Option<String>,
    #[serde(rename = "serverType")]
    server_type_alt: Option<String>,
    command: Option<String>,
    args: Option<Vec<String>>,
    env: Option<HashMap<String, String>>,
    url: Option<String>,
    headers: Option<HashMap<String, String>>,
    description: Option<String>,
    disabled: Option<bool>,
    options: Option<serde_json::Value>,
    openapi: Option<serde_json::Value>,
    proxy: Option<serde_json::Value>,
    #[serde(alias = "per_session_client")]
    per_session_client: Option<bool>,
    #[serde(rename = "startOnDemand", alias = "start_on_demand")]
    start_on_demand: Option<bool>,
    #[serde(rename = "idleTimeoutMs", alias = "idle_timeout_ms")]
    idle_timeout_ms: Option<u64>,
    #[serde(rename = "enableKeepAlive", alias = "enable_keep_alive")]
    enable_keep_alive: Option<bool>,
    #[serde(rename = "keepAliveInterval", alias = "keep_alive_interval")]
    keep_alive_interval: Option<u64>,
    #[serde(rename = "passthroughHeaders", alias = "passthrough_headers")]
    passthrough_headers: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct RawUser {
    username: String,
    password: Option<String>,
    #[serde(rename = "passwordHash")]
    #[allow(dead_code)]
    password_hash: Option<String>,
    admin: Option<bool>,
}

/// Parse server type from string, handling various formats
fn parse_server_type(type_str: &str) -> ServerType {
    let normalized = type_str
        .trim()
        .to_lowercase()
        .replace('_', "-")
        .replace(" ", "-");

    match normalized.as_str() {
        "sse" => ServerType::Sse,
        "streamable-http" | "streamablehttp" | "streamable" => ServerType::StreamableHttp,
        "openapi" | "open-api" => ServerType::Openapi,
        "stdio" => ServerType::Stdio,
        _ => {
            // Try to detect from common patterns
            if normalized.contains("sse") {
                ServerType::Sse
            } else if normalized.contains("http") || normalized.contains("stream") {
                ServerType::StreamableHttp
            } else if normalized.contains("openapi") || normalized.contains("open-api") {
                ServerType::Openapi
            } else {
                ServerType::Stdio
            }
        }
    }
}

/// Import from a JSON string (contents of mcp_settings.json)
pub async fn import_from_json(json: &str) -> Result<ImportSummary> {
    let settings: McpSettings = serde_json::from_str(json)?;
    let mut summary = ImportSummary::default();

    // Import servers
    if let Some(servers) = settings.mcp_servers {
        for (name, raw) in servers {
            // Try to get server type from multiple possible fields
            let type_str = raw.server_type
                .or(raw.server_type_alt)
                .unwrap_or_else(|| "stdio".to_string());

            let server_type = parse_server_type(&type_str);

            // Auto-detect type from URL if still stdio but url is present
            let server_type = if server_type == ServerType::Stdio && raw.url.is_some() {
                // If url is present but no command, likely SSE or HTTP
                if raw.command.is_none() {
                    ServerType::Sse
                } else {
                    server_type
                }
            } else {
                server_type
            };

            let cfg = ServerConfig {
                id: String::new(), // assigned by DB
                name: name.clone(),
                server_type,
                description: raw.description,
                command: raw.command,
                args: raw.args,
                env: raw.env,
                url: raw.url,
                headers: raw.headers,
                // Connection-relevant round-trip fields: importing an export
                // produced by this app must not silently drop these (the
                // export face emits them since the same-day export fix).
                // Parse failures are logged — a silently-vanished field is
                // indistinguishable from "never configured" during debugging.
                options: raw.options.and_then(|v| match serde_json::from_value(v) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        log::warn!("[import] server '{}': invalid options ({}), dropped", name, e);
                        None
                    }
                }),
                openapi: raw.openapi.and_then(|v| match serde_json::from_value(v) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        log::warn!("[import] server '{}': invalid openapi ({}), dropped", name, e);
                        None
                    }
                }),
                proxy: raw.proxy.and_then(|v| match serde_json::from_value(v) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        log::warn!("[import] server '{}': invalid proxy ({}), dropped", name, e);
                        None
                    }
                }),
                // Round-trip fields (see connection-relevant comment above).
                enable_keep_alive: raw.enable_keep_alive,
                keep_alive_interval: raw.keep_alive_interval,
                passthrough_headers: raw.passthrough_headers,
                per_session_client: raw.per_session_client,
                start_on_demand: raw.start_on_demand,
                idle_timeout_ms: raw.idle_timeout_ms,
                enabled: !raw.disabled.unwrap_or(false),
            };

            match server_service::create(&cfg).await {
                Ok(_) => summary.servers_imported += 1,
                Err(e) => {
                    log::warn!("Failed to import server '{}': {}", name, e);
                    summary.servers_skipped += 1;
                }
            }
        }
    }

    // Import users (skip if password_hash-only, since we can't verify those directly)
    if let Some(users) = settings.users {
        for raw in users {
            if let Some(password) = raw.password {
                let payload = UserPayload {
                    username: raw.username.clone(),
                    password,
                    role: if raw.admin.unwrap_or(false) {
                        Some(UserRole::Admin)
                    } else {
                        Some(UserRole::User)
                    },
                };
                match user_service::create(&payload).await {
                    Ok(_) => summary.users_imported += 1,
                    Err(e) => {
                        log::warn!("Failed to import user '{}': {}", raw.username, e);
                        summary.users_skipped += 1;
                    }
                }
            } else {
                log::warn!(
                    "Skipping user '{}': only password_hash available, manual reset required",
                    raw.username
                );
                summary.users_skipped += 1;
            }
        }
    }

    Ok(summary)
}

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub servers_imported: usize,
    pub servers_skipped: usize,
    pub users_imported: usize,
    pub users_skipped: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Export-shape round-trip: connection-relevant fields (keep-alive,
    /// passthrough headers, on-demand, proxy, options) must survive
    /// deserialization — an export→import cycle must not silently reset them.
    #[test]
    fn raw_server_config_round_trips_connection_fields() {
        let json = r#"{
            "mcpServers": {
                "srv": {
                    "type": "streamableHttp",
                    "url": "http://127.0.0.1:9/mcp",
                    "enableKeepAlive": true,
                    "keepAliveInterval": 15000,
                    "passthroughHeaders": ["X-Trace"],
                    "startOnDemand": false,
                    "idleTimeoutMs": 300000,
                    "perSessionClient": true
                }
            }
        }"#;
        let settings: McpSettings = serde_json::from_str(json).expect("parse");
        let raw = settings.mcp_servers.unwrap().remove("srv").unwrap();
        assert_eq!(raw.enable_keep_alive, Some(true));
        assert_eq!(raw.keep_alive_interval, Some(15000));
        assert_eq!(raw.passthrough_headers.as_ref().map(|h| h.as_slice()),
                   Some(&["X-Trace".to_string()][..]));
        assert_eq!(raw.start_on_demand, Some(false));
        assert_eq!(raw.idle_timeout_ms, Some(300000));
        assert_eq!(raw.per_session_client, Some(true));
    }

    /// snake_case aliases (origin mcp_settings.json) also parse.
    #[test]
    fn raw_server_config_accepts_snake_case_aliases() {
        let json = r#"{
            "mcpServers": {
                "srv": {
                    "enable_keep_alive": true,
                    "keep_alive_interval": 5000,
                    "passthrough_headers": ["A"],
                    "start_on_demand": true,
                    "idle_timeout_ms": 1000,
                    "per_session_client": false
                }
            }
        }"#;
        let settings: McpSettings = serde_json::from_str(json).expect("parse");
        let raw = settings.mcp_servers.unwrap().remove("srv").unwrap();
        assert_eq!(raw.enable_keep_alive, Some(true));
        assert_eq!(raw.keep_alive_interval, Some(5000));
        assert!(raw.passthrough_headers.is_some());
        assert_eq!(raw.start_on_demand, Some(true));
        assert_eq!(raw.idle_timeout_ms, Some(1000));
        assert_eq!(raw.per_session_client, Some(false));
    }

    /// Unknown top-level fields (e.g. origin's "groups") must not abort import.
    #[test]
    fn import_tolerates_unknown_top_level_fields() {
        let json = r#"{"mcpServers": {}, "groups": {"g1": {"servers": []}}, "users": []}"#;
        let parsed: McpSettings = serde_json::from_str(json).expect("parse");
        assert!(parsed.mcp_servers.unwrap().is_empty());
        assert!(parsed.users.unwrap().is_empty());
    }
}
