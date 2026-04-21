use crate::{db, models::resource::{BuiltinResource, BuiltinResourcePayload}};
use anyhow::Result;
use sqlx::Row;
use uuid::Uuid;

fn row_to_resource(r: &sqlx::sqlite::SqliteRow) -> Result<BuiltinResource> {
    let enabled: i64 = r.try_get("enabled")?;
    Ok(BuiltinResource {
        id: r.try_get("id")?,
        uri: r.try_get("uri")?,
        name: r.try_get("name").ok().flatten(),
        description: r.try_get("description").ok().flatten(),
        mime_type: r.try_get("mime_type").unwrap_or_else(|_| "text/plain".to_string()),
        content: r.try_get("content").unwrap_or_default(),
        enabled: enabled != 0,
        created_at: r.try_get("created_at")?,
    })
}

pub async fn list_all() -> Result<Vec<BuiltinResource>> {
    let rows = sqlx::query(
        "SELECT id, uri, name, description, mime_type, content, enabled, created_at \
         FROM builtin_resources ORDER BY name",
    )
    .fetch_all(db::pool())
    .await?;

    rows.iter().map(row_to_resource).collect()
}

pub async fn find_by_id(id: &str) -> Result<Option<BuiltinResource>> {
    let row = sqlx::query(
        "SELECT id, uri, name, description, mime_type, content, enabled, created_at \
         FROM builtin_resources WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(db::pool())
    .await?;

    match row {
        None => Ok(None),
        Some(r) => Ok(Some(row_to_resource(&r)?)),
    }
}

pub async fn create(payload: &BuiltinResourcePayload) -> Result<BuiltinResource> {
    let id = Uuid::new_v4().to_string();

    sqlx::query(
        "INSERT INTO builtin_resources (id, uri, name, description, mime_type, content, enabled) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&payload.uri)
    .bind(&payload.name)
    .bind(&payload.description)
    .bind(&payload.mime_type)
    .bind(&payload.content)
    .bind(payload.enabled as i64)
    .execute(db::pool())
    .await?;

    let row = sqlx::query(
        "SELECT id, uri, name, description, mime_type, content, enabled, created_at \
         FROM builtin_resources WHERE id = ?",
    )
    .bind(&id)
    .fetch_one(db::pool())
    .await?;

    row_to_resource(&row)
}

pub async fn update(id: &str, payload: &BuiltinResourcePayload) -> Result<Option<BuiltinResource>> {
    let affected = sqlx::query(
        "UPDATE builtin_resources SET uri = ?, name = ?, description = ?, mime_type = ?, \
         content = ?, enabled = ? WHERE id = ?",
    )
    .bind(&payload.uri)
    .bind(&payload.name)
    .bind(&payload.description)
    .bind(&payload.mime_type)
    .bind(&payload.content)
    .bind(payload.enabled as i64)
    .bind(id)
    .execute(db::pool())
    .await?
    .rows_affected();

    if affected == 0 {
        return Ok(None);
    }
    find_by_id(id).await
}

pub async fn delete(id: &str) -> Result<bool> {
    let affected = sqlx::query("DELETE FROM builtin_resources WHERE id = ?")
        .bind(id)
        .execute(db::pool())
        .await?
        .rows_affected();
    Ok(affected > 0)
}
