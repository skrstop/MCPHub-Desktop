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

    // FTS5 路径（§4.4）：key 非空时优先 FTS（rank 序），enabled 过滤 Rust 侧做；
    // 空表/Err/零结果降级原 LIKE。builtin_resources 行数小，Rust 分页代价可忽略。
    if !key.is_empty() {
        match crate::services::fts_service::search_ref_ids_weighted(
            crate::services::fts_service::FtsTable::Resources,
            &key,
            500,
        )
        .await
        {
            Ok(weighted) if !weighted.is_empty() => {
                let counts: std::collections::HashMap<String, i64> =
                    weighted.into_iter().collect();
                let all = sqlx::query(
                    "SELECT id, uri, name, description, mime_type, content, enabled, created_at \
                     FROM builtin_resources ORDER BY name",
                )
                .fetch_all(db::pool())
                .await?;
                let mut items = all
                    .iter()
                    .map(row_to_resource)
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .filter(|r| {
                        // BuiltinResource.name 可空；ref_id 取的就是 name，NULL 名的资源不入 FTS
                        let id_hit = r.name.as_ref().map(|n| counts.contains_key(n)).unwrap_or(false);
                        id_hit
                            && (filter == "all"
                                || (filter == "active" && r.enabled)
                                || (filter == "inactive" && !r.enabled))
                    })
                    .collect::<Vec<_>>();
                // 相关度第一优先（稳定排序：同命中数保持 name 序）
                items.sort_by_key(|r| {
                    std::cmp::Reverse(
                        r.name
                            .as_ref()
                            .and_then(|n| counts.get(n).copied())
                            .unwrap_or(0),
                    )
                });
                let total = items.len() as u64;
                let window: Vec<BuiltinResource> = items
                    .into_iter()
                    .skip(offset.max(0) as usize)
                    .take(page_size as usize)
                    .collect();
                return Ok(ResourcePage { items: window, total, page, page_size });
            }
            Ok(_) => {
                // 零结果/空表均不返回：落到下方原 LIKE（子串语义补 FTS 词前缀盲区）
            }
            Err(e) => {
                log::warn!("[fts] search resources failed, fallback to LIKE: {e}");
            }
        }
    }

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
    let fts_text = resource_fts_text(
        payload.name.as_deref().unwrap_or(""),
        Some(&payload.uri),
        payload.description.as_deref(),
    );

    let mut tx = db::pool().begin().await?;
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
    .execute(&mut *tx)
    .await?;

    // FTS 同步（§4.3 铁律：同事务；ref_id=name）
    crate::services::fts_service::sync_upsert_tx(
        &mut tx,
        crate::services::fts_service::FtsTable::Resources,
        payload.name.as_deref().unwrap_or(""),
        &fts_text,
    )
    .await?;
    tx.commit().await?;

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
    let fts_text = resource_fts_text(
        payload.name.as_deref().unwrap_or(""),
        Some(&payload.uri),
        payload.description.as_deref(),
    );

    let mut tx = db::pool().begin().await?;
    // 先读旧 name（ref_id=name：改名时需删旧插新，P5）
    let old_name: Option<String> = sqlx::query_scalar::<_, Option<String>>(
        "SELECT name FROM builtin_resources WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .flatten();

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
    .execute(&mut *tx)
    .await?
    .rows_affected();

    if affected == 0 {
        tx.commit().await?;
        return Ok(None);
    }
    if old_name.as_deref() != payload.name.as_deref() {
        if let Some(old) = &old_name {
            crate::services::fts_service::sync_delete_tx(
                &mut tx,
                crate::services::fts_service::FtsTable::Resources,
                old,
            )
            .await?;
        }
    }
    crate::services::fts_service::sync_upsert_tx(
        &mut tx,
        crate::services::fts_service::FtsTable::Resources,
        payload.name.as_deref().unwrap_or(""),
        &fts_text,
    )
    .await?;
    tx.commit().await?;
    find_by_id(id).await
}

pub async fn delete(id: &str) -> Result<bool> {
    let mut tx = db::pool().begin().await?;
    let name: Option<String> = sqlx::query_scalar::<_, Option<String>>(
        "SELECT name FROM builtin_resources WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .flatten();
    let affected = sqlx::query("DELETE FROM builtin_resources WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if let Some(name) = name {
        crate::services::fts_service::sync_delete_tx(
            &mut tx,
            crate::services::fts_service::FtsTable::Resources,
            &name,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(affected > 0)
}

/// resources 的 FTS 可搜索文本（P3）：name + uri + description
fn resource_fts_text(name: &str, uri: Option<&str>, description: Option<&str>) -> String {
    [Some(name), uri, description]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
