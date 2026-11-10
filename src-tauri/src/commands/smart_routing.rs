//! Smart Routing Tauri commands (Phase 4): status / reindex / performance / search.

use serde::{Deserialize, Serialize};

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
/// Participant count for the model-swap dialog's confirm-phase meta
/// ("N 个服务器"). Read-only; still gated for consistency with reindex.
pub async fn smart_routing_reindex_participants(
    session: tauri::State<'_, crate::commands::auth::SessionState>,
) -> Result<(usize, usize), String> {
    crate::commands::config::require_admin(&session).await?;
    crate::smart_routing::index::reindex_participants()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn smart_routing_reindex(session: tauri::State<'_, crate::commands::auth::SessionState>) -> Result<usize, String> {
    // Triggers a full embedding re-index and can spin up the shared mv model
    // runtime — admin-only (review round 8, 2026-10-04).
    crate::commands::config::require_admin(&session).await?;
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartSearchPayload {
    pub query: String,
    /// "all" (default) | group name/id | server name.
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Relevance search over the Smart Routing tool index (ServersPage dialog).
/// Same hybrid search the $smart meta tool uses; the scope selector narrows
/// the candidate set to one group's members or a single server.
#[tauri::command]
pub async fn smart_routing_search(
    session: tauri::State<'_, crate::commands::auth::SessionState>,
    payload: SmartSearchPayload,
) -> Result<Vec<crate::smart_routing::search::SmartSearchResult>, String> {
    crate::commands::config::require_admin(&session).await?;
    let query = payload.query.trim().to_string();
    if query.is_empty() {
        return Err("Query is required".to_string());
    }
    let scope = payload
        .scope
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "all");
    let allowed_servers = match scope {
        None => None,
        Some(s) => {
            // Group first (by name or id), expanding to its member server
            // names; anything else is a single-server scope. An unknown
            // scope yields an empty candidate set — search returns no hits
            // rather than silently degrading to a global search.
            let groups = crate::services::group_service::list_all()
                .await
                .map_err(|e| e.to_string())?;
            match groups.iter().find(|g| g.name == s || g.id == s) {
                Some(grp) => Some(
                    grp.servers
                        .iter()
                        .filter_map(|sv| {
                            sv.get("name").and_then(|n| n.as_str()).map(|x| x.to_string())
                        })
                        .collect::<Vec<String>>(),
                ),
                None => Some(vec![s.to_string()]),
            }
        }
    };
    crate::smart_routing::search::search(&query, payload.limit, allowed_servers, None)
        .await
        .map_err(|e| e.to_string())
}
