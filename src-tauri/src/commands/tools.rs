use crate::{
    mcp::pool,
    models::server::{Tool, ToolCallResult},
    services::{app_logger, log_service, server_tool_config_service},
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
    // Check if tool is enabled before calling
    let tools = pool::list_tools_for(&server_name).await.map_err(|e| e.to_string())?;
    let filtered = server_tool_config_service::apply_tool_filters(&server_name, tools)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(tool) = filtered.iter().find(|t| t.name == tool_name) {
        if !tool.enabled {
            return Err(format!("Tool '{}' is disabled", tool_name));
        }
    }

    let start = std::time::Instant::now();
    let result = pool::call_tool(&server_name, &tool_name, arguments.clone()).await;
    let duration_ms = start.elapsed().as_millis() as i64;

    match result {
        Ok(tool_result) => {
            let status = if tool_result.is_error { "error" } else { "success" };
            let output = serde_json::to_value(&tool_result).ok();

            // Log to system logs (app_log)
            let log_msg = format!("[{}] Tool '{}' call {} ({}ms)", server_name, tool_name, status, duration_ms);
            if tool_result.is_error {
                app_logger::log_to_db("warn", &log_msg);
            } else {
                app_logger::log_to_db("info", &log_msg);
            }

            // Log to activity_log
            let _ = log_service::write_activity(
                &server_name,
                &tool_name,
                Some(duration_ms),
                status,
                Some(arguments),
                output,
                None,
                None, // No client IP for Tauri commands
            )
            .await;
            Ok(tool_result)
        }
        Err(e) => {
            let err_msg = e.to_string();

            // Log to system logs (app_log)
            let log_msg = format!("[{}] Tool '{}' call failed ({}ms): {}", server_name, tool_name, duration_ms, err_msg);
            app_logger::log_to_db("error", &log_msg);

            // Log to activity_log
            let _ = log_service::write_activity(
                &server_name,
                &tool_name,
                Some(duration_ms),
                "error",
                Some(arguments),
                None,
                Some(&err_msg),
                None, // No client IP for Tauri commands
            )
            .await;
            Err(err_msg)
        }
    }
}
