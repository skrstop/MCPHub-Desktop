use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub id: String,
    pub level: String,
    pub message: String,
    pub server_name: Option<String>,
    pub created_at: String,
}

/// Activity entry aligned with the activity_log table created in migration 0002.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEntry {
    pub id: String,
    pub timestamp: String,
    pub server: String,
    pub tool: String,
    pub duration_ms: Option<i64>,
    pub status: String,
    pub input: Option<serde_json::Value>,
    pub output: Option<serde_json::Value>,
    pub group_name: Option<String>,
    pub key_id: Option<String>,
    pub key_name: Option<String>,
    pub error_message: Option<String>,
}

/// Filter / pagination parameters for querying activity_log.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityQuery {
    pub page: Option<u32>,
    pub page_size: Option<u32>,
    pub server: Option<String>,
    pub status: Option<String>,
    pub tool: Option<String>,
}

/// Aggregate counts returned by get_activity_stats.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityStats {
    pub total: i64,
    pub success: i64,
    pub error: i64,
}

/// Paginated result returned by query_tool_activities.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityPage {
    pub data: Vec<ActivityEntry>,
    pub page: u32,
    pub page_size: u32,
    pub total: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogQuery {
    pub page: Option<u32>,
    pub page_size: Option<u32>,
    pub level: Option<String>,
    pub server_name: Option<String>,
}
