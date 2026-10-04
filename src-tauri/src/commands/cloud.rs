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
    if api_key.is_empty() {
        // Parity with get_cloud_server_tools: a request with an empty
        // Authorization header would only surface a confusing upstream 401.
        return Err("MCPROUTER_API_KEY_NOT_CONFIGURED".to_string());
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))?;
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

    let body = read_capped_body(resp).await?;
    let data: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
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
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))?;
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

    let body = read_capped_body(resp).await?;
    let data: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    let tools = data["data"]["tools"].clone();
    Ok(if tools.is_null() {
        serde_json::json!([])
    } else {
        tools
    })
}

/// Read the response body with a streaming 32MB cap: a chunked (no
/// Content-Length) response previously accumulated fully into memory before
/// the post-read check could fire (review round 10).
async fn read_capped_body(resp: reqwest::Response) -> Result<Vec<u8>, String> {
    const MAX_BODY: usize = 32 * 1024 * 1024;
    if let Some(len) = resp.content_length() {
        if len as usize > MAX_BODY {
            return Err("MCPRouter response too large".to_string());
        }
    }
    use futures_util::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut body: Vec<u8> = Vec::with_capacity(64 * 1024);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        if body.len() + chunk.len() > MAX_BODY {
            return Err("MCPRouter response too large".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
