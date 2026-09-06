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
        "SELECT id, name, server_type, description, command, args, env, url, headers, options, openapi, per_session_client, start_on_demand, idle_timeout_ms, proxy, enabled
         FROM servers WHERE enabled = 1",
    )
    .fetch_all(db::pool())
    .await?;
    rows.into_iter().map(map_row).collect()
}

pub async fn list_all() -> Result<Vec<ServerConfig>> {
    let rows = sqlx::query(
        "SELECT id, name, server_type, description, command, args, env, url, headers, options, openapi, per_session_client, start_on_demand, idle_timeout_ms, proxy, enabled
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
            "SELECT id, name, server_type, description, command, args, env, url, headers, options, openapi, per_session_client, start_on_demand, idle_timeout_ms, proxy, enabled \
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
                "SELECT id, name, server_type, description, command, args, env, url, headers, options, openapi, per_session_client, start_on_demand, idle_timeout_ms, proxy, enabled \
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
    let pattern = format!("%{}%", key.to_lowercase().replace('%', "\\%").replace('_', "\\_"));
    let rows = sqlx::query(
        "SELECT id, name, server_type, description, command, args, env, url, headers, options, openapi, per_session_client, start_on_demand, idle_timeout_ms, proxy, enabled \
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
        "SELECT id, name, server_type, description, command, args, env, url, headers, options, openapi, per_session_client, start_on_demand, idle_timeout_ms, proxy, enabled
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
    validate_combination(cfg)?;
    let id = Uuid::new_v4().to_string();
    let args = cfg.args.as_ref().map(serde_json::to_string).transpose()?;
    let env = cfg.env.as_ref().map(serde_json::to_string).transpose()?;
    let headers = cfg.headers.as_ref().map(serde_json::to_string).transpose()?;
    let options = cfg.options.as_ref().map(serde_json::to_string).transpose()?;
    let openapi = cfg.openapi.as_ref().map(serde_json::to_string).transpose()?;
    let proxy = cfg.proxy.as_ref().map(serde_json::to_string).transpose()?;
    let server_type = encode_server_type(&cfg.server_type);
    let enabled = cfg.enabled as i64;
    let per_session_client = cfg.per_session_client.unwrap_or(false) as i64;
    let start_on_demand = cfg.start_on_demand.unwrap_or(false) as i64;
    let idle_timeout_ms = cfg.idle_timeout_ms.unwrap_or(0) as i64;

    let fts_text = server_fts_text(&cfg.name, cfg.description.as_deref());

    let mut tx = db::pool().begin().await?;
    sqlx::query(
        "INSERT INTO servers (id, name, server_type, description, command, args, env, url, headers, options, openapi, per_session_client, start_on_demand, idle_timeout_ms, proxy, enabled)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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

    get_by_name(&cfg.name).await?.ok_or_else(|| anyhow!("Insert failed"))
}

pub async fn update(name: &str, cfg: &ServerConfig) -> Result<ServerConfig> {
    // Reject renaming TO the reserved builtin name (renaming FROM it is moot -
    // the builtin has no DB row to update).
    if cfg.name.eq_ignore_ascii_case(crate::rag::service::BUILTIN_SERVER_NAME) {
        return Err(anyhow!("server name '{}' is reserved for the builtin server", cfg.name));
    }
    validate_combination(cfg)?;
    let args = cfg.args.as_ref().map(serde_json::to_string).transpose()?;
    let env = cfg.env.as_ref().map(serde_json::to_string).transpose()?;
    let headers = cfg.headers.as_ref().map(serde_json::to_string).transpose()?;
    let options = cfg.options.as_ref().map(serde_json::to_string).transpose()?;
    let openapi = cfg.openapi.as_ref().map(serde_json::to_string).transpose()?;
    let proxy = cfg.proxy.as_ref().map(serde_json::to_string).transpose()?;
    let server_type = encode_server_type(&cfg.server_type);
    let enabled = cfg.enabled as i64;
    let per_session_client = cfg.per_session_client.unwrap_or(false) as i64;
    let start_on_demand = cfg.start_on_demand.unwrap_or(false) as i64;
    let idle_timeout_ms = cfg.idle_timeout_ms.unwrap_or(0) as i64;

    let fts_text = server_fts_text(&cfg.name, cfg.description.as_deref());

    let mut tx = db::pool().begin().await?;
    let result = sqlx::query(
        "UPDATE servers SET name=?, server_type=?, description=?, command=?, args=?, env=?, url=?,
         headers=?, options=?, openapi=?, per_session_client=?, start_on_demand=?, idle_timeout_ms=?, proxy=?, enabled=?, updated_at=datetime('now') WHERE name=?",
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
    .bind(enabled)
    .bind(name)
    .execute(&mut *tx)
    .await?;

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
    }
    crate::services::fts_service::sync_upsert_tx(
        &mut tx,
        crate::services::fts_service::FtsTable::Servers,
        &cfg.name,
        &fts_text,
    )
    .await?;
    tx.commit().await?;

    get_by_name(&cfg.name).await?.ok_or_else(|| anyhow!("Server not found after update"))
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
    tx.commit().await?;
    Ok(())
}

pub async fn toggle_enabled(name: &str) -> Result<ServerConfig> {
    sqlx::query(
        "UPDATE servers SET enabled = CASE WHEN enabled=1 THEN 0 ELSE 1 END, updated_at=datetime('now') WHERE name=?",
    )
    .bind(name)
    .execute(db::pool())
    .await?;
    get_by_name(name)
        .await?
        .ok_or_else(|| anyhow!("Server '{}' not found", name))
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
        per_session_client: Some(r.try_get::<i64, _>("per_session_client")? != 0),
        start_on_demand: Some(r.try_get::<i64, _>("start_on_demand")? != 0),
        idle_timeout_ms: {
            let ms = r.try_get::<i64, _>("idle_timeout_ms")?;
            if ms > 0 { Some(ms as u64) } else { None }
        },
        enabled: r.try_get::<i64, _>("enabled")? != 0,
    })
}
