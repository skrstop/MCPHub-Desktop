use crate::{
    mcp::pool,
    models::server::{Tool, ToolCallResult},
    services::{log_service, server_tool_config_service},
};
use std::collections::HashMap;
use serde_json::Value;

#[tauri::command]
pub async fn list_tools(server_name: Option<String>) -> Result<Vec<Tool>, String> {
    if let Some(name) = server_name {
        let tools = pool::list_tools_for(&name).await.map_err(|e| e.to_string())?;
        server_tool_config_service::apply_tool_filters(&name, tools)
            .await
            .map_err(|e| e.to_string())
    } else {
        let all_tools = pool::list_all_tools().await;
        // Group by server and apply per-server filters
        let mut by_server: HashMap<String, Vec<Tool>> = HashMap::new();
        for tool in all_tools {
            by_server.entry(tool.server_name.clone()).or_default().push(tool);
        }
        let mut filtered: Vec<Tool> = Vec::new();
        for (sname, stools) in by_server {
            let f = server_tool_config_service::apply_tool_filters(&sname, stools)
                .await
                .unwrap_or_default();
            filtered.extend(f);
        }
        Ok(filtered)
    }
}

#[tauri::command]
pub async fn call_tool(
    server_name: String,
    tool_name: String,
    arguments: Value,
) -> Result<ToolCallResult, String> {
    let start = std::time::Instant::now();
    let result = pool::call_tool(&server_name, &tool_name, arguments.clone()).await;
    let duration_ms = start.elapsed().as_millis() as i64;

    match result {
        Ok(tool_result) => {
            let status = if tool_result.is_error { "error" } else { "success" };
            let output = serde_json::to_value(&tool_result).ok();
            let _ = log_service::write_activity(
                &server_name,
                &tool_name,
                Some(duration_ms),
                status,
                Some(arguments),
                output,
                None,
            )
            .await;
            Ok(tool_result)
        }
        Err(e) => {
            let err_msg = e.to_string();
            let _ = log_service::write_activity(
                &server_name,
                &tool_name,
                Some(duration_ms),
                "error",
                Some(arguments),
                None,
                Some(&err_msg),
            )
            .await;
            Err(err_msg)
        }
    }
}
