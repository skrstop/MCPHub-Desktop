use tauri::State;

use crate::commands::auth::SessionState;

use crate::{
    mcp::{pool, progress},
    models::server::{ServerConfig, ServerInfo, ServerPage, ServerStatus, ServerType},
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

/// Paginated server search for the dashboard list. SQL does the name/description
/// substring prefilter (`ORDER BY name` served by the UNIQUE index); runtime-only
/// fields (connection status, live tool names) are merged in Rust for the
/// candidates. The search haystack keeps the previous client-side behavior:
/// name + description + tool names.
///
/// `search` empty = match all. `type_filter`: "custom" (exclude builtin) |
/// "builtin" (only builtin) | "all". `status_filter`: "all" | "online" |
/// "issues" | "disabled" — these depend on runtime state, applied in Rust.
/// `page` is 1-based (matches the dashboard's pagination).
#[tauri::command]
pub async fn search_servers(
    search: String,
    type_filter: String,
    status_filter: String,
    page: u32,
    page_size: u32,
) -> Result<ServerPage, String> {
    let page = page.max(1);
    let page_size = page_size.clamp(1, 200);
    let key = search.trim().to_lowercase();

    // SQL prefilter on name/description (indexed ORDER BY; LIKE on the two
    // persisted searchable fields). Tool-name matching is added below.
    let mut candidates = server_service::search_configs(&search)
        .await
        .map_err(|e| e.to_string())?;
    let mut matched_names: std::collections::HashSet<String> =
        candidates.iter().map(|c| c.name.clone()).collect();

    // 相关度计数（2026-09-06 用户要求：命中查询词元数第一优先，同数保持原生序）。
    // ① name/description：FTS weighted 口径（与 search_configs 主路径同源）；
    //    FTS 降级（空表/Err/零结果走 LIKE）时 counts 为空，用 Rust token 计数补齐，
    //    防止工具名命中反超名称命中（LIKE 候选必含完整 key=全部 token，计数最大）。
    // ② 工具名兜底候选：token 级子串计数并入同一 counts（原来仅整 key 子串匹配，
    //    多词查询时几乎永不命中；token 级提高召回且可参与相关度排序）。
    let mut counts: std::collections::HashMap<String, i64> =
        crate::services::fts_service::search_ref_ids_weighted(
            crate::services::fts_service::FtsTable::Servers,
            &key,
            1000,
        )
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();
    let tokens: Vec<String> = key.split_whitespace().map(|s| s.to_string()).collect();
    if counts.is_empty() && !candidates.is_empty() {
        // FTS 降级路径：Rust 侧逐 token 计数（LIKE 候选均含完整 key → 计数相同
        // → 稳定排序保持 name 序不变；工具名候选计数较小 → 排在名称命中之后）
        for cfg in &candidates {
            let hay = format!(
                "{} {}",
                cfg.name,
                cfg.description.as_deref().unwrap_or("")
            )
            .to_lowercase();
            counts.insert(
                cfg.name.clone(),
                tokens.iter().filter(|t| hay.contains(t.as_str())).count() as i64,
            );
        }
    }

    // Tool-name fallback: tools live only in the runtime pool, so when a query
    // is active, additionally scan the full list's tool names and merge servers
    // not already matched (preserves the previous client-side haystack).
    if !key.is_empty() {
        let all = server_service::list_all().await.map_err(|e| e.to_string())?;
        for cfg in all {
            if matched_names.contains(&cfg.name) {
                continue;
            }
            let tool_hit = pool::get_entry_info(&cfg.name)
                .await
                .map(|(_, tools)| {
                    let hay = tools
                        .iter()
                        .map(|t| t.name.to_lowercase())
                        .collect::<Vec<_>>()
                        .join(" ");
                    tokens.iter().filter(|t| hay.contains(t.as_str())).count()
                })
                .unwrap_or(0);
            if tool_hit > 0 {
                matched_names.insert(cfg.name.clone());
                counts.insert(cfg.name.clone(), tool_hit as i64);
                candidates.push(cfg);
            }
        }
        // 统一相关度排序：命中 token 数降序，稳定排序保持候选进入序
        //（FTS 候选 = weighted+name 序；LIKE 候选 = name 序；工具兜底 = list_all 序）
        candidates.sort_by_key(|c| std::cmp::Reverse(counts.get(&c.name).copied().unwrap_or(0)));
    }

    // Enrich candidates with runtime status + filtered tools (same as list_servers).
    let mut infos: Vec<ServerInfo> = Vec::with_capacity(candidates.len());
    for cfg in candidates {
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
        let tools = server_tool_config_service::apply_tool_filters(&cfg.name, tools)
            .await
            .unwrap_or_default();
        infos.push(ServerInfo { config: cfg, status, tools, prompts: Vec::new(), resources: Vec::new() });
    }

    // Builtin virtual server (RAG): include when the tab asks for it and the
    // query matches its name or tool names (or the query is empty).
    let include_builtin = match type_filter.as_str() {
        "builtin" => true,
        "custom" => false,
        _ => true, // "all"
    };
    if include_builtin {
        if let Some(info) = crate::rag::service::builtin_server_info().await {
            let matches_query = key.is_empty()
                || info.config.name.to_lowercase().contains(&key)
                || info.tools.iter().any(|t| t.name.to_lowercase().contains(&key));
            if matches_query {
                infos.push(info);
            }
        }
    }

    // Type filter: drop builtin rows for the "custom" tab (real DB rows are
    // never builtin, but be explicit), keep only builtin for "builtin".
    let infos: Vec<ServerInfo> = infos
        .into_iter()
        .filter(|i| match type_filter.as_str() {
            "builtin" => i.config.server_type == ServerType::Builtin,
            "custom" => i.config.server_type != ServerType::Builtin,
            _ => true,
        })
        .collect();

    // Status filter (runtime state, applied in Rust before pagination so the
    // total is correct). Semantics mirror getServerFilterCounts in the frontend:
    // online = connected; disabled = enabled=false; issues = the rest.
    let filtered: Vec<ServerInfo> = infos
        .into_iter()
        .filter(|i| match status_filter.as_str() {
            "online" => i.status.connected,
            "disabled" => !i.config.enabled,
            "issues" => !i.status.connected && i.config.enabled,
            _ => true,
        })
        .collect();

    let total = filtered.len() as u64;
    let start = ((page as usize) - 1) * page_size as usize;
    let items = if start >= filtered.len() {
        Vec::new()
    } else {
        filtered[start..(start + page_size as usize).min(filtered.len())].to_vec()
    };
    Ok(ServerPage {
        items,
        total,
        page,
        page_size,
    })
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
pub async fn add_server(
    session: State<'_, SessionState>,
    config: ServerConfig,
) -> Result<ServerInfo, String> {
    // Arbitrary command/args/env injection executes at app privileges on
    // next connect — admin-gated in multi-user mode (skipAuth short-circuits).
    crate::commands::config::require_admin(&session).await?;
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
pub async fn update_server(
    session: State<'_, SessionState>,
    name: String,
    config: ServerConfig,
) -> Result<ServerInfo, String> {
    crate::commands::config::require_admin(&session).await?;
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

    // Smart Routing index (Phase 3): a rename orphans the old name's rows, and
    // any update can change the server-level searchable text (description).
    // Both are background best-effort; the skip check makes no-op updates free.
    if is_renaming {
        let old = name.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = crate::smart_routing::index::remove_server_embeddings(&old).await {
                log::warn!("[smart] remove embeddings for renamed server '{}': {}", old, e);
            }
        });
    }
    {
        let saved_for_index = saved.clone();
        // `needs_reconnect` (computed above) means the live runtime is about to
        // be torn down below — get_entry_info could still see the STALE entry
        // if this spawn raced ahead of the teardown, saving outdated tools.
        // When reconnecting, conservatively drop embeddings here and let the
        // reconnect's update/rebuild path re-save with fresh tools.
        let reconnecting = needs_reconnect;
        tauri::async_runtime::spawn(async move {
            let n = saved_for_index.name.clone();
            let desc = saved_for_index.description.clone();
            if !saved_for_index.enabled || reconnecting {
                let _ = crate::smart_routing::index::remove_server_embeddings(&n).await;
                return;
            }
            if let Some((_, tools)) = crate::mcp::pool::get_entry_info(&n).await {
                if tools.is_empty() {
                    let _ = crate::smart_routing::index::remove_server_embeddings(&n).await;
                } else {
                    let _ = crate::smart_routing::index::save_server_embeddings(&n, desc.as_deref(), &tools).await;
                }
            } else {
                // Entry genuinely absent (server never connected / already
                // torn down). Conservatively drop old embeddings — the next
                // successful connect's update/rebuild path re-saves with
                // fresh tools; keeping stale rows would leak outdated
                // searchable text indefinitely.
                let _ = crate::smart_routing::index::remove_server_embeddings(&n).await;
            }
        });
    }

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
    let mut v = match serde_json::to_value(cfg) {
        Ok(v) => v,
        Err(e) => {
            // Serialization failure is impossible for ServerConfig in practice,
            // but failing closed (force reconnect) is safer than failing open
            // (Null == Null would silently swallow the change).
            log::warn!("[update_server] failed to serialize config for comparison: {}", e);
            return serde_json::Value::String("__serialization_failed__".to_string());
        }
    };
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
    // Normalize boolean flags: the DB stores NOT NULL DEFAULT 0, so map_row
    // always emits `Some(false)` → serde `"perSessionClient": false`, while
    // the frontend payload omits the key entirely when false → `None` →
    // serde `null` → stripped by strip_empty. Without this normalization the
    // `false != absent` mismatch made has_connection_relevant_change return
    // true for EVERY server created via the UI (description-only edits
    // forced reconnects, defeating #1055). Same shape as the timeout rule:
    // `false` and "not set" are semantically identical (changing to `true`
    // still compares unequal and triggers a reconnect).
    for flag in ["perSessionClient", "startOnDemand"] {
        if obj.get(flag).and_then(|f| f.as_bool()) == Some(false) {
            obj.remove(flag);
        }
    }
    if let Some(options) = obj.get_mut("options").and_then(|o| o.as_object_mut()) {
        // Accept both numeric 60000 and string "60000" (JSON file import / API
        // writes may store it as a string) — either form is equivalent to
        // "not set" for comparison purposes.
        let timeout_eq_default = match options.get("timeout") {
            Some(serde_json::Value::Number(n)) => n.as_u64() == Some(60_000),
            Some(serde_json::Value::String(s)) => s.trim().parse::<u64>() == Ok(60_000),
            _ => false,
        };
        if timeout_eq_default {
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
pub async fn delete_server(
    session: State<'_, SessionState>,
    name: String,
) -> Result<(), String> {
    crate::commands::config::require_admin(&session).await?;
    pool::disconnect_server(&name).await.ok();
    server_service::delete(&name).await.map_err(|e| e.to_string())?;
    // Closing TOCTOU guard: between the pre-delete disconnect and the delete
    // commit, the 30s session-rebuild loop could see the still-enabled row
    // and spawn a fresh connect — disconnect again after the delete commits
    // so no live connection outlives a deleted server row.
    pool::disconnect_server(&name).await.ok();
    // Smart Routing index cleanup (best-effort; logged inside).
    if let Err(e) = crate::smart_routing::index::remove_server_embeddings(&name).await {
        log::warn!("[smart] remove embeddings for deleted '{}': {}", name, e);
    }
    Ok(())
}

#[tauri::command]
pub async fn toggle_server(
    session: State<'_, SessionState>,
    name: String,
) -> Result<bool, String> {
    crate::commands::config::require_admin(&session).await?;
    mcp_manager::toggle_server(&name)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn reload_server(
    session: State<'_, SessionState>,
    name: String,
) -> Result<ServerStatus, String> {
    crate::commands::config::require_admin(&session).await?;
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

/// Resolve the npx package spec(s) from a server's args (origin #1182):
/// `-p/--package[=]` values are explicit specs; the first bare token is the
/// package; everything after `-c/--call` is a shell command, not a spec.
fn resolve_npx_package_specs(args: &[String]) -> Vec<String> {
    let mut specs: Vec<String> = Vec::new();
    let mut explicit = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "-p" || arg == "--package" {
            if let Some(v) = args.get(i + 1) {
                specs.push(v.clone());
                explicit = true;
                i += 1;
            }
            i += 1;
            continue;
        }
        if let Some(v) = arg.strip_prefix("--package=") {
            specs.push(v.to_string());
            explicit = true;
            i += 1;
            continue;
        }
        if arg == "-c" || arg == "--call" {
            break;
        }
        if arg.starts_with('-') {
            i += 1;
            continue;
        }
        if !explicit {
            specs.push(arg.clone());
        }
        break;
    }
    specs
}

/// Strip the version from a package spec, keeping a scoped name intact:
/// `cowsay@1.5.0` -> `cowsay`, `@scope/pkg@1.2.3` -> `@scope/pkg`.
fn npx_package_name_from_spec(spec: &str) -> &str {
    match spec.rfind('@') {
        // `separator > 0` so a leading `@scope` marker is kept.
        Some(idx) if idx > 0 => &spec[..idx],
        _ => spec,
    }
}

/// Whether an `_npx` cache entry (identified by the metadata npm leaves in its
/// package.json) installed one of the given specs.
fn npx_entry_matches_spec(manifest: &serde_json::Value, specs: &[String]) -> bool {
    if let Some(packages) = manifest.get("_npx").and_then(|n| n.get("packages")).and_then(|p| p.as_array()) {
        return packages
            .iter()
            .filter_map(|p| p.as_str())
            .any(|p| specs.iter().any(|s| s == p));
    }
    if let Some(deps) = manifest.get("dependencies").and_then(|d| d.as_object()) {
        let names: std::collections::HashSet<&str> =
            specs.iter().map(|s| npx_package_name_from_spec(s)).collect();
        return deps.keys().any(|k| names.contains(k.as_str()));
    }
    false
}

/// Remove only the `_npx` cache entries belonging to the given package specs.
/// Returns Ok(true) when entries were cleared, Ok(false) when no spec could be
/// resolved (cache deliberately left untouched). Unreadable entries are skipped:
/// this runs to refresh one server, an unreadable neighbour is not ours to delete.
///
/// The cache scanned is the SERVER-SPECIFIC npm cache that `env_overrides`
/// actually points npx at (`npm_config_cache = <app_local>/npm-cache-{name}`) —
/// the shared `runtimes/_npx` dir is never written by managed npx spawns, so
/// scanning it made reinstalls a no-op (package kept running the old version).
async fn clear_npx_cache_for_specs(name: &str, args: &[String]) -> Result<bool, std::io::Error> {
    let specs = resolve_npx_package_specs(args);
    if specs.is_empty() {
        return Ok(false);
    }
    // env_overrides writes `npm-cache-{server}` inside the same app-local
    // dir app_local_dir resolves (runtime_env); reuse its base + `_npx`.
    let Some(cache_dir) = runtime_env::npm_server_cache_dir(name).map(|d| d.join("_npx")) else {
        return Ok(false);
    };
    let entries = match std::fs::read_dir(&cache_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let mut removed = false;
    for entry in entries.flatten() {
        let entry_dir = entry.path();
        let manifest: serde_json::Value = match std::fs::read_to_string(entry_dir.join("package.json"))
        {
            Ok(s) => match serde_json::from_str(&s) {
                Ok(v) => v,
                Err(_) => continue,
            },
            Err(_) => continue,
        };
        if !npx_entry_matches_spec(&manifest, &specs) {
            continue;
        }
        // spawn_blocking: remove_dir_all can block for seconds on large
        // package caches; must not stall the async executor.
        if let Err(e) = tauri::async_runtime::spawn_blocking(move || std::fs::remove_dir_all(&entry_dir)).await.unwrap_or(Err(std::io::Error::other("join failed"))) {
            return Err(e);
        }
        removed = true;
    }
    Ok(removed)
}

#[tauri::command]
pub async fn reinstall_server(
    session: State<'_, SessionState>,
    name: String,
) -> Result<serde_json::Value, String> {
    crate::commands::config::require_admin(&session).await?;
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
        // Scope the clear to this server's package (origin #1182): deleting the
        // whole `_npx` directory would discard every other npx server's install
        // as well. Entries are identified from the metadata npm leaves in each
        // entry's package.json; unreadable/unmatched entries are left alone.
        match clear_npx_cache_for_specs(&name, cfg.args.as_deref().unwrap_or(&[])).await {
            Ok(true) => cleared.push("npx".to_string()),
            Ok(false) => {
                log::warn!(
                    "[reinstall] Could not identify the npx package from args of '{}'; cache left untouched",
                    name
                );
            }
            Err(e) => log::warn!("[reinstall] Failed to clear scoped npx cache for '{}': {}", name, e),
        }
    }

    // Clear uvx cache if the server uses uvx
    if command == "uvx" {
        // uvx caches are per-server: `env_overrides` sets UV_CACHE_DIR to
        // `uv-cache-{server}` for each uvx server, so clearing THIS server's
        // cache dir forces a fresh download without touching other uvx
        // servers' caches (per-package scoping, same spirit as origin #1182
        // for npx). The previous code deleted the shared `runtimes/uv-cache`
        // dir — which neither holds this server's real cache nor is shared
        // per-server — so reinstalls silently kept serving stale packages.
        if let Some(cache) = runtime_env::uvx_server_cache_dir(&name) {
            if cache.exists() {
                let dir = cache.clone();
                let _ = tauri::async_runtime::spawn_blocking(move || std::fs::remove_dir_all(&dir))
                    .await;
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
pub async fn check_stdio_updates(
    session: tauri::State<'_, crate::commands::auth::SessionState>,
) -> Result<serde_json::Value, String> {
    // Spawns outbound registry checks for every npx/uvx server — bounded to
    // admin (review round 8, 2026-10-04).
    crate::commands::config::require_admin(&session).await?;
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
pub async fn check_server_update(
    session: tauri::State<'_, crate::commands::auth::SessionState>,
    name: String,
) -> Result<serde_json::Value, String> {
    crate::commands::config::require_admin(&session).await?;
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
pub async fn clear_cache(session: State<'_, SessionState>) -> Result<serde_json::Value, String> {
    crate::commands::config::require_admin(&session).await?;
    let mut results = serde_json::Map::new();

    // Clear npm/npx cache: the per-server cache dirs env_overrides actually
    // points npx at (`npm-cache-{server}`, whose `_npx` holds the installs).
    // clear_cache is a global clear — wipe EVERY `npm-cache-*` sibling.
    {
        let mc_root = runtime_env::npm_server_cache_dir("x").map(|p| {
            let mut pb = p;
            pb.pop(); // strip the "npm-cache-x" name → the mcphub-desktop dir
            pb
        });
        match mc_root {
            Some(root) if root.exists() => {
                let mut removed_any = false;
                let mut had_error: Option<String> = None;
                if let Ok(entries) = std::fs::read_dir(&root) {
                    for entry in entries.flatten() {
                        let fname = entry.file_name();
                        let fname = fname.to_string_lossy();
                        if !fname.starts_with("npm-cache-") {
                            continue;
                        }
                        let dir = entry.path();
                        match tauri::async_runtime::spawn_blocking(move || std::fs::remove_dir_all(&dir)).await
                        {
                            Ok(Ok(_)) => removed_any = true,
                            Ok(Err(e)) => had_error = Some(e.to_string()),
                            Err(e) => had_error = Some(e.to_string()),
                        }
                    }
                }
                match had_error {
                    Some(e) => { results.insert("npx".to_string(), serde_json::json!({"status": "error", "message": e})); }
                    None if removed_any => { results.insert("npx".to_string(), serde_json::json!({"status": "cleared"})); }
                    None => { results.insert("npx".to_string(), serde_json::json!({"status": "skipped"})); }
                }
            }
            _ => { results.insert("npx".to_string(), serde_json::json!({"status": "skipped"})); }
        }
    }

    // Clear uv/uvx caches. uvx servers use per-server dirs (`uv-cache-{name}`
    // / `uv-tools-{name}` under the app cache dir, matching the UV_CACHE_DIR /
    // UV_TOOL_DIR env overrides), NOT the legacy shared runtimes/uv-cache.
    if let Some(base) = runtime_env::uvx_server_cache_dir("probe").and_then(|p| p.parent().map(|p| p.to_path_buf())) {
        let mut cleared_any = false;
        let mut had_error: Option<String> = None;
        if let Ok(entries) = std::fs::read_dir(&base) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !(name.starts_with("uv-cache-") || name.starts_with("uv-tools-")) {
                    continue;
                }
                let dir = entry.path();
                if !dir.is_dir() {
                    continue;
                }
                cleared_any = true;
                match tauri::async_runtime::spawn_blocking(move || std::fs::remove_dir_all(&dir)).await
                {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => had_error = Some(e.to_string()),
                    Err(e) => had_error = Some(e.to_string()),
                }
            }
        }
        let status = match &had_error {
            Some(_) => "error",
            None if cleared_any => "cleared",
            None => "skipped",
        };
        let mut v = serde_json::json!({"status": status});
        if let Some(e) = had_error {
            v["message"] = serde_json::json!(e);
        }
        results.insert("uvx".to_string(), v);
    } else {
        results.insert("uvx".to_string(), serde_json::json!({"status": "skipped"}));
    }

    Ok(serde_json::json!({
        "success": true,
        "results": results
    }))
}

#[cfg(test)]
mod connection_relevance_tests {
    use super::*;
    use serde_json::json;

    fn cfg_from(extra: serde_json::Value) -> ServerConfig {
        let mut v = json!({
            "name": "s1",
            "serverType": "stdio",
            "command": "npx",
            "args": ["-y", "demo"],
        });
        if let (Some(base), Some(obj)) = (v.as_object_mut(), extra.as_object()) {
            for (k, val) in obj {
                base.insert(k.clone(), val.clone());
            }
        }
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn boolean_false_and_absent_are_equivalent() {
        // DB-backed configs always carry explicit `false` (NOT NULL DEFAULT 0)
        // while UI payloads omit the key — both must compare equal so a
        // description-only edit does not force a reconnect.
        let prev = cfg_from(json!({"perSessionClient": false, "startOnDemand": false}));
        let next = cfg_from(json!({}));
        assert!(!has_connection_relevant_change(&prev, &next));
        assert!(!has_connection_relevant_change(&next, &prev));

        // Flipping to true is still connection-relevant.
        let next_on = cfg_from(json!({"perSessionClient": true}));
        assert!(has_connection_relevant_change(&prev, &next_on));
    }

    #[test]
    fn default_timeout_is_equivalent_to_absent() {
        // Note: ServerOptions.timeout is a typed u64, so string-form "60000"
        // can never deserialize into ServerConfig; the string branch in
        // to_connection_relevant is purely defensive (JSON Value level).
        let numeric = cfg_from(json!({"options": {"timeout": 60000}}));
        let none = cfg_from(json!({}));
        assert!(!has_connection_relevant_change(&numeric, &none));

        let changed = cfg_from(json!({"options": {"timeout": 30000}}));
        assert!(has_connection_relevant_change(&numeric, &changed));
    }

    #[test]
    fn description_only_edit_does_not_reconnect() {
        let prev = cfg_from(json!({"description": "old"}));
        let next = cfg_from(json!({"description": "new"}));
        assert!(!has_connection_relevant_change(&prev, &next));

        let cmd_changed = cfg_from(json!({"description": "old", "command": "uvx"}));
        assert!(has_connection_relevant_change(&prev, &cmd_changed));
    }
}
