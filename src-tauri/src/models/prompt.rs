use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptArgument {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinPrompt {
    pub id: String,
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub template: String,
    #[serde(default)]
    pub arguments: Vec<PromptArgument>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub created_at: String,
}

/// Payload for creating or updating a builtin prompt.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinPromptPayload {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub template: String,
    #[serde(default)]
    pub arguments: Vec<PromptArgument>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

/// A page of builtin-prompt search results. `page` is 0-based.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptPage {
    pub items: Vec<BuiltinPrompt>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
}
