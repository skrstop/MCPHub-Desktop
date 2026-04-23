/// HTTP server management commands
use crate::services::{config_service, http_server};
use serde_json::{json, Value};

/// Start the embedded HTTP server on the given port.
/// Reads the body limit from current config.
#[tauri::command]
pub async fn start_http_server(port: u16) -> Result<Value, String> {
    let body_limit_bytes = config_service::get()
        .await
        .ok()
        .and_then(|c| c.get("routing").and_then(|r| r.get("jsonBodyLimit")).and_then(|v| v.as_str()).map(|s| http_server::parse_body_limit(s)))
        .unwrap_or(1024 * 1024);
    http_server::start(port, body_limit_bytes)
        .await
        .map_err(|e| e.to_string())?;
    Ok(json!({ "success": true, "port": port }))
}

/// Stop the embedded HTTP server.
#[tauri::command]
pub async fn stop_http_server() -> Result<Value, String> {
    http_server::stop().await;
    Ok(json!({ "success": true }))
}

/// Get the current HTTP server status.
#[tauri::command]
pub async fn get_http_server_status() -> Result<Value, String> {
    let port = http_server::current_port().await;
    Ok(json!({
        "running": port.is_some(),
        "port": port,
    }))
}
