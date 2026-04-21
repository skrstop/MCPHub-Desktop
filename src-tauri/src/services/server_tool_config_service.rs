use crate::db;
use crate::models::server::Tool;
use crate::models::server_tool_config::{ServerToolConfig, ServerToolConfigPayload};
use anyhow::Result;
use sqlx::Row;
use std::collections::HashMap;
use uuid::Uuid;

pub async fn get_config(
    server_name: &str,
    item_type: &str,
    item_name: &str,
) -> Result<Option<ServerToolConfig>> {
    let row = sqlx::query(
        "SELECT id, server_name, item_type, item_name, enabled, description, created_at, updated_at
         FROM server_tool_config
         WHERE server_name=? AND item_type=? AND item_name=?",
    )
    .bind(server_name)
    .bind(item_type)
    .bind(item_name)
    .fetch_optional(db::pool())
    .await?;

    Ok(row.map(|r| ServerToolConfig {
        id: r.try_get("id").unwrap_or_default(),
        server_name: r.try_get("server_name").unwrap_or_default(),
        item_type: r.try_get("item_type").unwrap_or_default(),
        item_name: r.try_get("item_name").unwrap_or_default(),
        enabled: r.try_get::<i64, _>("enabled").unwrap_or(1) != 0,
        description: r.try_get("description").ok(),
        created_at: r.try_get("created_at").unwrap_or_default(),
        updated_at: r.try_get("updated_at").unwrap_or_default(),
    }))
}

/// List all overrides for a given server (and optionally item_type).
pub async fn list_for_server(
    server_name: &str,
    item_type: Option<&str>,
) -> Result<Vec<ServerToolConfig>> {
    let rows = if let Some(t) = item_type {
        sqlx::query(
            "SELECT id, server_name, item_type, item_name, enabled, description, created_at, updated_at
             FROM server_tool_config WHERE server_name=? AND item_type=?",
        )
        .bind(server_name)
        .bind(t)
        .fetch_all(db::pool())
        .await?
    } else {
        sqlx::query(
            "SELECT id, server_name, item_type, item_name, enabled, description, created_at, updated_at
             FROM server_tool_config WHERE server_name=?",
        )
        .bind(server_name)
        .fetch_all(db::pool())
        .await?
    };

    Ok(rows
        .iter()
        .map(|r| ServerToolConfig {
            id: r.try_get("id").unwrap_or_default(),
            server_name: r.try_get("server_name").unwrap_or_default(),
            item_type: r.try_get("item_type").unwrap_or_default(),
            item_name: r.try_get("item_name").unwrap_or_default(),
            enabled: r.try_get::<i64, _>("enabled").unwrap_or(1) != 0,
            description: r.try_get("description").ok(),
            created_at: r.try_get("created_at").unwrap_or_default(),
            updated_at: r.try_get("updated_at").unwrap_or_default(),
        })
        .collect())
}

/// Upsert an override (insert or update on conflict).
pub async fn upsert(p: &ServerToolConfigPayload) -> Result<ServerToolConfig> {
    let id = Uuid::new_v4().to_string();
    let enabled_i: i64 = if p.enabled { 1 } else { 0 };
    sqlx::query(
        "INSERT INTO server_tool_config (id, server_name, item_type, item_name, enabled, description)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(server_name, item_type, item_name) DO UPDATE SET
           enabled=excluded.enabled,
           description=excluded.description,
           updated_at=datetime('now')",
    )
    .bind(&id)
    .bind(&p.server_name)
    .bind(&p.item_type)
    .bind(&p.item_name)
    .bind(enabled_i)
    .bind(&p.description)
    .execute(db::pool())
    .await?;

    get_config(&p.server_name, &p.item_type, &p.item_name)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Failed to fetch after upsert"))
}

/// Update only the description of an item.
pub async fn update_description(
    server_name: &str,
    item_type: &str,
    item_name: &str,
    description: Option<&str>,
) -> Result<()> {
    // If no override exists yet, insert one (enabled by default)
    let exists = get_config(server_name, item_type, item_name).await?.is_some();
    if exists {
        sqlx::query(
            "UPDATE server_tool_config SET description=?, updated_at=datetime('now')
             WHERE server_name=? AND item_type=? AND item_name=?",
        )
        .bind(description)
        .bind(server_name)
        .bind(item_type)
        .bind(item_name)
        .execute(db::pool())
        .await?;
    } else {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO server_tool_config (id, server_name, item_type, item_name, enabled, description)
             VALUES (?, ?, ?, ?, 1, ?)",
        )
        .bind(&id)
        .bind(server_name)
        .bind(item_type)
        .bind(item_name)
        .bind(description)
        .execute(db::pool())
        .await?;
    }
    Ok(())
}

/// Reset description to NULL (remove override).
pub async fn reset_description(server_name: &str, item_type: &str, item_name: &str) -> Result<()> {
    sqlx::query(
        "UPDATE server_tool_config SET description=NULL, updated_at=datetime('now')
         WHERE server_name=? AND item_type=? AND item_name=?",
    )
    .bind(server_name)
    .bind(item_type)
    .bind(item_name)
    .execute(db::pool())
    .await?;
    Ok(())
}

/// Filter and apply description overrides to a server's tool list.
/// Tools disabled via server_tool_config are excluded; description overrides are applied.
pub async fn apply_tool_filters(server_name: &str, tools: Vec<Tool>) -> Result<Vec<Tool>> {
    if tools.is_empty() {
        return Ok(tools);
    }
    let configs = list_for_server(server_name, Some("tool")).await?;
    if configs.is_empty() {
        return Ok(tools); // No overrides — return as-is
    }
    let config_map: HashMap<String, &ServerToolConfig> =
        configs.iter().map(|c| (c.item_name.clone(), c)).collect();

    Ok(tools
        .into_iter()
        .filter_map(|mut tool| {
            if let Some(cfg) = config_map.get(&tool.name) {
                if !cfg.enabled {
                    return None; // Skip disabled tool
                }
                if let Some(ref desc) = cfg.description {
                    tool.description = Some(desc.clone());
                }
            }
            Some(tool)
        })
        .collect())
}
