use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// Server configuration within a group - supports tool/prompt/resource selection
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupServerConfig {
    pub name: String,
    #[serde(default = "default_all")]
    pub tools: JsonValue,  // "all" or ["tool1", "tool2"]
    #[serde(default = "default_all")]
    pub prompts: JsonValue,  // "all" or ["prompt1", "prompt2"]
    #[serde(default = "default_all")]
    pub resources: JsonValue,  // "all" or ["resource1", "resource2"]
}

fn default_all() -> JsonValue {
    JsonValue::String("all".to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub servers: Vec<JsonValue>,  // Can be string[] or GroupServerConfig[]
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupPayload {
    pub name: String,
    pub description: Option<String>,
    pub servers: Vec<JsonValue>,  // Can be string[] or GroupServerConfig[]
}
