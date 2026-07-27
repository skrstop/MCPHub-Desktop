use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// Server configuration within a group - supports tool/prompt/resource selection
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupServerConfig {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
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
    /// Builtin prompt names this group exposes. None = all (back-compat),
    /// Some([]) = none, Some([...]) = only those. Stored in groups.builtin_prompts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builtin_prompts: Option<Vec<String>>,
    /// Builtin resource URIs this group exposes. Same semantics as builtin_prompts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builtin_resources: Option<Vec<String>>,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupPayload {
    pub name: String,
    pub description: Option<String>,
    pub servers: Vec<JsonValue>,  // Can be string[] or GroupServerConfig[]
    /// Builtin prompt selection: "all"/null = expose all, array = expose those (empty = none).
    #[serde(default)]
    pub builtin_prompts: Option<JsonValue>,
    /// Builtin resource selection: "all"/null = expose all, array = expose those (empty = none).
    #[serde(default)]
    pub builtin_resources: Option<JsonValue>,
}
