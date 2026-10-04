use crate::{
    auth as auth_util,
    services::{bearer_key_service, config_service, http_server, settings_import},
};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use crate::commands::auth::SessionState;

/// Get public configuration (skipAuth setting) without authentication
/// This is used by the frontend to determine if login should be skipped
#[tauri::command]
pub async fn get_public_config() -> Result<serde_json::Value, String> {
    let config = config_service::get().await.map_err(|e| e.to_string())?;

    let skip_auth = config
        .get("routing")
        .and_then(|r| r.get("skipAuth"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true); // Default to true for desktop app

    let permissions = config
        .get("permissions")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));

    Ok(serde_json::json!({
        "skipAuth": skip_auth,
        "permissions": permissions
    }))
}

/// Helper to verify that the current session belongs to an admin user.
/// In no-login (skipAuth) mode the dashboard treats the user as an admin,
/// so admin-gated read operations are allowed without a session token.
pub(crate) async fn require_admin(session: &SessionState) -> Result<(), String> {
    if crate::commands::auth::is_skip_auth_enabled().await {
        return Ok(());
    }
    let token_str = {
        let guard = session.0.lock().await;
        guard.as_ref().ok_or("Not authenticated")?.token.clone()
    };
    let claims = auth_util::verify_token(&token_str).map_err(|e| e.to_string())?;
    if claims.role != "admin" {
        return Err("Admin access required".to_string());
    }
    Ok(())
}

#[tauri::command]
pub async fn get_system_config(session: State<'_, SessionState>) -> Result<serde_json::Value, String> {
    // Admin-gated: returns the same payload as get_settings (contains
    // smartRouting.llmProviderApiKey-class secrets); without this gate a
    // non-admin session could read it directly.
    require_admin(&session).await?;
    config_service::get().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_system_config(session: State<'_, SessionState>, config: serde_json::Value) -> Result<serde_json::Value, String> {
    // Admin-gated: without this gate any caller could write routing.skipAuth
    // (disabling authentication) or permissions — a privilege escalation.
    require_admin(&session).await?;
    // Capture the PREVIOUS smartRouting.enabled so a false→true transition can
    // pull the model + index up immediately (otherwise the user must restart
    // the app before Smart Routing works — the UI switch implies live effect).
    let prev_smart_enabled = config_service::get()
        .await
        .ok()
        .and_then(|c| {
            c.get("smartRouting")
                .and_then(|r| r.get("enabled"))
                .and_then(|v| v.as_bool())
        })
        .unwrap_or(false);

    let result = config_service::update(&config).await.map_err(|e| e.to_string())?;
    // Sync HTTP server state (start/stop) based on updated config
    http_server::sync_with_config().await;

    // Smart Routing enable/disable transitions (fire-and-forget; errors logged).
    let new_smart_enabled = result
        .get("smartRouting")
        .and_then(|r| r.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !prev_smart_enabled && new_smart_enabled {
        let Some(app) = crate::mcp::progress::get_app_handle().cloned() else {
            log::warn!("[smart] post-enable hook skipped: app handle not ready");
            return Ok(result);
        };
        tauri::async_runtime::spawn(async move {
            match crate::mv::ensure_started("smart", &app).await {
                Ok(_) => {
                    // Model up — index the connected servers (skip-check makes
                    // repeat runs cheap; no-op when already indexed).
                    if let Err(e) = crate::smart_routing::index::reindex_all().await {
                        log::warn!("[smart] post-enable reindex: {}", e);
                    }
                }
                Err(e) => log::error!("[smart] post-enable model start failed: {:#}", e),
            }
        });
    } else if prev_smart_enabled && !new_smart_enabled {
        // Only release when RAG isn't holding the model too. Release INSIDE a
        // spawned task that first waits for any in-flight enable-load: a fast
        // enable→disable would otherwise take the (still empty) runtime slot
        // and leak the model running with zero consumers once the load
        // completes.
        let rag_enabled = crate::rag::service::config_enabled().await;
        if !rag_enabled {
            tauri::async_runtime::spawn(async move {
                crate::mv::wait_while_initializing(std::time::Duration::from_secs(180)).await;
                crate::mv::release("smart").await;
            });
        }
    }
    Ok(result)
}

/// Returns the full settings payload expected by the frontend SettingsContext:
/// { systemConfig: { routing, install, smartRouting, ... }, bearerKeys: [...] }
/// Admin-gated: the payload includes bearerKeys (HTTP endpoint tokens) and
/// smartRouting.llmProviderApiKey. In skipAuth mode require_admin short-circuits.
#[tauri::command]
pub async fn get_settings(session: State<'_, SessionState>) -> Result<serde_json::Value, String> {
    require_admin(&session).await?;
    let system_config = config_service::get().await.map_err(|e| e.to_string())?;
    let bearer_keys = bearer_key_service::list_all().await.map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "systemConfig": system_config,
        "bearerKeys": bearer_keys
    }))
}

/// Import servers/users from a mcp_settings.json string.
/// Admin-gated: the import can create users (including admins with known
/// passwords) — without this gate any local IPC caller could escalate in
/// multi-user (skipAuth=false) mode. Mirrors bearer_keys.rs require_admin.
#[tauri::command]
pub async fn import_settings(
    session: State<'_, SessionState>,
    json: String,
) -> Result<settings_import::ImportSummary, String> {
    require_admin(&session).await?;
    settings_import::import_from_json(&json)
        .await
        .map_err(|e| e.to_string())
}

type HeaderMapCfg = Option<std::collections::HashMap<String, String>>;

fn is_sensitive_name(lower_k: &str) -> bool {
    lower_k.contains("key")
        || lower_k.contains("token")
        || lower_k.contains("secret")
        || lower_k.contains("password")
        || lower_k.contains("auth")
}

/// Redact sensitive env vars / HTTP headers for export & copy.
/// Headers routinely carry `Authorization` / `X-Api-Key`; exporting them
/// verbatim would leak upstream credentials into the shared settings file.
fn redact_map(map: HeaderMapCfg) -> HeaderMapCfg {
    map.map(|env| {
        env.iter()
            .map(|(k, v)| {
                if is_sensitive_name(&k.to_lowercase()) {
                    (k.clone(), "***REDACTED***".to_string())
                } else {
                    (k.clone(), v.clone())
                }
            })
            .collect::<std::collections::HashMap<String, String>>()
    })
}

/// Get a single server's config for copying (no admin auth required).
/// Sensitive values (API keys, tokens) are redacted.
#[tauri::command]
pub async fn get_server_config_for_copy(server_name: String) -> Result<serde_json::Value, String> {
    use crate::services::server_service;

    let server = server_service::get_by_name(&server_name)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Server '{}' not found", server_name))?;

    // Redact sensitive environment variables and headers for copy-out.
    let redacted_env = redact_map(server.env.clone());
    let redacted_headers = redact_map(server.headers.clone());
    // Proxy credentials never leave the machine in plaintext exports.
    let redacted_proxy = server.proxy.as_ref().map(|p| {
        let mut v = serde_json::to_value(p).unwrap_or_else(|_| serde_json::json!({}));
        if let Some(obj) = v.as_object_mut() {
            if let Some(pw) = obj.get("password").and_then(|x| x.as_str()) {
                if !pw.is_empty() {
                    obj.insert("password".to_string(), serde_json::json!("***REDACTED***"));
                }
            }
            if let Some(u) = obj.get("username").and_then(|x| x.as_str()) {
                if !u.is_empty() {
                    obj.insert("username".to_string(), serde_json::json!("***REDACTED***"));
                }
            }
        }
        v
    });
    // OpenAPI inline security credentials (header values, apiKey/bearer) are
    // redacted the same way; the spec URL/schema stay intact so the copy can
    // be re-imported and re-authenticated by the user.
    let redacted_openapi = server.openapi.as_ref().and_then(|o| {
        let mut v = serde_json::to_value(o).ok()?;
        if let Some(obj) = v.as_object_mut() {
            if let Some(h) = obj.get_mut("headers").and_then(|x| x.as_object_mut()) {
                if let Some(redacted) = redact_map(Some(
                    h.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect::<std::collections::HashMap<String, String>>(),
                )) {
                    if let Ok(rv) = serde_json::to_value(redacted) {
                        if let Some(rm) = rv.as_object() { *h = rm.clone(); }
                    }
                }
            }
            if let Some(sec) = obj.get_mut("security") {
                if let Ok(s) = serde_json::to_value(&*sec) {
                    let s = redact_secret_leaves(s);
                    *sec = serde_json::from_value(s).unwrap_or(serde_json::Value::Null);
                }
            }
        }
        Some(v)
    });

    Ok(serde_json::json!({
        "mcpServers": {
            server.name: {
                "type": server.server_type,
                "description": server.description,
                "command": server.command,
                "args": server.args,
                "env": redacted_env,
                "url": server.url,
                "disabled": !server.enabled,
                "headers": redacted_headers,
                "proxy": redacted_proxy,
                "openapi": redacted_openapi,
                "options": server.options,
                "perSessionClient": server.per_session_client,
                "startOnDemand": server.start_on_demand,
                "idleTimeoutMs": server.idle_timeout_ms,
            }
        }
    }))
}

/// Recursively replace string leaves under sensitive-looking keys.
fn redact_secret_leaves(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.into_iter()
                .map(|(k, val)| {
                    if is_sensitive_name(&k.to_lowercase()) && val.is_string() {
                        (k, serde_json::json!("***REDACTED***"))
                    } else {
                        (k, redact_secret_leaves(val))
                    }
                })
                .collect(),
        ),
        serde_json::Value::Array(a) => {
            serde_json::Value::Array(a.into_iter().map(redact_secret_leaves).collect())
        }
        other => other,
    }
}

/// Export current server/group/system config as a JSON string (mcp_settings.json compatible)
/// Requires admin access. Sensitive values (API keys, tokens) are redacted.
#[tauri::command]
pub async fn export_settings(session: State<'_, SessionState>) -> Result<String, String> {
    require_admin(&session).await?;

    use crate::services::{group_service, server_service};
    use std::collections::HashMap;

    let servers = server_service::list_all().await.map_err(|e| e.to_string())?;
    let groups = group_service::list_all().await.map_err(|e| e.to_string())?;

    let mut mcp_servers: HashMap<String, serde_json::Value> = HashMap::new();
    for s in &servers {
        let redacted_env = redact_map(s.env.clone());
        let redacted_headers = redact_map(s.headers.clone());
        let redacted_proxy = s.proxy.as_ref().map(|p| {
            let mut v = serde_json::to_value(p).unwrap_or_else(|_| serde_json::json!({}));
            if let Some(obj) = v.as_object_mut() {
                if obj.get("password").and_then(|x| x.as_str()).is_some_and(|p| !p.is_empty()) {
                    obj.insert("password".to_string(), serde_json::json!("***REDACTED***"));
                }
                if obj.get("username").and_then(|x| x.as_str()).is_some_and(|u| !u.is_empty()) {
                    obj.insert("username".to_string(), serde_json::json!("***REDACTED***"));
                }
            }
            v
        });
        let redacted_openapi = s.openapi.as_ref().and_then(|o| {
            let mut v = serde_json::to_value(o).ok()?;
            if let Some(obj) = v.as_object_mut() {
                if let Some(h) = obj.get_mut("headers").and_then(|x| x.as_object_mut()) {
                    if let Some(redacted) = redact_map(Some(
                        h.iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect::<std::collections::HashMap<String, String>>(),
                    )) {
                        if let Ok(rv) = serde_json::to_value(redacted) {
                            if let Some(rm) = rv.as_object() { *h = rm.clone(); }
                        }
                    }
                }
                if let Some(sec) = obj.get_mut("security") {
                    if let Ok(sv) = serde_json::to_value(&*sec) {
                        *sec = serde_json::from_value(redact_secret_leaves(sv)).unwrap_or(serde_json::Value::Null);
                    }
                }
            }
            Some(v)
        });

        mcp_servers.insert(
            s.name.clone(),
            serde_json::json!({
                "type": s.server_type,
                "description": s.description,
                "command": s.command,
                "args": s.args,
                "env": redacted_env,
                "url": s.url,
                "disabled": !s.enabled,
                "headers": redacted_headers,
                "proxy": redacted_proxy,
                "openapi": redacted_openapi,
                "options": s.options,
                "perSessionClient": s.per_session_client,
                "startOnDemand": s.start_on_demand,
                "idleTimeoutMs": s.idle_timeout_ms,
            }),
        );
    }

    let mut groups_map: HashMap<String, serde_json::Value> = HashMap::new();
    for g in &groups {
        groups_map.insert(g.name.clone(), serde_json::json!({ "servers": g.servers }));
    }

    let output = serde_json::json!({
        "mcpServers": mcp_servers,
        "groups": groups_map,
    });

    serde_json::to_string_pretty(&output).map_err(|e| e.to_string())
}

/// Show a native "Save As" dialog and write the given JSON text to the chosen
/// file. The Tauri webview cannot perform programmatic blob-URL downloads the
/// way a browser can, so the export "Download JSON" button routes here instead.
/// Returns the saved file path, or an error message (e.g. dialog cancelled).
#[tauri::command]
pub async fn save_settings_json(
    app: AppHandle,
    content: String,
    file_name: Option<String>,
) -> Result<String, String> {
    let default_name = file_name.unwrap_or_else(|| "mcp_settings.json".to_string());
    let file_path = app
        .dialog()
        .file()
        .add_filter("JSON", &["json"])
        .set_file_name(default_name)
        .blocking_save_file();

    let Some(file_path) = file_path else {
        // User cancelled the save dialog — surface as a non-fatal "no file" result.
        return Err("cancelled".to_string());
    };

    let path = file_path.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&path, content.as_bytes()).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}
