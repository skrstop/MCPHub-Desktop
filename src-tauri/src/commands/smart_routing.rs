//! Smart Routing Tauri commands (Phase 4): status / reindex / performance.

use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartRoutingStatus {
    /// The smartRouting config intent.
    pub enabled: bool,
    /// Progressive disclosure flag (drives 3-step vs 2-step meta tools).
    pub progressive_disclosure: bool,
    /// Whether the shared mv runtime is up (model loaded).
    pub mv_running: bool,
    /// Real embedding dimension of the loaded model.
    pub embed_dim: Option<u32>,
    /// Per-server row counts in the smart_tool index.
    pub indexed_servers: Vec<ServerIndexInfo>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerIndexInfo {
    pub server: String,
    pub tools: usize,
}

#[tauri::command]
pub async fn smart_routing_status() -> Result<SmartRoutingStatus, String> {
    let settings = crate::smart_routing::models::get_settings().await;
    let mv_running = crate::mv::is_running();
    let indexed = if mv_running {
        crate::smart_routing::store_index_summary()
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    Ok(SmartRoutingStatus {
        enabled: settings.enabled,
        progressive_disclosure: settings.progressive_disclosure,
        mv_running,
        embed_dim: crate::mv::embed_dim().map(|d| d as u32),
        indexed_servers: indexed
            .into_iter()
            .map(|(server, tools)| ServerIndexInfo { server, tools })
            .collect(),
    })
}

#[tauri::command]
pub async fn smart_routing_reindex() -> Result<usize, String> {
    crate::smart_routing::index::reindex_all()
        .await
        .map_err(|e| e.to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartRoutingPerformance {
    /// Server rows in the index (server-level embeddings).
    pub server_rows: usize,
    /// Tool rows in the index.
    pub tool_rows: usize,
    /// Total rows.
    pub total_rows: usize,
}

#[tauri::command]
pub async fn smart_routing_performance() -> Result<SmartRoutingPerformance, String> {
    let (server_rows, tool_rows, total_rows) = crate::smart_routing::store_performance()
        .await
        .map_err(|e| e.to_string())?;
    Ok(SmartRoutingPerformance {
        server_rows,
        tool_rows,
        total_rows,
    })
}
