use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinResource {
    pub id: String,
    pub uri: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub mime_type: String,
    pub content: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub created_at: String,
}

/// Payload for creating or updating a builtin resource.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinResourcePayload {
    pub uri: String,
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(default = "default_mime_type")]
    pub mime_type: String,
    pub content: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

fn default_mime_type() -> String {
    "text/plain".to_string()
}
