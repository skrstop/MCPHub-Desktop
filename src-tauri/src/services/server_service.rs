use crate::{
    db,
    models::server::{OpenApiConfig, ProxychainsConfig, ServerConfig, ServerOptions, ServerType},
};
use anyhow::{anyhow, Result};
use sqlx::Row;
use std::collections::HashMap;
use uuid::Uuid;

fn decode_server_type(s: &str) -> ServerType {
    match s {
        "sse" => ServerType::Sse,
        "streamable-http" => ServerType::StreamableHttp,
        "openapi" => ServerType::Openapi,
        _ => ServerType::Stdio,
    }
}

fn encode_server_type(t: &ServerType) -> &'static str {
    match t {
        ServerType::Stdio => "stdio",
        ServerType::Sse => "sse",
        ServerType::StreamableHttp => "streamable-http",
        ServerType::Openapi => "openapi",
        // Builtin servers are virtual (never persisted), so this is never
        // written to the DB - but the match must be exhaustive.
        ServerType::Builtin => "builtin",
    }
}

pub async fn list_all_enabled() -> Result<Vec<ServerConfig>> {
    let rows = sqlx::query(
        "SELECT id, name, server_type, description, command, args, env, url, headers, options, openapi, per_session_client, start_on_demand, idle_timeout_ms, proxy, enable_keep_alive, keep_alive_interval, passthrough_headers, enabled
         FROM servers WHERE enabled = 1",
    )
    .fetch_all(db::pool())
    .await?;
    rows.into_iter().map(map_row).collect()
}

pub async fn list_all() -> Result<Vec<ServerConfig>> {
    let rows = sqlx::query(
        "SELECT id, name, server_type, description, command, args, env, url, headers, options, openapi, per_session_client, start_on_demand, idle_timeout_ms, proxy, enable_keep_alive, keep_alive_interval, passthrough_headers, enabled
         FROM servers ORDER BY name",
    )
    .fetch_all(db::pool())
    .await?;
    rows.into_iter().map(map_row).collect()
}

/// SQL-level search over the `servers` table: case-insensitive substring on
/// name OR description (empty key = all rows), `ORDER BY name`. Serves the
/// `search_servers` command; runtime-only fields (connection status, live tool
/// list) are merged by the caller for the returned candidates only.
pub async fn search_configs(search_key: &str) -> Result<Vec<ServerConfig>> {
    let key = search_key.trim();
    if key.is_empty() {
        let rows = sqlx::query(
            "SELECT id, name, server_type, description, command, args, env, url, headers, options, openapi, per_session_client, start_on_demand, idle_timeout_ms, proxy, enable_keep_alive, keep_alive_interval, passthrough_headers, enabled \
             FROM servers ORDER BY name",
        )
        .fetch_all(db::pool())
        .await?;
        return rows.into_iter().map(map_row).collect();
    }

    // FTS5 路径（中/英/拼音分词）；命中词数优先 + 同数按 name 序；空表/Err/零结果降级 LIKE（§4.4）
    match crate::services::fts_service::search_ref_ids_weighted(
        crate::services::fts_service::FtsTable::Servers,
        key,
        200,
    )
    .await
    {
        Ok(weighted) if !weighted.is_empty() => {
            // 回表（原生 name 序）→ 稳定排序按命中词数降序：相关度第一优先，
            // 同相关度保持 name 序（2026-09-06 用户要求）。servers 总量小，全量读代价可忽略
            let counts: std::collections::HashMap<String, i64> =
                weighted.into_iter().collect();
            let all = sqlx::query(
                "SELECT id, name, server_type, description, command, args, env, url, headers, options, openapi, per_session_client, start_on_demand, idle_timeout_ms, proxy, enable_keep_alive, keep_alive_interval, passthrough_headers, enabled \
                 FROM servers ORDER BY name",
            )
            .fetch_all(db::pool())
            .await?;
            let mut configs: Vec<ServerConfig> =
                all.into_iter().map(map_row).collect::<Result<_>>()?;
            configs.retain(|c| counts.contains_key(&c.name));
            configs.sort_by_key(|c| std::cmp::Reverse(counts.get(&c.name).copied().unwrap_or(0)));
            Ok(configs)
        }
        Ok(_) => {
            // 零结果/空表均降级 LIKE：LIKE 子串语义可补 FTS 词前缀盲区
            //（如 CJK 内部子串「据」不在分词 token 头部，FTS 查不到）
            like_search(key).await
        }
        Err(e) => {
            log::warn!("[fts] search servers failed, fallback to LIKE: {e}");
            like_search(key).await
        }
    }
}

/// 原 LIKE 搜索路径（降级兜底）
async fn like_search(key: &str) -> Result<Vec<ServerConfig>> {
    let pattern = format!(
        "%{}%",
        key.to_lowercase()
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    let rows = sqlx::query(
        "SELECT id, name, server_type, description, command, args, env, url, headers, options, openapi, per_session_client, start_on_demand, idle_timeout_ms, proxy, enable_keep_alive, keep_alive_interval, passthrough_headers, enabled \
         FROM servers \
         WHERE LOWER(name) LIKE ? ESCAPE '\\' OR LOWER(COALESCE(description, '')) LIKE ? ESCAPE '\\' \
         ORDER BY name",
    )
    .bind(&pattern)
    .bind(&pattern)
    .fetch_all(db::pool())
    .await?;
    rows.into_iter().map(map_row).collect()
}

pub async fn get_by_name(name: &str) -> Result<Option<ServerConfig>> {
    let row = sqlx::query(
        "SELECT id, name, server_type, description, command, args, env, url, headers, options, openapi, per_session_client, start_on_demand, idle_timeout_ms, proxy, enable_keep_alive, keep_alive_interval, passthrough_headers, enabled
         FROM servers WHERE name = ?",
    )
    .bind(name)
    .fetch_optional(db::pool())
    .await?;
    row.map(map_row).transpose()
}

/// Per-session client isolation and on-demand spawning are mutually exclusive:
/// the former creates a dedicated upstream client per session, the latter
/// keeps a single shared client that sleeps. Reject the combination up front.
/// Server names are used verbatim as cache/tool directory suffixes
/// (`npm-cache-{name}`, `uv-cache-{name}`, `uv-tools-{name}`) and fed to
/// `remove_dir_all` on reinstall — a name containing `/`/`\`/`..` escapes the
/// cache root (directory creation outside it, arbitrary-directory deletion).
fn validate_server_name(name: &str) -> Result<()> {
    let ok = !name.trim().is_empty()
        && name.len() <= 128
        && !name.contains("..")
        && !name.contains('/')
        && !name.contains('\\')
        && !name.starts_with('$')
        && !name.chars().any(|c| c.is_control());
    if ok {
        Ok(())
    } else {
        Err(anyhow!(
            "server name '{}' is invalid: must not be empty, contain path separators, '..', '$' prefix (reserved for $smart scopes), or control characters",
            name
        ))
    }
}

fn validate_combination(cfg: &ServerConfig) -> Result<()> {
    if cfg.per_session_client.unwrap_or(false) && cfg.start_on_demand.unwrap_or(false) {
        return Err(anyhow!(
            "perSessionClient and startOnDemand cannot both be enabled on the same server"
        ));
    }
    Ok(())
}

/// servers 的 FTS 可搜索文本（P3：多字段单空格拼接）：name + description
fn server_fts_text(name: &str, description: Option<&str>) -> String {
    match description {
        Some(d) if !d.is_empty() => format!("{name} {d}"),
        _ => name.to_string(),
    }
}

pub async fn create(cfg: &ServerConfig) -> Result<ServerConfig> {
    // The name "RAG" is reserved for the builtin RAG server (see
    // rag::service::BUILTIN_SERVER_NAME). Reject custom servers using it so a
    // group referencing "RAG" always means the builtin, never a custom server.
    if cfg.name.eq_ignore_ascii_case(crate::rag::service::BUILTIN_SERVER_NAME) {
        return Err(anyhow!("server name '{}' is reserved for the builtin server", cfg.name));
    }
    validate_server_name(&cfg.name)?;
    validate_combination(cfg)?;
    let id = Uuid::new_v4().to_string();
    let args = cfg.args.as_ref().map(serde_json::to_string).transpose()?;
    let env = cfg.env.as_ref().map(serde_json::to_string).transpose()?;
    let headers = cfg.headers.as_ref().map(serde_json::to_string).transpose()?;
    let options = cfg.options.as_ref().map(serde_json::to_string).transpose()?;
    let openapi = cfg.openapi.as_ref().map(serde_json::to_string).transpose()?;
    let proxy = cfg.proxy.as_ref().map(serde_json::to_string).transpose()?;
    let enable_keep_alive = cfg.enable_keep_alive.map(|b| b as i64);
    let keep_alive_interval = cfg.keep_alive_interval.map(|v| v as i64);
    let passthrough_headers = cfg
        .passthrough_headers
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let server_type = encode_server_type(&cfg.server_type);
    let enabled = cfg.enabled as i64;
    let per_session_client = cfg.per_session_client.unwrap_or(false) as i64;
    let start_on_demand = cfg.start_on_demand.unwrap_or(false) as i64;
    let idle_timeout_ms = cfg.idle_timeout_ms.unwrap_or(0) as i64;

    let fts_text = server_fts_text(&cfg.name, cfg.description.as_deref());

    let mut tx = db::pool().begin().await?;
    sqlx::query(
        "INSERT INTO servers (id, name, server_type, description, command, args, env, url, headers, options, openapi, per_session_client, start_on_demand, idle_timeout_ms, proxy, enable_keep_alive, keep_alive_interval, passthrough_headers, enabled)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&cfg.name)
    .bind(server_type)
    .bind(&cfg.description)
    .bind(&cfg.command)
    .bind(&args)
    .bind(&env)
    .bind(&cfg.url)
    .bind(&headers)
    .bind(&options)
    .bind(&openapi)
    .bind(per_session_client)
    .bind(start_on_demand)
    .bind(idle_timeout_ms)
    .bind(&proxy)
    .bind(enable_keep_alive)
    .bind(keep_alive_interval)
    .bind(&passthrough_headers)
    .bind(enabled)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        let msg = e.to_string();
        if msg.contains("UNIQUE constraint failed") {
            anyhow!("A server with the name '{}' already exists", cfg.name)
        } else {
            anyhow!(msg)
        }
    })?;

    // FTS 同步（§4.3 铁律：同事务）
    crate::services::fts_service::sync_upsert_tx(
        &mut tx,
        crate::services::fts_service::FtsTable::Servers,
        &cfg.name,
        &fts_text,
    )
    .await?;
    tx.commit().await?;

    let out = get_by_name(&cfg.name).await?.ok_or_else(|| anyhow!("Insert failed"));
    crate::services::subscription_hub::notify_tools_list_changed().await;
    // Prompts/resources exposure changes with server create/rename/delete/toggle
    // too — without the trio fan-out connected clients cache stale lists (R119).
    crate::services::subscription_hub::notify_prompts_list_changed().await;
    crate::services::subscription_hub::notify_resources_list_changed().await;
    out
}

pub async fn update(name: &str, cfg: &ServerConfig) -> Result<ServerConfig> {
    // Reject renaming TO the reserved builtin name (renaming FROM it is moot -
    // the builtin has no DB row to update).
    if cfg.name.eq_ignore_ascii_case(crate::rag::service::BUILTIN_SERVER_NAME) {
        return Err(anyhow!("server name '{}' is reserved for the builtin server", cfg.name));
    }
    validate_server_name(&cfg.name)?;
    validate_combination(cfg)?;
    let args = cfg.args.as_ref().map(serde_json::to_string).transpose()?;
    let env = cfg.env.as_ref().map(serde_json::to_string).transpose()?;
    let headers = cfg.headers.as_ref().map(serde_json::to_string).transpose()?;
    let options = cfg.options.as_ref().map(serde_json::to_string).transpose()?;
    let openapi = cfg.openapi.as_ref().map(serde_json::to_string).transpose()?;
    let proxy = cfg.proxy.as_ref().map(serde_json::to_string).transpose()?;
    let enable_keep_alive = cfg.enable_keep_alive.map(|b| b as i64);
    let keep_alive_interval = cfg.keep_alive_interval.map(|v| v as i64);
    let passthrough_headers = cfg
        .passthrough_headers
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let server_type = encode_server_type(&cfg.server_type);
    let enabled = cfg.enabled as i64;
    let per_session_client = cfg.per_session_client.unwrap_or(false) as i64;
    let start_on_demand = cfg.start_on_demand.unwrap_or(false) as i64;
    let idle_timeout_ms = cfg.idle_timeout_ms.unwrap_or(0) as i64;

    let fts_text = server_fts_text(&cfg.name, cfg.description.as_deref());

    let mut tx = db::pool().begin().await?;
    let result = sqlx::query(
        "UPDATE servers SET name=?, server_type=?, description=?, command=?, args=?, env=?, url=?,
         headers=?, options=?, openapi=?, per_session_client=?, start_on_demand=?, idle_timeout_ms=?, proxy=?, enable_keep_alive=?, keep_alive_interval=?, passthrough_headers=?, enabled=?, updated_at=datetime('now') WHERE name=?",
    )
    .bind(&cfg.name)
    .bind(server_type)
    .bind(&cfg.description)
    .bind(&cfg.command)
    .bind(&args)
    .bind(&env)
    .bind(&cfg.url)
    .bind(&headers)
    .bind(&options)
    .bind(&openapi)
    .bind(per_session_client)
    .bind(start_on_demand)
    .bind(idle_timeout_ms)
    .bind(&proxy)
    .bind(enable_keep_alive)
    .bind(keep_alive_interval)
    .bind(&passthrough_headers)
    .bind(enabled)
    .bind(name)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        // Rename onto an existing server name hits the UNIQUE index — surface
        // the same friendly message `create` uses instead of raw SQL text.
        if e.to_string().contains("UNIQUE constraint failed") {
            anyhow!("Server name '{}' is already in use", cfg.name)
        } else {
            anyhow!(e.to_string())
        }
    })?;

    if result.rows_affected() == 0 {
        return Err(anyhow!("Server '{}' not found", name));
    }

    // FTS 同步（§4.3 铁律：同事务）。改名 = 删旧 ref_id 行 + 插新 ref_id 行（P5）
    if name != cfg.name {
        crate::services::fts_service::sync_delete_tx(
            &mut tx,
            crate::services::fts_service::FtsTable::Servers,
            name,
        )
        .await?;
        // Groups store members by server NAME — a rename must rewrite the
        // member lists in the same transaction or every referencing group is
        // left with a dangling member (tools silently missing from /mcp/{group}).
        cascade_groups_rename_tx(&mut tx, name, &cfg.name).await?;
        // Bearer keys scope by server NAME too — a rename must rewrite
        // `allowed_servers` or every key scoped to the old name silently loses
        // access to the server.
        cascade_bearer_keys_rename_tx(&mut tx, name, &cfg.name).await?;
        // Per-tool enabled/description overrides are keyed by server NAME — a
        // rename must rewrite `server_tool_config.server_name` or previously
        // disabled tools silently become callable again on the renamed server.
        sqlx::query("UPDATE server_tool_config SET server_name=? WHERE server_name=?")
            .bind(&cfg.name)
            .bind(name)
            .execute(&mut *tx)
            .await?;
        // Drop the old name's connect-lock entry (renamed → old key orphaned).
        let old_name = name.to_string();
        tokio::spawn(async move { crate::mcp::pool::forget_connect_lock(&old_name).await });
    }
    crate::services::fts_service::sync_upsert_tx(
        &mut tx,
        crate::services::fts_service::FtsTable::Servers,
        &cfg.name,
        &fts_text,
    )
    .await?;
    tx.commit().await?;

    let out = get_by_name(&cfg.name).await?.ok_or_else(|| anyhow!("Server not found after update"));
    // A3: command/env/args changes reconnect (tool set may change);
    // description-only edits are rare enough that a spurious notification is
    // harmless — keep the notification unconditional for simplicity.
    crate::services::subscription_hub::notify_tools_list_changed().await;
    // Prompts/resources exposure changes with server create/rename/delete/toggle
    // too — without the trio fan-out connected clients cache stale lists (R119).
    crate::services::subscription_hub::notify_prompts_list_changed().await;
    crate::services::subscription_hub::notify_resources_list_changed().await;
    out
}

pub async fn delete(name: &str) -> Result<()> {
    let mut tx = db::pool().begin().await?;
    sqlx::query("DELETE FROM servers WHERE name = ?")
        .bind(name)
        .execute(&mut *tx)
        .await?;
    crate::services::fts_service::sync_delete_tx(
        &mut tx,
        crate::services::fts_service::FtsTable::Servers,
        name,
    )
    .await?;
    // Groups store members by server NAME — drop the deleted server from all
    // member lists in the same transaction (dangling members otherwise
    // silently resolve to nothing on the /mcp/{group} endpoint).
    cascade_groups_remove_tx(&mut tx, name).await?;
    // Bearer keys + per-tool config are keyed by server NAME — drop stale
    // references in the same transaction (stale allowed_servers entries would
    // keep granting access to a dead name; stale tool-config rows would
    // silently resurrect disabled state if the name is ever reused).
    sqlx::query("DELETE FROM server_tool_config WHERE server_name=?")
        .bind(name)
        .execute(&mut *tx)
        .await?;
    cascade_bearer_keys_remove_tx(&mut tx, name).await?;
    tx.commit().await?;
    // Connect-lock entry is keyed by NAME and never contended again — drop it
    // or the map grows without bound over a long session (rename/delete).
    crate::mcp::pool::forget_connect_lock(name).await;
    crate::services::subscription_hub::notify_tools_list_changed().await;
    // Prompts/resources exposure changes with server create/rename/delete/toggle
    // too — without the trio fan-out connected clients cache stale lists (R119).
    crate::services::subscription_hub::notify_prompts_list_changed().await;
    crate::services::subscription_hub::notify_resources_list_changed().await;
    Ok(())
}

/// Rewrite every group's `servers` member list, replacing `old_name` with
/// `new_name` (server rename). Must run inside the caller's transaction.
/// Member name of a group `servers[]` entry: legacy plain string or the
/// object form `{name, ...}`.
fn group_member_name(m: &serde_json::Value) -> Option<&str> {
    match m {
        serde_json::Value::String(s) => Some(s.as_str()),
        v => v.get("name").and_then(|n| n.as_str()),
    }
}

async fn cascade_groups_rename_tx(
    tx: &mut sqlx::SqliteConnection,
    old_name: &str,
    new_name: &str,
) -> Result<()> {
    let rows = sqlx::query("SELECT id, servers FROM groups")
        .fetch_all(&mut *tx)
        .await?;
    for row in rows {
        let id: String = row.try_get("id")?;
        let servers_str: String = row.try_get("servers")?;
        let members: Vec<serde_json::Value> = serde_json::from_str(&servers_str).unwrap_or_default();
        // Members may be plain strings (legacy) or objects {name, tools?, pinnedTools?...}.

        let has_old = members.iter().any(|m| group_member_name(m) == Some(old_name));
        if !has_old {
            continue;
        }
        let rewritten: Vec<serde_json::Value> = members
            .into_iter()
            .map(|m| {
                if m.as_str() == Some(old_name) {
                    serde_json::Value::String(new_name.to_string())
                } else if m.get("name").and_then(|v| v.as_str()) == Some(old_name) {
                    // Object member: rewrite only `name`, keep alias/tools/pinnedTools.
                    let mut obj = m;
                    if let Some(map) = obj.as_object_mut() {
                        map.insert("name".into(), serde_json::Value::String(new_name.to_string()));
                    }
                    obj
                } else {
                    m
                }
            })
            .collect();
        sqlx::query("UPDATE groups SET servers=? WHERE id=?")
            .bind(serde_json::to_string(&rewritten).unwrap_or_else(|_| "[]".into()))
            .bind(&id)
            .execute(&mut *tx)
            .await?;
    }
    Ok(())
}

/// Remove `name` from every group's `servers` member list (server delete).
/// Must run inside the caller's transaction.
async fn cascade_groups_remove_tx(tx: &mut sqlx::SqliteConnection, name: &str) -> Result<()> {
    let rows = sqlx::query("SELECT id, servers FROM groups")
        .fetch_all(&mut *tx)
        .await?;
    for row in rows {
        let id: String = row.try_get("id")?;
        let servers_str: String = row.try_get("servers")?;
        let members: Vec<serde_json::Value> = serde_json::from_str(&servers_str).unwrap_or_default();
        // Members may be plain strings (legacy) or objects {name, ...}.

        let has_old = members.iter().any(|m| group_member_name(m) == Some(name));
        if !has_old {
            continue;
        }
        let rewritten: Vec<serde_json::Value> = members
            .into_iter()
            .filter(|m| group_member_name(m) != Some(name))
            .collect();
        sqlx::query("UPDATE groups SET servers=? WHERE id=?")
            .bind(serde_json::to_string(&rewritten).unwrap_or_else(|_| "[]".into()))
            .bind(&id)
            .execute(&mut *tx)
            .await?;
    }
    Ok(())
}

/// Rewrite every bearer key's `allowed_servers`, replacing `old_name` with
/// `new_name` (server rename). Must run inside the caller's transaction.
async fn cascade_bearer_keys_rename_tx(
    tx: &mut sqlx::SqliteConnection,
    old_name: &str,
    new_name: &str,
) -> Result<()> {
    let rows = sqlx::query("SELECT id, allowed_servers FROM bearer_keys")
        .fetch_all(&mut *tx)
        .await?;
    for row in rows {
        let id: String = row.try_get("id")?;
        let servers_str: String = row.try_get("allowed_servers")?;
        let mut servers: Vec<String> = serde_json::from_str(&servers_str).unwrap_or_default();
        if !servers.iter().any(|s| s == old_name) {
            continue;
        }
        for s in servers.iter_mut() {
            if s == old_name {
                *s = new_name.to_string();
            }
        }
        sqlx::query("UPDATE bearer_keys SET allowed_servers=? WHERE id=?")
            .bind(serde_json::to_string(&servers).unwrap_or_else(|_| "[]".into()))
            .bind(&id)
            .execute(&mut *tx)
            .await?;
    }
    Ok(())
}

/// Remove `name` from every bearer key's `allowed_servers` (server delete).
/// Must run inside the caller's transaction.
async fn cascade_bearer_keys_remove_tx(
    tx: &mut sqlx::SqliteConnection,
    name: &str,
) -> Result<()> {
    let rows = sqlx::query("SELECT id, allowed_servers FROM bearer_keys")
        .fetch_all(&mut *tx)
        .await?;
    for row in rows {
        let id: String = row.try_get("id")?;
        let servers_str: String = row.try_get("allowed_servers")?;
        let servers: Vec<String> = serde_json::from_str(&servers_str).unwrap_or_default();
        if !servers.iter().any(|s| s == name) {
            continue;
        }
        let filtered: Vec<String> = servers.into_iter().filter(|s| s != name).collect();
        sqlx::query("UPDATE bearer_keys SET allowed_servers=? WHERE id=?")
            .bind(serde_json::to_string(&filtered).unwrap_or_else(|_| "[]".into()))
            .bind(&id)
            .execute(&mut *tx)
            .await?;
    }
    Ok(())
}

pub async fn toggle_enabled(name: &str) -> Result<ServerConfig> {
    let updated = sqlx::query(
        "UPDATE servers SET enabled = CASE WHEN enabled=1 THEN 0 ELSE 1 END, updated_at=datetime('now') WHERE name=?",
    )
    .bind(name)
    .execute(db::pool())
    .await?
    .rows_affected()
        > 0;
    let cfg = get_by_name(name)
        .await?
        .ok_or_else(|| anyhow!("Server '{}' not found", name));
    // Enable/disable changes the exposed tool set (tools are filtered by
    // `enabled` on HTTP endpoints) — subscribed MCP clients must be told to
    // re-fetch tools/list. Every other tool-set-changing path (create/update/
    // delete/tool-config) notifies; this path was the only omission.
    if updated {
        crate::services::subscription_hub::notify_tools_list_changed().await;
    // Prompts/resources exposure changes with server create/rename/delete/toggle
    // too — without the trio fan-out connected clients cache stale lists (R119).
    crate::services::subscription_hub::notify_prompts_list_changed().await;
    crate::services::subscription_hub::notify_resources_list_changed().await;
    }
    cfg
}

// ---------------------------------------------------------------------------
// Row mapper (shared by all SELECT queries)
// ---------------------------------------------------------------------------
fn map_row(r: sqlx::sqlite::SqliteRow) -> Result<ServerConfig> {
    let args: Option<Vec<String>> = r
        .try_get::<Option<String>, _>("args")?
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?;
    let env: Option<HashMap<String, String>> = r
        .try_get::<Option<String>, _>("env")?
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?;
    let headers: Option<HashMap<String, String>> = r
        .try_get::<Option<String>, _>("headers")?
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?;
    let options: Option<ServerOptions> = r
        .try_get::<Option<String>, _>("options")?
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?;
    let openapi: Option<OpenApiConfig> = r
        .try_get::<Option<String>, _>("openapi")?
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?;
    let proxy: Option<ProxychainsConfig> = r
        .try_get::<Option<String>, _>("proxy")?
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?;
    let enable_keep_alive = r
        .try_get::<Option<i64>, _>("enable_keep_alive")?
        .map(|v| v != 0);
    let keep_alive_interval = r
        .try_get::<Option<i64>, _>("keep_alive_interval")?
        .map(|v| v as u64);
    let passthrough_headers: Option<Vec<String>> = r
        .try_get::<Option<String>, _>("passthrough_headers")?
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?;
    Ok(ServerConfig {
        id: r.try_get("id")?,
        name: r.try_get("name")?,
        server_type: decode_server_type(r.try_get::<&str, _>("server_type")?),
        description: r.try_get("description")?,
        command: r.try_get("command")?,
        args,
        env,
        url: r.try_get("url")?,
        headers,
        options,
        openapi,
        proxy,
        enable_keep_alive,
        keep_alive_interval,
        passthrough_headers,
        per_session_client: Some(r.try_get::<i64, _>("per_session_client")? != 0),
        start_on_demand: Some(r.try_get::<i64, _>("start_on_demand")? != 0),
        idle_timeout_ms: {
            let ms = r.try_get::<i64, _>("idle_timeout_ms")?;
            if ms > 0 { Some(ms as u64) } else { None }
        },
        enabled: r.try_get::<i64, _>("enabled")? != 0,
    })
}
