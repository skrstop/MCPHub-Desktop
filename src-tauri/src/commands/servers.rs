use crate::{
    mcp::{pool, progress},
    models::server::{ServerConfig, ServerInfo, ServerStatus, ServerType},
    services::{mcp_manager, server_service, server_tool_config_service, runtime_env},
};

#[tauri::command]
pub async fn list_servers() -> Result<Vec<ServerInfo>, String> {
    let configs = server_service::list_all().await.map_err(|e| e.to_string())?;
    let mut result = Vec::new();

    for cfg in configs {
        let (status, tools) = pool::get_entry_info(&cfg.name).await.unwrap_or_else(|| (
            ServerStatus {
                name: cfg.name.clone(),
                connected: false,
                starting: false,
                start_on_demand: cfg.start_on_demand.unwrap_or(false),
                tool_count: 0,
                error: None,
                last_connected: None,
                server_version: None,
            },
            vec![],
        ));
        // Apply tool enabled/description configs
        let tools = server_tool_config_service::apply_tool_filters(&cfg.name, tools)
            .await
            .unwrap_or_default();
        result.push(ServerInfo { config: cfg, status, tools, prompts: Vec::new(), resources: Vec::new() });
    }
    // Append the "mcphub-desktop" builtin server (virtual, no DB row), which
    // bundles the RAG tools + builtin prompts + builtin resources as one
    // server's capabilities. Always shown so groups can select its
    // prompts/resources even when RAG is off.
    if let Some(info) = crate::rag::service::builtin_server_info().await {
        result.push(info);
    }
    Ok(result)
}

#[tauri::command]
pub async fn get_server(name: String) -> Result<Option<ServerInfo>, String> {
    let cfg = server_service::get_by_name(&name)
        .await
        .map_err(|e| e.to_string())?;

    if let Some(cfg) = cfg {
        let (status, tools) = pool::get_entry_info(&name).await.unwrap_or_else(|| (
            ServerStatus {
                name: name.clone(),
                connected: false,
                starting: false,
                start_on_demand: cfg.start_on_demand.unwrap_or(false),
                tool_count: 0,
                error: None,
                last_connected: None,
                server_version: None,
            },
            vec![],
        ));
        // Apply tool enabled/description configs
        let tools = server_tool_config_service::apply_tool_filters(&name, tools)
            .await
            .unwrap_or_default();
        Ok(Some(ServerInfo { config: cfg, status, tools, prompts: Vec::new(), resources: Vec::new() }))
    } else {
        Ok(None)
    }
}

#[tauri::command]
pub async fn add_server(config: ServerConfig) -> Result<ServerInfo, String> {
    let saved = server_service::create(&config).await.map_err(|e| e.to_string())?;
    if saved.enabled {
        // Connect in background so the API returns immediately
        let saved_clone = saved.clone();
        tauri::async_runtime::spawn(async move {
            pool::connect_server(&saved_clone).await;
        });
    }
    let status = ServerStatus {
        name: saved.name.clone(),
        connected: false,
        starting: saved.enabled,
        start_on_demand: saved.start_on_demand.unwrap_or(false),
        tool_count: 0,
        error: None,
        last_connected: None,
        server_version: None,
    };
    Ok(ServerInfo { config: saved, status, tools: vec![], prompts: Vec::new(), resources: Vec::new() })
}

#[tauri::command]
pub async fn update_server(name: String, config: ServerConfig) -> Result<ServerInfo, String> {
    // Mirror of upstream #1055 ("avoid unnecessary runtime reloads when editing
    // a server"): if no connection-relevant field changed, persist + refresh the
    // in-memory access metadata only, WITHOUT tearing down and reconnecting the
    // live client. Otherwise (previously) editing a server's description would
    // kill the MCP connection and restart the stdio process for nothing.
    //
    // We still `disconnect_server` first on the connection-changed path (also
    // covers the rename case: the old name's runtime is closed before the row is
    // rewritten to the new name, so no stdio child is orphaned).
    let existing = server_service::get_by_name(&name).await.map_err(|e| e.to_string())?;

    // A rename changes the pool key (old-name entry must be torn down + a new
    // one inserted under the new name), so it ALWAYS needs a reconnect even if
    // no connection-relevant field changed. Mirrors upstream #1055's
    // `if (isRenaming) { closeServer(name); }` + `if (!isRenaming && !hasConnectionRelevantChange)`.
    let is_renaming = existing.as_ref().map(|e| e.name != config.name).unwrap_or(false);
    let connection_relevant_changed = match &existing {
        Some(prev) => has_connection_relevant_change(prev, &config),
        // No existing row (e.g. direct API call for a missing server): persist +
        // (re)connect from scratch, same as before.
        None => true,
    };
    let needs_reconnect = connection_relevant_changed || is_renaming;

    // Persist first - this must not be blocked by the connect attempt.
    let saved = server_service::update(&name, &config)
        .await
        .map_err(|e| e.to_string())?;

    if needs_reconnect && saved.enabled {
        // Connection-relevant field changed (command/url/args/env/headers/
        // options/openapi/perSessionClient/startOnDemand/idleTimeoutMs/proxy/
        // keepAlive/type) OR the server was renamed: tear down the old runtime
        // (keyed by the OLD name on rename) and reconnect in the background.
        // The DB write above already persists the config; connecting is a
        // best-effort side effect that may take minutes (npx/uvx downloads,
        // unreachable remotes) and must not hold the save response hostage.
        pool::disconnect_server(&name).await.ok();
        let saved_clone = saved.clone();
        tauri::async_runtime::spawn(async move {
            pool::connect_server(&saved_clone).await;
        });
        // A reconnect was triggered, so the live runtime is gone: report a
        // synthesized "starting" status for the freshly-spawned connect.
        let status = ServerStatus {
            name: saved.name.clone(),
            connected: false,
            starting: saved.enabled,
            start_on_demand: saved.start_on_demand.unwrap_or(false),
            tool_count: 0,
            error: None,
            last_connected: None,
            server_version: None,
        };
        return Ok(ServerInfo { config: saved, status, tools: vec![], prompts: Vec::new(), resources: Vec::new() });
    }

    if needs_reconnect {
        // Connection-relevant change (or rename) on a now-disabled server, or an
        // edit that disabled it: tear down the live runtime but do not reconnect.
        pool::disconnect_server(&name).await.ok();
    }

    // Access/metadata-only edit (description etc.), a disabled server, or a
    // no-op edit: the live runtime is left untouched. The pool entry's in-memory
    // config is NOT refreshed here, but `list_servers` re-reads config from the
    // DB so the edited fields surface immediately. Reflect the LIVE pool status
    // (connected/tools/version) back to the caller instead of synthesizing a
    // blank "disconnected" status — otherwise the frontend would flicker the
    // server to "disconnected / 0 tools" on a description-only edit even though
    // the connection never dropped.
    log::info!(
        "[{}] update_server: no connection-relevant change, kept live runtime",
        name
    );
    let (live_status, live_tools) = pool::get_entry_info(&saved.name)
        .await
        .unwrap_or_else(|| (
            ServerStatus {
                name: saved.name.clone(),
                connected: false,
                starting: false,
                start_on_demand: saved.start_on_demand.unwrap_or(false),
                tool_count: 0,
                error: None,
                last_connected: None,
                server_version: None,
            },
            vec![],
        ));
    // Apply tool enabled/description configs the same way `list_servers` does,
    // so a no-op edit does not drop the per-tool override display.
    let tools = server_tool_config_service::apply_tool_filters(&saved.name, live_tools)
        .await
        .unwrap_or_default();
    Ok(ServerInfo { config: saved, status: live_status, tools, prompts: Vec::new(), resources: Vec::new() })
}

/// Fields baked into the live MCP client/transport at connect time. Editing any
/// of these requires tearing down and re-establishing the runtime. Everything
/// else in `ServerConfig` (description, owner, visibility, `enabled`, and the
/// tools/prompts/resources per-item overrides) is read-time or access metadata
/// that can be applied without a reconnect.
///
/// Mirrors upstream #1055's `CONNECTION_RELEVANT_CONFIG_FIELDS`. Compares the
/// normalized JSON of just these fields between the existing and incoming
/// config. The request timeout default (60000) is treated as equivalent to
/// "not set" so a stored explicit 60000 and an absent one compare as equal.
fn has_connection_relevant_change(prev: &ServerConfig, next: &ServerConfig) -> bool {
    to_connection_relevant(prev) != to_connection_relevant(next)
}

/// Extract + normalize the connection-relevant subset of a `ServerConfig` as a
/// JSON value for deep comparison. Omits all access/metadata fields.
fn to_connection_relevant(cfg: &ServerConfig) -> serde_json::Value {
    // Serialize the whole config, then drop the non-connection keys. Cheaper and
    // less error-prone than hand-listing every field (stays in sync with model
    // changes), at the cost of serializing a few extra fields that we then strip.
    let mut v = serde_json::to_value(cfg).unwrap_or(serde_json::Value::Null);
    let obj = match v.as_object_mut() {
        Some(o) => o,
        None => return v,
    };
    for key in [
        "id",
        "name",
        "description",
        // visibility/owner/sharedWithUsers are access metadata (desktop keeps
        // them off the Rust model anyway); a change must NOT force a reconnect.
    ] {
        obj.remove(key);
    }
    // NOTE: `enabled` is intentionally KEPT in the comparison: toggling enabled
    // via update_server must connect/disconnect the runtime, same as before.
    // Treat the default request timeout (60000, the dashboard form default) as
    // equivalent to "not set": an explicit 60000 stored via API/file import and
    // an absent timeout resolve to the same effective connect timeout.
    if let Some(options) = obj.get_mut("options").and_then(|o| o.as_object_mut()) {
        if options.get("timeout").and_then(|t| t.as_u64()) == Some(60_000) {
            options.remove("timeout");
            if options.is_empty() {
                obj.remove("options");
            }
        }
    }
    // Drop nulls AND empty containers so "unset/None" and "empty map/vec" compare
    // equal. Without this, a stdio server stored with `env IS NULL` (DB NULL →
    // Rust `None` → serde `null` → dropped) would mismatch the incoming edit's
    // `env = {}` (frontend always sends a map, even when empty → Rust `Some({})`
    // → serde `{}` → kept), causing a spurious reconnect on a description-only
    // edit. Mirrors upstream #1055's `normalizeServerConfigForPersistence`, which
    // normalizes empty records/arrays/options to `undefined` before comparison.
    strip_empty(&mut v);
    v
}

/// Recursively drop `null` values and empty objects/arrays, bottom-up. After
/// stripping a child's own children, an emptied container is itself dropped by
/// its parent, so an `{a: null}` collapses all the way to "absent".
fn strip_empty(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(map) => {
            map.values_mut().for_each(strip_empty);
            map.retain(|_, child| !is_empty_or_null(child));
        }
        serde_json::Value::Array(items) => {
            // Recurse so nested emptiness inside array elements (e.g. an array of
            // objects each reduced to `{}`) collapses too. The array itself is
            // kept here; its parent drops it if it ends up empty.
            items.iter_mut().for_each(strip_empty);
        }
        _ => {}
    }
}

fn is_empty_or_null(v: &serde_json::Value) -> bool {
    v.is_null()
        || matches!(v, serde_json::Value::Object(m) if m.is_empty())
        || matches!(v, serde_json::Value::Array(a) if a.is_empty())
}

#[tauri::command]
pub async fn delete_server(name: String) -> Result<(), String> {
    pool::disconnect_server(&name).await.ok();
    server_service::delete(&name).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn toggle_server(name: String) -> Result<bool, String> {
    mcp_manager::toggle_server(&name)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn reload_server(name: String) -> Result<ServerStatus, String> {
    mcp_manager::reload_server(&name)
        .await
        .map_err(|e| e.to_string())?;
    // reload_server now connects in the background; the "starting" placeholder
    // is inserted early inside connect_server, but to avoid a rare race where
    // get_status runs before the spawned task inserts it, fall back to a
    // synthesized starting status.
    let status = pool::get_status(&name).await.unwrap_or(ServerStatus {
        name: name.clone(),
        connected: false,
        starting: true,
        start_on_demand: false,
        tool_count: 0,
        error: None,
        last_connected: None,
        server_version: None,
    });
    Ok(status)
}

#[tauri::command]
pub async fn reinstall_server(name: String) -> Result<serde_json::Value, String> {
    // Disconnect first
    pool::disconnect_server(&name).await.ok();

    // Get server config to check command type
    let cfg = server_service::get_by_name(&name)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Server '{}' not found", name))?;

    let command = cfg.command.as_ref().map(|c| c.to_lowercase()).unwrap_or_default();
    let mut cleared: Vec<String> = Vec::new();

    // Clear npx cache if the server uses npx
    if command == "npx" {
        if let Some(cache_dir) = runtime_env::npm_cache_dir() {
            if cache_dir.exists() {
                let _ = std::fs::remove_dir_all(&cache_dir);
                cleared.push("npx".to_string());
            }
        }
    }

    // Clear uvx cache if the server uses uvx
    if command == "uvx" {
        if let Some(cache_dir) = runtime_env::uvx_cache_dir() {
            if cache_dir.exists() {
                let _ = std::fs::remove_dir_all(&cache_dir);
                cleared.push("uvx".to_string());
            }
        }
    }

    // Reconnect the server in the background - the npx/uvx re-download may
    // take a while and progress is reported via the `server://install-progress`
    // event, so we must not block the command response here.
    //
    // We allow reinstall even for a disabled server so the user can pull a
    // fresh package version without first enabling it. `connect_server`
    // reconnects regardless of `enabled` (the flag only gates the *initial*
    // startup connect, not an explicit reinstall); the server stays in its
    // prior enabled/disabled state in the DB since we never touch it here.
    // Mark as "just reinstalled" so the post-connect update check records
    // the freshly-downloaded version as installed (and clears the badge)
    // instead of re-notifying about the same version.
    crate::mcp::progress::mark_reinstalled(&cfg.name);
    let cfg_clone = cfg.clone();
    tauri::async_runtime::spawn(async move {
        pool::connect_server(&cfg_clone).await;
    });

    Ok(serde_json::json!({
        "success": true,
        "cleared": cleared
    }))
}

/// Trigger an "update available" check for one server config. Shared by the
/// batch (`check_stdio_updates`) and single (`check_server_update`) entry
/// points so both reuse the connect-time logic in `progress::spawn_update_check`
/// (extract package name → fetch registry latest → compare recorded version →
/// emit `server://update-available`). The check itself runs in the background;
/// this returns immediately.
async fn run_update_check(cfg: &ServerConfig) {
    let running_version = pool::get_entry_info(&cfg.name)
        .await
        .map(|(status, _)| status.server_version)
        .unwrap_or(None);
    progress::spawn_update_check(
        cfg.name.clone(),
        cfg.command.clone().unwrap_or_default(),
        cfg.args.clone().unwrap_or_default(),
        running_version,
    );
}

/// Check all npx/uvx stdio servers for package updates. Fires the same
/// best-effort background check that runs after a successful connect, so the
/// result (and badge) flow back via `server://update-available` exactly as on
/// connect. Returns the number of servers scheduled for a check.
#[tauri::command]
pub async fn check_stdio_updates() -> Result<serde_json::Value, String> {
    let configs = server_service::list_all().await.map_err(|e| e.to_string())?;
    let mut count = 0usize;
    for cfg in configs {
        // Only stdio servers backed by a package manager (npx/uvx) have a
        // registry to check; plain local commands have no package version.
        if cfg.server_type == ServerType::Stdio && progress::is_package_manager(&cfg.command) {
            run_update_check(&cfg).await;
            count += 1;
        }
    }
    Ok(serde_json::json!({ "checked": count }))
}

/// Check a single server for package updates (npx/uvx stdio only). Mirrors the
/// connect-time check; the result returns via `server://update-available`.
#[tauri::command]
pub async fn check_server_update(name: String) -> Result<serde_json::Value, String> {
    let cfg = server_service::get_by_name(&name)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Server '{}' not found", name))?;
    if cfg.server_type != ServerType::Stdio || !progress::is_package_manager(&cfg.command) {
        return Err(format!(
            "Server '{}' is not an npx/uvx stdio server; no package update to check",
            name
        ));
    }
    run_update_check(&cfg).await;
    Ok(serde_json::json!({ "checked": true }))
}

#[tauri::command]
pub async fn clear_cache() -> Result<serde_json::Value, String> {
    let mut results = serde_json::Map::new();

    // Clear npm/npx cache
    if let Some(npm_cache) = runtime_env::npm_cache_dir() {
        if npm_cache.exists() {
            match std::fs::remove_dir_all(&npm_cache) {
                Ok(_) => { results.insert("npx".to_string(), serde_json::json!({"status": "cleared"})); }
                Err(e) => { results.insert("npx".to_string(), serde_json::json!({"status": "error", "message": e.to_string()})); }
            }
        } else {
            results.insert("npx".to_string(), serde_json::json!({"status": "skipped"}));
        }
    } else {
        results.insert("npx".to_string(), serde_json::json!({"status": "skipped"}));
    }

    // Clear uv/uvx cache
    if let Some(uvx_cache) = runtime_env::uvx_cache_dir() {
        if uvx_cache.exists() {
            match std::fs::remove_dir_all(&uvx_cache) {
                Ok(_) => { results.insert("uvx".to_string(), serde_json::json!({"status": "cleared"})); }
                Err(e) => { results.insert("uvx".to_string(), serde_json::json!({"status": "error", "message": e.to_string()})); }
            }
        } else {
            results.insert("uvx".to_string(), serde_json::json!({"status": "skipped"}));
        }
    } else {
        results.insert("uvx".to_string(), serde_json::json!({"status": "skipped"}));
    }

    Ok(serde_json::json!({
        "success": true,
        "results": results
    }))
}
