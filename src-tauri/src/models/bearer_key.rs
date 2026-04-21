use serde::{Deserialize, Serialize};

/// A Bearer API key used for authenticating external HTTP access to MCP endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BearerKey {
    pub id: String,
    pub name: String,
    pub token: String,
    pub enabled: bool,
    /// "all" | "groups" | "servers" | "custom"
    pub access_type: String,
    pub allowed_groups: Vec<String>,
    pub allowed_servers: Vec<String>,
    pub created_at: String,
}

/// Payload for creating or updating a bearer key.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BearerKeyPayload {
    pub name: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_access_type")]
    pub access_type: String,
    #[serde(default)]
    pub allowed_groups: Vec<String>,
    #[serde(default)]
    pub allowed_servers: Vec<String>,
}

fn default_enabled() -> bool {
    true
}

fn default_access_type() -> String {
    "all".to_string()
}
