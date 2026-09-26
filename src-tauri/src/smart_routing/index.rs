//! Indexing: toolset hash (origin-compatible scrypt), skip-check, and the
//! save/remove primitives the lifecycle hooks call.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use super::models;
use super::store::{SmartRow, SmartToolStore};
use crate::models::server::Tool;

/// Same key as origin (`vectorSearchService.ts`) — hash inputs are computed
/// over the same normalized shape, so the constant identifies the cache
/// format, not the embedding backend.
const TOOLSET_HASH_KEY: &[u8] = b"mcphub:toolset-embedding-cache:v1";

/// stableHashSerialize (origin parity): deterministic JSON with object keys
/// sorted (locale-independent byte sort is close enough — origin uses
/// localeCompare, but its inputs are ASCII identifiers, where both agree).
pub fn stable_hash_serialize(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => serde_json::to_string(s).unwrap_or_default(),
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(stable_hash_serialize).collect();
            format!("[{}]", inner.join(","))
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let inner: Vec<String> = keys
                .iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap_or_default(),
                        stable_hash_serialize(&map[*k])
                    )
                })
                .collect();
            format!("{{{}}}", inner.join(","))
        }
    }
}

/// toolSetHash (origin `buildToolSetHash` parity, sans description override):
/// - tools sorted by name;
/// - normalized shape `{name, inputSchema, description: null}` — raw upstream
///   descriptions are EXCLUDED (origin #1198: dynamic content in upstream
///   descriptions must not invalidate the cache);
/// - scrypt(N=2048, r=8, p=1, 32 bytes) → hex.
pub fn build_toolset_hash(tools: &[Tool]) -> String {
    let mut normalized: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "inputSchema": t.input_schema,
                "description": Value::Null,
            })
        })
        .collect();
    normalized.sort_by(|a, b| {
        a.get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase()
            .cmp(&b.get("name").and_then(|v| v.as_str()).unwrap_or("").to_lowercase())
    });
    scrypt_hex(&stable_hash_serialize(&Value::Array(normalized)))
}

fn scrypt_hex(input: &str) -> String {
    // N=2048 → log_n = 11. Params tuned down (~2ms) like origin: this runs on
    // every tool-set refresh only.
    let params = scrypt::Params::new(11, 8, 1, 32).expect("static scrypt params are valid");
    let mut out = [0u8; 32];
    scrypt::scrypt(input.as_bytes(), TOOLSET_HASH_KEY, &params, &mut out)
        .expect("scrypt with valid params cannot fail");
    out.iter().map(|b| format!("{:02x}", b)).collect()
}

/// The text embedded for one tool (origin `searchableText` composition):
/// name + description + schema top-level keys (except type/properties) +
/// property names.
pub fn tool_searchable_text(tool: &Tool) -> String {
    let mut parts: Vec<String> = vec![tool.name.clone()];
    if let Some(d) = &tool.description {
        if !d.trim().is_empty() {
            parts.push(d.clone());
        }
    }
    if let Some(obj) = tool.input_schema.as_object() {
        for key in obj.keys() {
            if key != "type" && key != "properties" {
                parts.push(key.clone());
            }
        }
        if let Some(props) = obj.get("properties").and_then(|p| p.as_object()) {
            for key in props.keys() {
                parts.push(key.clone());
            }
        }
    }
    parts.join(" ")
}

/// Server-level searchable text (origin `buildServerSearchableText`).
pub fn server_searchable_text(server_name: &str, description: Option<&str>) -> String {
    [Some(server_name.to_string()), description.map(str::to_string)]
        .into_iter()
        .flatten()
        .filter(|s| !s.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// True when the Smart Routing consumer wants indexing (its own enable flag).
async fn smart_enabled() -> bool {
    models::get_settings().await.enabled
}

/// Lifecycle hook: a server connected (or its tool list changed). Indexes the
/// server's tools + server row in the background; errors are logged, never
/// propagated (indexing must not break server connectivity).
pub fn on_server_connected(server_name: String, tools: Vec<Tool>, description: Option<String>) {
    if tools.is_empty() {
        // origin: empty tool list removes any previous embeddings.
        tokio::spawn(async move {
            if let Err(e) = remove_server_embeddings(&server_name).await {
                log::warn!("[smart] remove embeddings for '{}': {}", server_name, e);
            }
        });
        return;
    }
    tokio::spawn(async move {
        if let Err(e) = save_server_embeddings(&server_name, description.as_deref(), &tools).await {
            log::warn!("[smart] index server '{}': {}", server_name, e);
        }
    });
}

/// Lifecycle hook: server config updated. Re-runs the save — the skip check
/// makes it a no-op when the tool set / description / model are unchanged.
pub fn on_server_updated(server_name: String, tools: Vec<Tool>, description: Option<String>) {
    on_server_connected(server_name, tools, description);
}

/// Lifecycle hook: server deleted / renamed away / disabled — drop its rows.
pub async fn remove_server_embeddings(server_name: &str) -> Result<()> {
    if !smart_enabled().await {
        return Ok(());
    }
    let embed_dim = crate::mv::embed_dim().unwrap_or(0);
    let store = open_store(embed_dim).await?;
    store.delete_server_rows(server_name).await
}

/// Index one server: skip-check → embed tools + server row → replace rows.
/// Called from background hooks only (embedding a big tool list takes
/// seconds; never on a request path).
pub async fn save_server_embeddings(
    server_name: &str,
    description: Option<&str>,
    tools: &[Tool],
) -> Result<()> {
    if !smart_enabled().await {
        return Ok(());
    }
    if !crate::mv::is_running() {
        return Err(anyhow!("mv runtime not running (smart routing enabled but model not loaded)"));
    }
    let settings = models::get_settings().await;
    let _ = settings; // weights only affect search, not the index

    // Apply the user's per-tool enable/description-override config first:
    // disabled tools are not indexed at all; overridden descriptions are what
    // gets embedded (and would matter for the hash if we hashed descriptions).
    let tools = crate::services::server_tool_config_service::apply_tool_filters(
        server_name,
        tools.to_vec(),
    )
    .await?;
    let tools = tools
        .into_iter()
        .filter(|t| t.enabled)
        .collect::<Vec<_>>();

    let expected_hash = build_toolset_hash(&tools);
    let server_text = server_searchable_text(server_name, description);
    let model = crate::mv::active_model()
        .await
        .ok_or_else(|| anyhow!("mv model not loaded"))?;
    let embed_dim = crate::mv::embed_dim().unwrap_or(0);

    let store = open_store(embed_dim).await?;
    if store.needs_full_reindex() {
        log::info!("[smart] table recreated (model dim change) — rebuilding index");
    }

    // ── Skip check (origin parity): count + exact content ids + hash + server row.
    let expected_ids: Vec<String> = tools
        .iter()
        .map(|t| format!("{}:{}", server_name, t.name))
        .collect();
    let mut skip = false;
    if !store.needs_full_reindex() {
        match store.identities(server_name, &model).await {
            Ok(existing) => {
                let count_ok = existing.len() == tools.len();
                let ids_ok = {
                    let mut a: Vec<String> = existing.iter().map(|(id, _)| id.clone()).collect();
                    a.sort();
                    let mut b = expected_ids.clone();
                    b.sort();
                    a == b
                };
                let hash_ok =
                    !existing.is_empty() && existing.iter().all(|(_, h)| *h == expected_hash);
                let server_ok = matches!(
                    store.server_row(server_name).await,
                    Ok(Some((m, t))) if m == model && t == server_text
                );
                skip = count_ok && ids_ok && hash_ok && server_ok;
            }
            Err(e) => {
                log::warn!(
                    "[smart] skip check failed for '{}', full sync: {}",
                    server_name,
                    e
                );
            }
        }
    }
    if skip {
        log::info!(
            "[smart] '{}' tool set up-to-date (model={}, hash={}…), skipping",
            server_name,
            model,
            &expected_hash[..12.min(expected_hash.len())]
        );
        return Ok(());
    }
    log::info!(
        "[smart] indexing '{}' ({} tools, model={}, hash={}…)",
        server_name,
        tools.len(),
        model,
        &expected_hash[..12.min(expected_hash.len())]
    );

    // ── Phase 1: embed everything under one mv lock hold (sync closure).
    let texts: Vec<String> = tools.iter().map(tool_searchable_text).collect();
    let texts_for_embed = texts.clone();
    let embeddings = crate::mv::with_model(move |model| -> Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(texts_for_embed.len());
        for text in &texts_for_embed {
            out.push(model.embed(text)?);
        }
        Ok(out)
    })
    .await
    .map_err(|e| anyhow!("embed tools for '{}': {}", server_name, e))?;
    let server_text_for_embed = server_text.clone();
    let server_embedding = crate::mv::with_model(move |model| model.embed(&server_text_for_embed))
        .await
        .map_err(|e| anyhow!("embed server row for '{}': {}", server_name, e))?;

    // ── Phase 2: persist (delete + insert under the store's connection).
    let content_ids: Vec<String> = tools
        .iter()
        .map(|t| format!("{}:{}", server_name, t.name))
        .collect();
    let metadatas: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "serverName": server_name,
                "toolName": t.name,
                "description": t.description,
                "inputSchema": t.input_schema,
            })
        })
        .collect();
    let server_meta = json!({ "serverName": server_name });
    let mut rows: Vec<SmartRow> = Vec::with_capacity(tools.len() + 1);
    for i in 0..tools.len() {
        rows.push(SmartRow {
            content_type: "tool",
            content_id: &content_ids[i],
            server_name,
            tool_name: &tools[i].name,
            text_content: &texts[i],
            model: &model,
            tool_set_hash: &expected_hash,
            metadata: &metadatas[i],
            embedding: &embeddings[i],
        });
    }
    // Borrow dance: content_id/text borrow `server_name`/`server_text`.
    let server_content_id = server_name.to_string();
    rows.push(SmartRow {
        content_type: "server",
        content_id: &server_content_id,
        server_name,
        tool_name: "",
        text_content: &server_text,
        model: &model,
        tool_set_hash: "",
        metadata: &server_meta,
        embedding: &server_embedding,
    });
    store.replace_server(server_name, &rows).await?;
    Ok(())
}

/// Re-index every connected server (boot restore when SR is on / model swap /
/// manual reindex). The per-server skip check makes repeated runs cheap.
pub async fn reindex_all() -> Result<usize> {
    if !smart_enabled().await {
        return Ok(0);
    }
    if !crate::mv::is_running() {
        return Err(anyhow!("mv runtime not running"));
    }
    let servers = crate::services::server_service::list_all().await?;
    let mut indexed = 0usize;
    for cfg in servers {
        if !cfg.enabled {
            // Disabled servers contribute no rows; clean any stale ones.
            let _ = remove_server_embeddings(&cfg.name).await;
            continue;
        }
        let tools = match crate::mcp::pool::get_entry_info(&cfg.name).await {
            // Sleeping on-demand servers keep their cached tools discoverable
            // (parity with the non-smart tools/list: connected ||
            // start_on_demand) — a hit will cold-start them via call_tool.
            Some((status, tools)) if status.connected || status.start_on_demand => tools,
            _ => continue, // not connected and not on-demand — skip
        };
        if tools.is_empty() {
            let _ = remove_server_embeddings(&cfg.name).await;
            continue;
        }
        match save_server_embeddings(&cfg.name, cfg.description.as_deref(), &tools).await {
            Ok(()) => indexed += 1,
            Err(e) => {
                log::warn!("[smart] reindex '{}': {}", cfg.name, e);
                crate::services::app_logger::log_to_db("warn", &format!("[smart] reindex '{}' failed: {e:#}", cfg.name));
            }
        }
    }
    // Builtin "mcphub-desktop" server: virtual (no pool entry / DB row) —
    // without indexing it here, smart_route_search can never discover its tools
    // and resolve_tool would fail for them. Source of truth =
    // rag::service::builtin_server_tools() — the SAME assembly every listing
    // uses — minus the smart meta tools themselves (they are the SEARCH UI,
    // not search targets; indexing them would let smart_route_search return
    // itself/siblings as results). New builtin tools added later are picked
    // up automatically — no per-tool list to maintain here.
    let builtin_desc = Some("Built-in capabilities (RAG, Smart Routing, prompts, resources)".to_string());
    let builtin_tools = crate::rag::service::builtin_server_tools()
        .await
        .into_iter()
        .filter(|t| !crate::smart_routing::meta::is_meta_tool(&t.name))
        .collect::<Vec<_>>();
    if !builtin_tools.is_empty() {
        match save_server_embeddings(crate::rag::service::BUILTIN_SERVER_NAME, builtin_desc.as_deref(), &builtin_tools).await {
            Ok(()) => {
                indexed += 1;
                crate::services::app_logger::log_to_db("info", &format!("[smart] reindex builtin: {} tools", builtin_tools.len()));
            }
            Err(e) => {
                log::warn!("[smart] reindex builtin: {}", e);
                crate::services::app_logger::log_to_db("warn", &format!("[smart] reindex builtin failed: {e:#}"));
            }
        }
    } else {
        let _ = remove_server_embeddings(crate::rag::service::BUILTIN_SERVER_NAME).await;
        crate::services::app_logger::log_to_db("warn", "[smart] reindex builtin: NO tools to index (rag disabled or filtered empty)");
    }
    Ok(indexed)
}

/// Model-reload hook: after mv loads a (possibly different) model, purge rows
/// embedded by ANY OTHER model (same-dim swaps don't recreate the table, so
/// stale rows would silently mix into searches) and re-index the connected
/// servers under the new model. Fire-and-forget; failures logged.
pub async fn on_model_reloaded(model: String) {
    if !smart_enabled().await {
        return;
    }
    let embed_dim = crate::mv::embed_dim().unwrap_or(0);
    let Ok(store) = open_store(embed_dim).await else {
        return;
    };
    // Drop rows from other models.
    let stale_filter = format!("model != '{}'", model.replace('\'', "''"));
    if let Err(e) = store.delete_where(&stale_filter).await {
        log::warn!("[smart] purge stale rows after model reload: {}", e);
        return;
    }
    match reindex_all().await {
        Ok(n) if n > 0 => log::info!("[smart] re-indexed {} server(s) after model reload", n),
        Ok(_) => {}
        Err(e) => log::warn!("[smart] reindex after model reload: {}", e),
    }
}

async fn open_store(embed_dim: usize) -> Result<SmartToolStore> {
    let conn = crate::mv::connection_async().await?;
    SmartToolStore::open_with_conn(conn, embed_dim).await
}
