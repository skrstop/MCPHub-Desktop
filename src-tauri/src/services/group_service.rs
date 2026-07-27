use crate::{db, models::group::{Group, GroupPayload}};
use anyhow::{anyhow, Result};
use serde_json::Value as JsonValue;
use sqlx::Row;
use uuid::Uuid;

/// Decode a builtin-selection JSON value into the stored form.
/// None / "all" / null  → None  (expose all, back-compat)
/// array (incl. [])      → Some(vec)  ([] = expose none; empty stays empty on purpose)
/// NOTE: deliberately diverges from http_server::extract_filter_list, which
/// maps [] → None (= all). Here [] must mean "none".
fn normalize_builtin(v: Option<&JsonValue>) -> Option<Vec<String>> {
    match v {
        None | Some(JsonValue::Null) => None,
        Some(JsonValue::String(s)) if s == "all" => None,
        Some(JsonValue::Array(arr)) => {
            let names: Vec<String> = arr
                .iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect();
            Some(names)
        }
        _ => None,
    }
}

/// Serialize a builtin selection for storage. None → NULL; Some(vec) → JSON text.
fn builtin_to_storage(v: &Option<Vec<String>>) -> Option<String> {
    v.as_ref().map(|names| serde_json::to_string(names).unwrap_or_else(|_| "[]".to_string()))
}

/// Parse a stored builtin-selection column (TEXT, NULL or JSON array) into the model form.
fn builtin_from_storage(raw: Option<String>) -> Option<Vec<String>> {
    let s = raw?;
    serde_json::from_str::<Vec<String>>(&s).ok()
}

fn row_to_group(r: &sqlx::sqlite::SqliteRow) -> Result<Group> {
    let servers_str: String = r.try_get("servers")?;
    let servers: Vec<JsonValue> = serde_json::from_str(&servers_str).unwrap_or_default();
    Ok(Group {
        id: r.try_get("id")?,
        name: r.try_get("name")?,
        description: r.try_get("description").ok().flatten(),
        servers,
        builtin_prompts: builtin_from_storage(r.try_get("builtin_prompts").ok().flatten()),
        builtin_resources: builtin_from_storage(r.try_get("builtin_resources").ok().flatten()),
        created_at: r.try_get("created_at")?,
    })
}

const SELECT_COLS: &str = "id, name, description, servers, builtin_prompts, builtin_resources, created_at";

pub async fn list_all() -> Result<Vec<Group>> {
    let rows = sqlx::query(&format!(
        "SELECT {SELECT_COLS} FROM groups ORDER BY name"
    ))
    .fetch_all(db::pool())
    .await?;

    rows.iter().map(row_to_group).collect()
}

pub async fn find_by_name_or_id(name_or_id: &str) -> Result<Option<Group>> {
    let row = sqlx::query(&format!(
        "SELECT {SELECT_COLS} FROM groups WHERE name = ? OR id = ?"
    ))
    .bind(name_or_id)
    .bind(name_or_id)
    .fetch_optional(db::pool())
    .await?;

    match row {
        None => Ok(None),
        Some(r) => Ok(Some(row_to_group(&r)?)),
    }
}

pub async fn create(payload: &GroupPayload) -> Result<Group> {
    let id = Uuid::new_v4().to_string();
    let servers_json = serde_json::to_string(&payload.servers)?;
    let builtin_prompts = normalize_builtin(payload.builtin_prompts.as_ref());
    let builtin_resources = normalize_builtin(payload.builtin_resources.as_ref());
    let bp_storage = builtin_to_storage(&builtin_prompts);
    let br_storage = builtin_to_storage(&builtin_resources);

    sqlx::query(
        "INSERT INTO groups (id, name, description, servers, builtin_prompts, builtin_resources) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&payload.name)
    .bind(&payload.description)
    .bind(&servers_json)
    .bind(&bp_storage)
    .bind(&br_storage)
    .execute(db::pool())
    .await?;

    Ok(Group {
        id,
        name: payload.name.clone(),
        description: payload.description.clone(),
        servers: payload.servers.clone(),
        builtin_prompts,
        builtin_resources,
        created_at: chrono::Utc::now().to_rfc3339(),
    })
}

pub async fn update(id: &str, payload: &GroupPayload) -> Result<Group> {
    let servers_json = serde_json::to_string(&payload.servers)?;
    let builtin_prompts = normalize_builtin(payload.builtin_prompts.as_ref());
    let builtin_resources = normalize_builtin(payload.builtin_resources.as_ref());
    let bp_storage = builtin_to_storage(&builtin_prompts);
    let br_storage = builtin_to_storage(&builtin_resources);

    sqlx::query(
        "UPDATE groups SET name=?, description=?, servers=?, builtin_prompts=?, builtin_resources=? WHERE id=?",
    )
    .bind(&payload.name)
    .bind(&payload.description)
    .bind(&servers_json)
    .bind(&bp_storage)
    .bind(&br_storage)
    .bind(id)
    .execute(db::pool())
    .await?;

    let row = sqlx::query(&format!(
        "SELECT {SELECT_COLS} FROM groups WHERE id=?"
    ))
    .bind(id)
    .fetch_optional(db::pool())
    .await?
    .ok_or_else(|| anyhow!("Group not found"))?;

    row_to_group(&row)
}

pub async fn delete(id: &str) -> Result<()> {
    sqlx::query("DELETE FROM groups WHERE id=?")
        .bind(id)
        .execute(db::pool())
        .await?;
    Ok(())
}
