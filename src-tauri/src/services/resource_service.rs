use crate::{db, models::resource::{BuiltinResource, BuiltinResourcePayload, ResourcePage}};
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

/// Paginated builtin-resource search: case-insensitive substring on
/// name/description only (uri is an identifier, not search text — empty =
/// all) + enabled filter ("all" | "active" | "inactive"). SQL does the
/// filtering + LIMIT/OFFSET; `page` is 0-based. Backs the Resources page's
/// debounced toolbar search.
pub async fn search_paged(
    search_key: &str,
    filter: &str,
    page: u32,
    page_size: u32,
) -> Result<ResourcePage> {
    let page = page.min(10_000);
    let page_size = page_size.clamp(1, 200);
    let key = search_key.trim().to_lowercase();
    let offset = (page as i64) * (page_size as i64);

    let mut conds: Vec<String> = Vec::new();
    if !key.is_empty() {
        conds.push(
            "(LOWER(COALESCE(name, '')) LIKE ? ESCAPE '\\' OR LOWER(COALESCE(description, '')) LIKE ? ESCAPE '\\')"
                .to_string(),
        );
    }
    match filter {
        "active" => conds.push("enabled = 1".to_string()),
        "inactive" => conds.push("enabled = 0".to_string()),
        _ => {}
    }
    let where_clause = if conds.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conds.join(" AND "))
    };

    let pattern = format!("%{}%", key.replace('%', "\\%").replace('_', "\\_"));
    let count_sql = format!("SELECT COUNT(*) FROM builtin_resources {}", where_clause);
    let mut count_q = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(&*count_sql));
    if !key.is_empty() {
        count_q = count_q.bind(&pattern).bind(&pattern);
    }
    let total: i64 = count_q.fetch_one(db::pool()).await?;

    let data_sql = format!(
        "SELECT id, uri, name, description, mime_type, content, enabled, created_at \
         FROM builtin_resources {} ORDER BY name LIMIT ? OFFSET ?",
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

    let items = rows.iter().map(row_to_resource).collect::<Result<Vec<_>>>()?;
    Ok(ResourcePage {
        items,
        total: total.max(0) as u64,
        page,
        page_size,
    })
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
