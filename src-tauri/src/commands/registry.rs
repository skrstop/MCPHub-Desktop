const REGISTRY_BASE: &str = "https://registry.modelcontextprotocol.io/v0.1";

/// Proxy GET /registry/servers?limit=&cursor=&search= to the official MCP registry.
#[tauri::command]
pub async fn list_registry_servers(
    limit: Option<u32>,
    cursor: Option<String>,
    search: Option<String>,
) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::new();
    let mut req = client
        .get(format!("{}/servers", REGISTRY_BASE))
        .header("Accept", "application/json, application/problem+json");

    let mut params: Vec<(&str, String)> = Vec::new();
    if let Some(l) = limit {
        params.push(("limit", l.to_string()));
    }
    if let Some(c) = cursor {
        if !c.is_empty() {
            params.push(("cursor", c));
        }
    }
    if let Some(s) = search {
        if !s.is_empty() {
            params.push(("search", s));
        }
    }
    req = req.query(&params);

    proxy_send(req).await
}

/// Proxy GET /registry/servers/:name/versions to the official MCP registry.
#[tauri::command]
pub async fn get_registry_server_versions(name: String) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::new();
    let req = client
        .get(format!("{}/servers/{}/versions", REGISTRY_BASE, name))
        .header("Accept", "application/json, application/problem+json");
    proxy_send(req).await
}

async fn proxy_send(req: reqwest::RequestBuilder) -> Result<serde_json::Value, String> {
    let resp = req.send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("Registry returned HTTP {}", resp.status()));
    }
    resp.json::<serde_json::Value>().await.map_err(|e| e.to_string())
}

