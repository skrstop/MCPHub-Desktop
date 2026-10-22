use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerToolConfig {
    pub id: String,
    pub server_name: String,
    pub item_type: String, // tool | prompt | resource
    pub item_name: String,
    pub enabled: bool,
    pub description: Option<String>,
    /// Server-level $smart pin (tools only). Root `$smart` lists pinned tools
    /// across servers; `$smart/{group}` unions them with group-member pins.
    #[serde(default)]
    pub pinned: bool,
    /// Top-level input-schema parameters models do not see (#1343 parity).
    /// Removed from the schema models receive and dropped from their calls;
    /// required parameters are never hidden. NULL = setting absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden_params: Option<Vec<String>>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerToolConfigPayload {
    pub server_name: String,
    pub item_type: String,
    pub item_name: String,
    pub enabled: bool,
    pub description: Option<String>,
    /// None = keep the existing pin state (toggle/description paths); Some = set.
    #[serde(default)]
    pub pinned: Option<bool>,
}
