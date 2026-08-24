use crate::{db, models::group::{Group, GroupPayload, GroupPage}};
use anyhow::{anyhow, Result};
use serde_json::Value as JsonValue;
use sqlx::Row;
use uuid::Uuid;

fn row_to_group(r: &sqlx::sqlite::SqliteRow) -> Result<Group> {
    let servers_str: String = r.try_get("servers")?;
    let servers: Vec<JsonValue> = serde_json::from_str(&servers_str).unwrap_or_default();
    Ok(Group {
        id: r.try_get("id")?,
        name: r.try_get("name")?,
        description: r.try_get("description").ok().flatten(),
        servers,
        created_at: r.try_get("created_at")?,
    })
}

const SELECT_COLS: &str = "id, name, description, servers, created_at";

pub async fn list_all() -> Result<Vec<Group>> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(&*format!(
        "SELECT {SELECT_COLS} FROM groups ORDER BY name"
    )))
    .fetch_all(db::pool())
    .await?;

    rows.iter().map(row_to_group).collect()
}

/// Paginated group search (case-insensitive substring on name/description,
/// empty key = all). SQL does the filtering (`LOWER(...) LIKE`, served by the
/// name UNIQUE index for ordering) + LIMIT/OFFSET; `page` is 0-based. Backs
/// the ServerForm group dropdown + any group list search.
pub async fn search_paged(search_key: &str, page: u32, page_size: u32) -> Result<GroupPage> {
    let page = page.min(10_000);
    let page_size = page_size.clamp(1, 200);
    let key = search_key.trim().to_lowercase();
    let offset = (page as i64) * (page_size as i64);

    let mut where_clause = String::new();
    let mut pattern = String::new();
    if !key.is_empty() {
        pattern = format!("%{}%", key.replace('%', "\\%").replace('_', "\\_"));
        where_clause = "WHERE LOWER(name) LIKE ? ESCAPE '\\' OR LOWER(COALESCE(description, '')) LIKE ? ESCAPE '\\'".to_string();
    }

    let count_sql = format!("SELECT COUNT(*) FROM groups {}", where_clause);
    let mut count_q = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(&*count_sql));
    if !key.is_empty() {
        count_q = count_q.bind(&pattern).bind(&pattern);
    }
    let total: i64 = count_q.fetch_one(db::pool()).await?;

    let data_sql = format!(
        "SELECT {SELECT_COLS} FROM groups {} ORDER BY name LIMIT ? OFFSET ?",
        where_clause
    );
    let mut data_q = sqlx::query(sqlx::AssertSqlSafe(&*data_sql));
    if !key.is_empty() {
        data_q = data_q.bind(&pattern).bind(&pattern);
    }
    let rows = data_q
        .bind(page_size as i64)
        .bind(offset)
        .fetch_all(db::pool())
        .await?;

    let items = rows.iter().map(row_to_group).collect::<Result<Vec<_>>>()?;
    Ok(GroupPage {
        items,
        total: total.max(0) as u64,
        page,
        page_size,
    })
}

pub async fn find_by_name_or_id(name_or_id: &str) -> Result<Option<Group>> {
    let row = sqlx::query(sqlx::AssertSqlSafe(&*format!(
        "SELECT {SELECT_COLS} FROM groups WHERE name = ? OR id = ?"
    )))
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
    sqlx::query(
        "INSERT INTO groups (id, name, description, servers) \
         VALUES (?, ?, ?, ?)",
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

    let row = sqlx::query(sqlx::AssertSqlSafe(&*format!(
        "SELECT {SELECT_COLS} FROM groups WHERE id=?"
    )))
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
