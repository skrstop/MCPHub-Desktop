use crate::{db, models::group::{Group, GroupPayload}};
use anyhow::{anyhow, Result};
use sqlx::Row;
use uuid::Uuid;

pub async fn list_all() -> Result<Vec<Group>> {
    let rows = sqlx::query(
        "SELECT id, name, description, servers, created_at FROM groups ORDER BY name",
    )
    .fetch_all(db::pool())
    .await?;

    rows.into_iter()
        .map(|r| {
            let servers_str: String = r.try_get("servers")?;
            let servers: Vec<String> = serde_json::from_str(&servers_str).unwrap_or_default();
            Ok(Group {
                id: r.try_get("id")?,
                name: r.try_get("name")?,
                description: r.try_get("description")?,
                servers,
                created_at: r.try_get("created_at")?,
            })
        })
        .collect()
}

pub async fn create(payload: &GroupPayload) -> Result<Group> {
    let id = Uuid::new_v4().to_string();
    let servers_json = serde_json::to_string(&payload.servers)?;

    sqlx::query(
        "INSERT INTO groups (id, name, description, servers) VALUES (?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&payload.name)
    .bind(&payload.description)
    .bind(&servers_json)
    .execute(db::pool())
    .await?;

    Ok(Group {
        id,
        name: payload.name.clone(),
        description: payload.description.clone(),
        servers: payload.servers.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
    })
}

pub async fn update(id: &str, payload: &GroupPayload) -> Result<Group> {
    let servers_json = serde_json::to_string(&payload.servers)?;
    sqlx::query(
        "UPDATE groups SET name=?, description=?, servers=? WHERE id=?",
    )
    .bind(&payload.name)
    .bind(&payload.description)
    .bind(&servers_json)
    .bind(id)
    .execute(db::pool())
    .await?;

    let row = sqlx::query(
        "SELECT id, name, description, servers, created_at FROM groups WHERE id=?",
    )
    .bind(id)
    .fetch_optional(db::pool())
    .await?
    .ok_or_else(|| anyhow!("Group not found"))?;

    let servers_str: String = row.try_get("servers")?;
    let servers = serde_json::from_str(&servers_str).unwrap_or_default();
    Ok(Group {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        servers,
        created_at: row.try_get("created_at")?,
    })
}

pub async fn delete(id: &str) -> Result<()> {
    sqlx::query("DELETE FROM groups WHERE id=?")
        .bind(id)
        .execute(db::pool())
        .await?;
    Ok(())
}
