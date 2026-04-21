use crate::{db, models::bearer_key::{BearerKey, BearerKeyPayload}};
use anyhow::Result;
use sqlx::Row;
use uuid::Uuid;

pub async fn list_all() -> Result<Vec<BearerKey>> {
    let rows = sqlx::query(
        "SELECT id, name, token, enabled, access_type, allowed_groups, allowed_servers, created_at \
         FROM bearer_keys ORDER BY created_at DESC",
    )
    .fetch_all(db::pool())
    .await?;

    rows.into_iter()
        .map(|r| {
            let enabled: i64 = r.try_get("enabled")?;
            let groups_json: String = r.try_get("allowed_groups")?;
            let servers_json: String = r.try_get("allowed_servers")?;
            Ok(BearerKey {
                id: r.try_get("id")?,
                name: r.try_get("name")?,
                token: r.try_get("token")?,
                enabled: enabled != 0,
                access_type: r.try_get("access_type")?,
                allowed_groups: serde_json::from_str(&groups_json).unwrap_or_default(),
                allowed_servers: serde_json::from_str(&servers_json).unwrap_or_default(),
                created_at: r.try_get("created_at")?,
            })
        })
        .collect()
}

pub async fn find_by_token(token: &str) -> Result<Option<BearerKey>> {
    let row = sqlx::query(
        "SELECT id, name, token, enabled, access_type, allowed_groups, allowed_servers, created_at \
         FROM bearer_keys WHERE token = ?",
    )
    .bind(token)
    .fetch_optional(db::pool())
    .await?;

    match row {
        None => Ok(None),
        Some(r) => {
            let enabled: i64 = r.try_get("enabled")?;
            let groups_json: String = r.try_get("allowed_groups")?;
            let servers_json: String = r.try_get("allowed_servers")?;
            Ok(Some(BearerKey {
                id: r.try_get("id")?,
                name: r.try_get("name")?,
                token: r.try_get("token")?,
                enabled: enabled != 0,
                access_type: r.try_get("access_type")?,
                allowed_groups: serde_json::from_str(&groups_json).unwrap_or_default(),
                allowed_servers: serde_json::from_str(&servers_json).unwrap_or_default(),
                created_at: r.try_get("created_at")?,
            }))
        }
    }
}

pub async fn create(payload: &BearerKeyPayload) -> Result<BearerKey> {
    let id = Uuid::new_v4().to_string();
    // Generate a random 32-byte bearer token
    let token = format!("mcphub_{}", Uuid::new_v4().simple());
    let groups_json = serde_json::to_string(&payload.allowed_groups)?;
    let servers_json = serde_json::to_string(&payload.allowed_servers)?;

    sqlx::query(
        "INSERT INTO bearer_keys (id, name, token, enabled, access_type, allowed_groups, allowed_servers) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&payload.name)
    .bind(&token)
    .bind(payload.enabled as i64)
    .bind(&payload.access_type)
    .bind(&groups_json)
    .bind(&servers_json)
    .execute(db::pool())
    .await?;

    let row = sqlx::query("SELECT created_at FROM bearer_keys WHERE id = ?")
        .bind(&id)
        .fetch_one(db::pool())
        .await?;

    Ok(BearerKey {
        id,
        name: payload.name.clone(),
        token,
        enabled: payload.enabled,
        access_type: payload.access_type.clone(),
        allowed_groups: payload.allowed_groups.clone(),
        allowed_servers: payload.allowed_servers.clone(),
        created_at: row.try_get("created_at")?,
    })
}

pub async fn update(id: &str, payload: &BearerKeyPayload) -> Result<Option<BearerKey>> {
    let groups_json = serde_json::to_string(&payload.allowed_groups)?;
    let servers_json = serde_json::to_string(&payload.allowed_servers)?;

    let affected = sqlx::query(
        "UPDATE bearer_keys SET name = ?, enabled = ?, access_type = ?, allowed_groups = ?, allowed_servers = ? \
         WHERE id = ?",
    )
    .bind(&payload.name)
    .bind(payload.enabled as i64)
    .bind(&payload.access_type)
    .bind(&groups_json)
    .bind(&servers_json)
    .bind(id)
    .execute(db::pool())
    .await?
    .rows_affected();

    if affected == 0 {
        return Ok(None);
    }

    let row = sqlx::query(
        "SELECT id, name, token, enabled, access_type, allowed_groups, allowed_servers, created_at \
         FROM bearer_keys WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(db::pool())
    .await?;

    match row {
        None => Ok(None),
        Some(r) => {
            let enabled: i64 = r.try_get("enabled")?;
            let groups_json: String = r.try_get("allowed_groups")?;
            let servers_json: String = r.try_get("allowed_servers")?;
            Ok(Some(BearerKey {
                id: r.try_get("id")?,
                name: r.try_get("name")?,
                token: r.try_get("token")?,
                enabled: enabled != 0,
                access_type: r.try_get("access_type")?,
                allowed_groups: serde_json::from_str(&groups_json).unwrap_or_default(),
                allowed_servers: serde_json::from_str(&servers_json).unwrap_or_default(),
                created_at: r.try_get("created_at")?,
            }))
        }
    }
}

pub async fn delete(id: &str) -> Result<bool> {
    let affected = sqlx::query("DELETE FROM bearer_keys WHERE id = ?")
        .bind(id)
        .execute(db::pool())
        .await?
        .rows_affected();
    Ok(affected > 0)
}
