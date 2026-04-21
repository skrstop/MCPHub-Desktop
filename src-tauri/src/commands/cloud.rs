use crate::services::config_service;

const DEFAULT_BASE_URL: &str = "https://api.mcprouter.to/v1";

async fn get_mcprouter_config() -> Result<(String, String, String, String), String> {
    let cfg = config_service::get().await.map_err(|e| e.to_string())?;
    let mcp_router = &cfg["mcpRouter"];
    let api_key = mcp_router["apiKey"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let referer = mcp_router["referer"]
        .as_str()
        .unwrap_or("https://www.mcphub.app")
        .to_string();
    let title = mcp_router["title"]
        .as_str()
        .unwrap_or("MCPHub")
        .to_string();
    let base_url = mcp_router["baseUrl"]
        .as_str()
        .unwrap_or(DEFAULT_BASE_URL)
        .to_string();
    Ok((api_key, referer, title, base_url))
}

/// List all available cloud servers from MCPRouter.
#[tauri::command]
pub async fn list_cloud_servers() -> Result<serde_json::Value, String> {
    let (api_key, referer, title, base_url) = get_mcprouter_config().await?;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/list-servers", base_url))
        .header("Authorization", format!("Bearer {}", api_key))
        .header("HTTP-Referer", referer)
        .header("X-Title", title)
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({}))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("MCPRouter returned HTTP {}", resp.status()));
    }

    let data: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    // data.data.servers
    let servers = data["data"]["servers"].clone();
    Ok(if servers.is_null() {
        serde_json::json!([])
    } else {
        servers
    })
}

/// Get tools for a specific cloud server.
#[tauri::command]
pub async fn get_cloud_server_tools(server: String) -> Result<serde_json::Value, String> {
    let (api_key, referer, title, base_url) = get_mcprouter_config().await?;
    if api_key.is_empty() {
        return Err("MCPROUTER_API_KEY_NOT_CONFIGURED".to_string());
    }
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/list-tools", base_url))
        .header("Authorization", format!("Bearer {}", api_key))
        .header("HTTP-Referer", referer)
        .header("X-Title", title)
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({ "server": server }))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("MCPRouter returned HTTP {}", resp.status()));
    }

    let data: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let tools = data["data"]["tools"].clone();
    Ok(if tools.is_null() {
        serde_json::json!([])
    } else {
        tools
    })
}
