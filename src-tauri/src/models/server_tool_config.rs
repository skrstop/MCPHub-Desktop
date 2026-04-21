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
}
