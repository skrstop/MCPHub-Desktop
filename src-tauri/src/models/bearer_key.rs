use serde::{Deserialize, Serialize};

/// A Bearer API key used for authenticating external HTTP access to MCP endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BearerKey {
    pub id: String,
    pub name: String,
    pub token: String,
    pub enabled: bool,
    /// "all" | "groups" | "servers" | "custom" | "exclude"
    pub access_type: String,
    pub allowed_groups: Vec<String>,
    pub allowed_servers: Vec<String>,
    /// #1413 "all-except" scope (access_type "exclude" only): group names/ids
    /// and server names hidden from this key. Dormant for other access types.
    pub excluded_groups: Vec<String>,
    pub excluded_servers: Vec<String>,
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
    #[serde(default)]
    pub excluded_groups: Vec<String>,
    #[serde(default)]
    pub excluded_servers: Vec<String>,
}

fn default_enabled() -> bool {
    true
}

fn default_access_type() -> String {
    "all".to_string()
}
