use crate::{
    db,
    models::server::{OpenApiConfig, ServerConfig, ServerOptions, ServerType},
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
        "SELECT id, name, server_type, description, command, args, env, url, headers, options, openapi, per_session_client, start_on_demand, idle_timeout_ms, enabled
         FROM servers WHERE enabled = 1",
    )
    .fetch_all(db::pool())
    .await?;
    rows.into_iter().map(map_row).collect()
}

pub async fn list_all() -> Result<Vec<ServerConfig>> {
    let rows = sqlx::query(
        "SELECT id, name, server_type, description, command, args, env, url, headers, options, openapi, per_session_client, start_on_demand, idle_timeout_ms, enabled
         FROM servers ORDER BY name",
    )
    .fetch_all(db::pool())
    .await?;
    rows.into_iter().map(map_row).collect()
}

pub async fn get_by_name(name: &str) -> Result<Option<ServerConfig>> {
    let row = sqlx::query(
        "SELECT id, name, server_type, description, command, args, env, url, headers, options, openapi, per_session_client, start_on_demand, idle_timeout_ms, enabled
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

pub async fn create(cfg: &ServerConfig) -> Result<ServerConfig> {
    // The name "RAG" is reserved for the builtin RAG server (see
    // rag::service::BUILTIN_SERVER_NAME). Reject custom servers using it so a
    // group referencing "RAG" always means the builtin, never a custom server.
    if cfg.name.eq_ignore_ascii_case(crate::rag::service::BUILTIN_SERVER_NAME) {
        return Err(anyhow!("server name '{}' is reserved for the builtin server", cfg.name));
    }
    validate_combination(cfg)?;
    let id = Uuid::new_v4().to_string();
    let args = cfg.args.as_ref().map(|a| serde_json::to_string(a)).transpose()?;
    let env = cfg.env.as_ref().map(|e| serde_json::to_string(e)).transpose()?;
    let headers = cfg.headers.as_ref().map(|h| serde_json::to_string(h)).transpose()?;
    let options = cfg.options.as_ref().map(|o| serde_json::to_string(o)).transpose()?;
    let openapi = cfg.openapi.as_ref().map(|o| serde_json::to_string(o)).transpose()?;
    let server_type = encode_server_type(&cfg.server_type);
    let enabled = cfg.enabled as i64;
    let per_session_client = cfg.per_session_client.unwrap_or(false) as i64;
    let start_on_demand = cfg.start_on_demand.unwrap_or(false) as i64;
    let idle_timeout_ms = cfg.idle_timeout_ms.unwrap_or(0) as i64;

    sqlx::query(
        "INSERT INTO servers (id, name, server_type, description, command, args, env, url, headers, options, openapi, per_session_client, start_on_demand, idle_timeout_ms, enabled)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
    .bind(enabled)
    .execute(db::pool())
    .await
    .map_err(|e| {
        let msg = e.to_string();
        if msg.contains("UNIQUE constraint failed") {
            anyhow!("A server with the name '{}' already exists", cfg.name)
        } else {
            anyhow!(msg)
        }
    })?;

    get_by_name(&cfg.name).await?.ok_or_else(|| anyhow!("Insert failed"))
}

pub async fn update(name: &str, cfg: &ServerConfig) -> Result<ServerConfig> {
    // Reject renaming TO the reserved builtin name (renaming FROM it is moot -
    // the builtin has no DB row to update).
    if cfg.name.eq_ignore_ascii_case(crate::rag::service::BUILTIN_SERVER_NAME) {
        return Err(anyhow!("server name '{}' is reserved for the builtin server", cfg.name));
    }
    validate_combination(cfg)?;
    let args = cfg.args.as_ref().map(|a| serde_json::to_string(a)).transpose()?;
    let env = cfg.env.as_ref().map(|e| serde_json::to_string(e)).transpose()?;
    let headers = cfg.headers.as_ref().map(|h| serde_json::to_string(h)).transpose()?;
    let options = cfg.options.as_ref().map(|o| serde_json::to_string(o)).transpose()?;
    let openapi = cfg.openapi.as_ref().map(|o| serde_json::to_string(o)).transpose()?;
    let server_type = encode_server_type(&cfg.server_type);
    let enabled = cfg.enabled as i64;
    let per_session_client = cfg.per_session_client.unwrap_or(false) as i64;
    let start_on_demand = cfg.start_on_demand.unwrap_or(false) as i64;
    let idle_timeout_ms = cfg.idle_timeout_ms.unwrap_or(0) as i64;

    sqlx::query(
        "UPDATE servers SET name=?, server_type=?, description=?, command=?, args=?, env=?, url=?,
         headers=?, options=?, openapi=?, per_session_client=?, start_on_demand=?, idle_timeout_ms=?, enabled=?, updated_at=datetime('now') WHERE name=?",
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
    .bind(enabled)
    .bind(name)
    .execute(db::pool())
    .await?;

    get_by_name(&cfg.name).await?.ok_or_else(|| anyhow!("Server not found after update"))
}

pub async fn delete(name: &str) -> Result<()> {
    sqlx::query("DELETE FROM servers WHERE name = ?")
        .bind(name)
        .execute(db::pool())
        .await?;
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
        per_session_client: Some(r.try_get::<i64, _>("per_session_client")? != 0),
        start_on_demand: Some(r.try_get::<i64, _>("start_on_demand")? != 0),
        idle_timeout_ms: {
            let ms = r.try_get::<i64, _>("idle_timeout_ms")?;
            if ms > 0 { Some(ms as u64) } else { None }
        },
        enabled: r.try_get::<i64, _>("enabled")? != 0,
    })
}
