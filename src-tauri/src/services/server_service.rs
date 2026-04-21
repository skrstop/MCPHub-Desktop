use crate::{
    db,
    models::server::{ServerConfig, ServerOptions, ServerType},
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
    }
}

pub async fn list_all_enabled() -> Result<Vec<ServerConfig>> {
    let rows = sqlx::query(
        "SELECT id, name, server_type, description, command, args, env, url, headers, options, enabled
         FROM servers WHERE enabled = 1",
    )
    .fetch_all(db::pool())
    .await?;
    rows.into_iter().map(map_row).collect()
}

pub async fn list_all() -> Result<Vec<ServerConfig>> {
    let rows = sqlx::query(
        "SELECT id, name, server_type, description, command, args, env, url, headers, options, enabled
         FROM servers ORDER BY name",
    )
    .fetch_all(db::pool())
    .await?;
    rows.into_iter().map(map_row).collect()
}

pub async fn get_by_name(name: &str) -> Result<Option<ServerConfig>> {
    let row = sqlx::query(
        "SELECT id, name, server_type, description, command, args, env, url, headers, options, enabled
         FROM servers WHERE name = ?",
    )
    .bind(name)
    .fetch_optional(db::pool())
    .await?;
    row.map(map_row).transpose()
}

pub async fn create(cfg: &ServerConfig) -> Result<ServerConfig> {
    let id = Uuid::new_v4().to_string();
    let args = cfg.args.as_ref().map(|a| serde_json::to_string(a)).transpose()?;
    let env = cfg.env.as_ref().map(|e| serde_json::to_string(e)).transpose()?;
    let headers = cfg.headers.as_ref().map(|h| serde_json::to_string(h)).transpose()?;
    let options = cfg.options.as_ref().map(|o| serde_json::to_string(o)).transpose()?;
    let server_type = encode_server_type(&cfg.server_type);
    let enabled = cfg.enabled as i64;

    sqlx::query(
        "INSERT INTO servers (id, name, server_type, description, command, args, env, url, headers, options, enabled)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
    let args = cfg.args.as_ref().map(|a| serde_json::to_string(a)).transpose()?;
    let env = cfg.env.as_ref().map(|e| serde_json::to_string(e)).transpose()?;
    let headers = cfg.headers.as_ref().map(|h| serde_json::to_string(h)).transpose()?;
    let options = cfg.options.as_ref().map(|o| serde_json::to_string(o)).transpose()?;
    let server_type = encode_server_type(&cfg.server_type);
    let enabled = cfg.enabled as i64;

    sqlx::query(
        "UPDATE servers SET name=?, server_type=?, description=?, command=?, args=?, env=?, url=?,
         headers=?, options=?, enabled=?, updated_at=datetime('now') WHERE name=?",
    )
    .bind(&cfg.name)   // new name (may differ from `name` on rename)
    .bind(server_type)
    .bind(&cfg.description)
    .bind(&cfg.command)
    .bind(&args)
    .bind(&env)
    .bind(&cfg.url)
    .bind(&headers)
    .bind(&options)
    .bind(enabled)
    .bind(name)        // WHERE name = old_name
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
    Ok(ServerConfig {
        id: r.try_get("id")?,
        name: r.try_get("name")?,
        server_type: decode_server_type(r.try_get::<&str, _>("server_type")?
),
        description: r.try_get("description")?,
        command: r.try_get("command")?,
        args,
        env,
        url: r.try_get("url")?,
        headers,
        options,
        enabled: r.try_get::<i64, _>("enabled")? != 0,
    })
}
