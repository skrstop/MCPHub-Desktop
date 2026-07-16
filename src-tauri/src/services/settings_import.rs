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
                options: None,
                openapi: None,
                per_session_client: None,
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
pub struct ImportSummary {
    pub servers_imported: usize,
    pub servers_skipped: usize,
    pub users_imported: usize,
    pub users_skipped: usize,
}
