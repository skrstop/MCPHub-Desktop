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
    pub created_at: String,
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
    pub source_ip: Option<String>,
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
    /// 分组筛选（group_name 列等值；2026-09-05 新增——此前被 tauriClient 丢弃）
    #[serde(default)]
    pub group_name: Option<String>,
    /// API 秘钥筛选（key_name 列等值；同上）
    #[serde(default)]
    pub key_name: Option<String>,
}

/// Aggregate counts returned by get_activity_stats.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityStats {
    pub total: i64,
    pub success: i64,
    pub error: i64,
    pub avg_duration: f64,
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

/// 分页筛选候选结果（get_activity_filter_options 返回）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityFilterOptionsPage {
    pub options: Vec<String>,
    pub total: i64,
    pub page: u32,
    pub page_size: u32,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogQuery {
    pub page: Option<u32>,
    pub page_size: Option<u32>,
    pub level: Option<String>,
    pub server_name: Option<String>,
    /// 全文搜索关键词（FTS5 中/英/拼音分词匹配 message；空/缺省=不过滤）
    #[serde(default)]
    pub search: Option<String>,
}
