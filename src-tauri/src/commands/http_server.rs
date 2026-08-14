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

/// Get the current HTTP server status — running / port / last error.
///
/// The `error` field carries the last bind/start failure message (Windows
/// firewall / port-in-use, etc.); it is None when the server is running or has
/// never been started. The frontend fetches this on mount to catch a startup
/// failure it may have missed (the server starts before the webview registers
/// its event listener); live updates arrive on the `http://server-status` event.
#[tauri::command]
pub async fn get_http_server_status() -> Result<Value, String> {
    let s = http_server::current_status();
    serde_json::to_value(&s).map_err(|e| e.to_string())
}
