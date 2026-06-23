/// Context footprint calculation — estimates token counts for MCP tools/prompts/resources.
///
/// Token estimation uses a simple heuristic: ~4 characters per token (English text).
/// This matches the behavior of the original Node.js MCPHub backend.
use crate::{
    mcp::pool,
    services::server_tool_config_service,
};
use serde::Serialize;

/// Approximate tokens from a string (4 chars ≈ 1 token).
fn estimate_tokens(text: &str) -> u64 {
    ((text.len() as f64) / 4.0).ceil() as u64
}

/// Estimate tokens for a JSON value (serialized length).
fn estimate_json_tokens(value: &serde_json::Value) -> u64 {
    let s = serde_json::to_string(value).unwrap_or_default();
    estimate_tokens(&s)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemCost {
    pub kind: String,   // "tool" | "prompt" | "resource"
    pub name: String,
    pub cost: u64,
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerCost {
    pub name: String,
    pub connected: bool,
    pub exposed: u64,   // tokens from enabled items
    pub gross: u64,     // tokens from all items
    pub items: Vec<ItemCost>,
}

/// Calculate context footprint for all connected servers.
#[tauri::command]
pub async fn get_server_costs() -> Result<Vec<ServerCost>, String> {
    let all_statuses = pool::get_all_statuses().await;
    let mut result = Vec::new();

    for status in &all_statuses {
        let tools = pool::list_tools_for(&status.name).await.unwrap_or_default();

        // Get per-server tool/prompt/resource configs for enabled state
        let tool_configs = server_tool_config_service::list_for_server(&status.name, Some("tool"))
            .await
            .unwrap_or_default();

        let mut items: Vec<ItemCost> = Vec::new();

        // Calculate tool costs
        for tool in &tools {
            let mut cost: u64 = 0;
            // Tool description tokens
            if let Some(ref desc) = tool.description {
                cost += estimate_tokens(desc);
            }
            // Input schema tokens
            cost += estimate_json_tokens(&tool.input_schema);

            // Check if tool is enabled (default: true)
            let enabled = tool_configs.iter()
                .find(|c| c.item_type == "tool" && c.item_name == tool.name)
                .map(|c| c.enabled)
                .unwrap_or(true);

            items.push(ItemCost {
                kind: "tool".to_string(),
                name: tool.name.clone(),
                cost,
                enabled,
            });
        }

        let exposed: u64 = items.iter().filter(|i| i.enabled).map(|i| i.cost).sum();
        let gross: u64 = items.iter().map(|i| i.cost).sum();

        result.push(ServerCost {
            name: status.name.clone(),
            connected: status.connected,
            exposed,
            gross,
            items,
        });
    }

    Ok(result)
}

/// Calculate context footprint for all groups (sum of their servers).
#[tauri::command]
pub async fn get_group_costs() -> Result<Vec<serde_json::Value>, String> {
    let groups = crate::services::group_service::list_all().await
        .map_err(|e| e.to_string())?;
    let server_costs = get_server_costs().await?;
    let cost_map: std::collections::HashMap<String, &ServerCost> = server_costs.iter()
        .map(|c| (c.name.clone(), c))
        .collect();

    let mut result = Vec::new();
    for group in &groups {
        let server_names: Vec<String> = group.servers.iter()
            .filter_map(|s| {
                if s.is_string() {
                    s.as_str().map(|v| v.to_string())
                } else {
                    s.get("name").and_then(|v| v.as_str()).map(|v| v.to_string())
                }
            })
            .collect();
        let exposed: u64 = server_names.iter()
            .filter_map(|n| cost_map.get(n))
            .map(|c| c.exposed)
            .sum();
        let gross: u64 = server_names.iter()
            .filter_map(|n| cost_map.get(n))
            .map(|c| c.gross)
            .sum();

        result.push(serde_json::json!({
            "name": group.name,
            "serverCount": server_names.len(),
            "exposed": exposed,
            "gross": gross,
        }));
    }

    Ok(result)
}
