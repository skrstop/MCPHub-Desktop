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
        "SELECT id, server_name, item_type, item_name, enabled, description, pinned, created_at, updated_at
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
        pinned: r.try_get::<i64, _>("pinned").unwrap_or(0) != 0,
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
            "SELECT id, server_name, item_type, item_name, enabled, description, pinned, created_at, updated_at
             FROM server_tool_config WHERE server_name=? AND item_type=?",
        )
        .bind(server_name)
        .bind(t)
        .fetch_all(db::pool())
        .await?
    } else {
        sqlx::query(
            "SELECT id, server_name, item_type, item_name, enabled, description, pinned, created_at, updated_at
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
            pinned: r.try_get::<i64, _>("pinned").unwrap_or(0) != 0,
            created_at: r.try_get("created_at").unwrap_or_default(),
            updated_at: r.try_get("updated_at").unwrap_or_default(),
        })
        .collect())
}

/// Upsert an override (insert or update on conflict).
pub async fn upsert(p: &ServerToolConfigPayload) -> Result<ServerToolConfig> {
    // Read failure degrades to None (treat as changed) rather than failing
    // the whole write — `previous` only drives change-detection for the
    // list_changed notification (review round 9).
    let previous = get_config(&p.server_name, &p.item_type, &p.item_name).await.ok().flatten();
    let id = Uuid::new_v4().to_string();
    let enabled_i: i64 = if p.enabled { 1 } else { 0 };
    let pinned_i: i64 = if p.pinned.unwrap_or(false) { 1 } else { 0 };
    // `pinned` is intentionally absent from the DO UPDATE clause: toggle /
    // description writes (payload.pinned = None) must preserve the existing
    // pin; only set_pinned (Some) writes it, via its own statement below.
    sqlx::query(
        "INSERT INTO server_tool_config (id, server_name, item_type, item_name, enabled, description, pinned)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(server_name, item_type, item_name) DO UPDATE SET
           enabled=excluded.enabled,
           description=CASE WHEN excluded.description IS NULL THEN server_tool_config.description ELSE excluded.description END,
           updated_at=datetime('now')",
    )
    .bind(&id)
    .bind(&p.server_name)
    .bind(&p.item_type)
    .bind(&p.item_name)
    .bind(enabled_i)
    .bind(&p.description)
    .bind(pinned_i)
    .execute(db::pool())
    .await?;

    let out = get_config(&p.server_name, &p.item_type, &p.item_name)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Failed to fetch after upsert"));
    // A3: tool enable/disable changes the exposed tool list — but only notify
    // when the write changed something (same-value overwrite would trigger a
    // pointless client re-list; review round 8, 2026-10-04).
    let changed = previous.as_ref().map(|prev| {
        prev.enabled != p.enabled
            // description=None means "keep current" (SQL CASE WHEN preserve
            // semantics) — only an explicit Some value can change it.
            || p.description.as_ref().map_or(false, |d| Some(d) != prev.description.as_ref())
    }).unwrap_or(true);
    if changed {
        crate::services::subscription_hub::notify_tools_list_changed().await;
    }
    out
}

/// Update only the description of an item.
pub async fn update_description(
    server_name: &str,
    item_type: &str,
    item_name: &str,
    description: Option<&str>,
) -> Result<()> {
    // If no override exists yet, insert one (enabled by default)
    // Read failure degrades to None (treat as changed) — see upsert (round 9).
    let previous = get_config(server_name, item_type, item_name).await.ok().flatten();
    // Unconditional upsert (not read-then-insert): two concurrent calls for
    // the same (server, type, item) both see "not exists" and race the INSERT
    // — the loser fails the UNIQUE constraint. Same handling as `upsert`.
    sqlx::query(
        "INSERT INTO server_tool_config (id, server_name, item_type, item_name, enabled, description)
         VALUES (?, ?, ?, ?, 1, ?)
         ON CONFLICT(server_name, item_type, item_name)
         DO UPDATE SET description=excluded.description, updated_at=datetime('now')",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(server_name)
    .bind(item_type)
    .bind(item_name)
    .bind(description)
    .execute(db::pool())
    .await?;
    // Description overrides flow through apply_tool_filters into the exposed
    // tools/list — notify only when the value actually changed.
    let changed = previous
        .as_ref()
        .map(|prev| prev.description != description.map(|d| d.to_string()))
        .unwrap_or(true);
    if changed {
        crate::services::subscription_hub::notify_tools_list_changed().await;
    }
    Ok(())
}

/// Reset description to NULL (remove override).
pub async fn reset_description(server_name: &str, item_type: &str, item_name: &str) -> Result<()> {
    // Read failure degrades to None (treat as changed) — see upsert (round 9).
    let previous = get_config(server_name, item_type, item_name).await.ok().flatten();
    sqlx::query(
        "UPDATE server_tool_config SET description=NULL, updated_at=datetime('now')
         WHERE server_name=? AND item_type=? AND item_name=?",
    )
    .bind(server_name)
    .bind(item_type)
    .bind(item_name)
    .execute(db::pool())
    .await?;
    // Same exposed-list change as update_description — notify only when a row
    // was actually reset (0 rows / already-NULL reset is a no-op).
    let changed = previous
        .as_ref()
        .map(|prev| prev.description.is_some())
        .unwrap_or(false);
    if changed {
        crate::services::subscription_hub::notify_tools_list_changed().await;
    }
    Ok(())
}

/// Set the server-level $smart pin for an item (tools only in practice).
pub async fn set_pinned(
    server_name: &str,
    item_type: &str,
    item_name: &str,
    pinned: bool,
) -> Result<()> {
    let previous = match get_config(server_name, item_type, item_name).await {
        Ok(p) => p,
        // A transient read failure must not silently turn unpin into a
        // reported success (the row may exist with pinned=1 — the early
        // return below would skip the write and the caller would show
        // "unpinned" while $smart keeps exposing the tool). Unpin propagates
        // the real error; pin can proceed (upsert creates the row).
        Err(e) if !pinned => return Err(e),
        Err(_) => None,
    };
    // Unpinning a tool that never had an override must not CREATE a dead row
    // (R5-1): the command layer frames unpin as dead-row cleanup, so an
    // INSERT here would be the only producer of spurious override rows.
    if previous.is_none() && !pinned {
        return Ok(());
    }
    // Upsert keeps enabled/description untouched when the row exists.
    sqlx::query(
        "INSERT INTO server_tool_config (id, server_name, item_type, item_name, enabled, description, pinned)
         VALUES (?, ?, ?, ?, 1, NULL, ?)
         ON CONFLICT(server_name, item_type, item_name)
         DO UPDATE SET pinned=excluded.pinned, updated_at=datetime('now')",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(server_name)
    .bind(item_type)
    .bind(item_name)
    .bind(if pinned { 1i64 } else { 0i64 })
    .execute(db::pool())
    .await?;
    // Pin changes the exposed tools/list on $smart endpoints — notify only on
    // an actual flip (same-value writes are no-ops).
    let changed = previous
        .as_ref()
        .map(|prev| prev.pinned != pinned)
        .unwrap_or(pinned);
    if changed && item_type == "tool" {
        crate::services::subscription_hub::notify_tools_list_changed().await;
    }
    Ok(())
}

/// Server-level pinned tool names (item_type='tool', pinned=1). Ordered by
/// creation time; same-second pins fall back to alphabetical item_name
/// (deterministic, but not strictly insertion order).
pub async fn list_pinned_tools(server_name: &str) -> Result<Vec<String>> {
    let rows = sqlx::query(
        "SELECT item_name FROM server_tool_config
         WHERE server_name=? AND item_type='tool' AND pinned=1
         ORDER BY created_at, item_name",
    )
    .bind(server_name)
    .fetch_all(db::pool())
    .await?;
    Ok(rows
        .iter()
        .filter_map(|r| r.try_get::<String, _>("item_name").ok())
        .collect())
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
        .map(|mut tool| {
            if let Some(cfg) = config_map.get(&tool.name) {
                tool.enabled = cfg.enabled;
                if let Some(ref desc) = cfg.description {
                    tool.description = Some(desc.clone());
                    // Mark for the smart toolset hash: override edits must
                    // trigger a re-embed (origin #1198 parity).
                    tool.description_overridden = true;
                }
            }
            tool
        })
        .collect())
}
