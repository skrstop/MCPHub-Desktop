const REGISTRY_BASE: &str = "https://registry.modelcontextprotocol.io/v0.1";

/// Proxy GET /registry/servers?limit=&cursor=&search= to the official MCP registry.
#[tauri::command]
pub async fn list_registry_servers(
    limit: Option<u32>,
    cursor: Option<String>,
    search: Option<String>,
) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))?;
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

/// Proxy GET /registry/servers/versions?serverName= to the official MCP registry.
/// Uses query parameter style as per frontend useRegistryData.ts.
#[tauri::command]
pub async fn get_registry_server_versions(server_name: String) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))?;
    let req = client
        .get(format!("{}/servers/{}/versions", REGISTRY_BASE, url_encode_path(&server_name)))
        .header("Accept", "application/json, application/problem+json");
    proxy_send(req).await
}

/// Proxy GET /registry/servers/version?serverName=&version= to the official MCP registry.
/// Uses query parameter style as per frontend useRegistryData.ts.
#[tauri::command]
pub async fn get_registry_server_version(server_name: String, version: String) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))?;
    let req = client
        .get(format!(
            "{}/servers/{}/versions/{}",
            REGISTRY_BASE,
            url_encode_path(&server_name),
            url_encode_path(&version)
        ))
        .header("Accept", "application/json, application/problem+json");
    proxy_send(req).await
}

async fn proxy_send(req: reqwest::RequestBuilder) -> Result<serde_json::Value, String> {
    let resp = req.send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("Registry returned HTTP {}", resp.status()));
    }
    // Cap response size — registry is untrusted-adjacent; a huge body must
    // not balloon memory. Stream with a running cap: a chunked (no
    // Content-Length) response previously accumulated fully into memory
    // before the post-read check could fire (review round 10).
    const MAX_BODY: usize = 32 * 1024 * 1024;
    if let Some(len) = resp.content_length() {
        if len as usize > MAX_BODY {
            return Err("Registry response too large".to_string());
        }
    }
    use futures_util::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut body: Vec<u8> = Vec::with_capacity(64 * 1024);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        if body.len() + chunk.len() > MAX_BODY {
            return Err("Registry response too large".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|e| e.to_string())
}


/// Percent-encode a value for safe interpolation into a URL path segment:
/// server names/versions come from callers and could otherwise rewrite the
/// path with `/` or `?`.
fn url_encode_path(v: &str) -> String {
    const SAFE: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'~');
    percent_encoding::utf8_percent_encode(v, SAFE).to_string()
}
