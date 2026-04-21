use serde::{Deserialize, Serialize};

/// A single market server entry matching the servers.json schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketServer {
    pub name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub repository: Option<serde_json::Value>,
    pub homepage: Option<String>,
    pub author: Option<serde_json::Value>,
    pub license: Option<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub examples: Option<Vec<serde_json::Value>>,
    pub installations: Option<serde_json::Value>,
    pub arguments: Option<serde_json::Value>,
    pub tools: Option<Vec<serde_json::Value>>,
    pub is_official: Option<bool>,
}
