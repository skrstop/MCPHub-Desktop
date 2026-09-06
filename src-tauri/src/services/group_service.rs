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

    // FTS5 路径（中/英/拼音分词 + rank 排序）；空表/Err 降级原 LIKE（§4.4）
    if !key.is_empty() {
        match crate::services::fts_service::search_ref_ids_weighted(
            crate::services::fts_service::FtsTable::Groups,
            &key,
            1000,
        )
        .await
        {
            Ok(weighted) if !weighted.is_empty() => {
                // groups 数量小：全量读（原生 name 序）→ 稳定排序按命中词数降序 → Rust 侧分页
                let rows = sqlx::query(sqlx::AssertSqlSafe(&*format!(
                    "SELECT {SELECT_COLS} FROM groups ORDER BY name"
                )))
                .fetch_all(db::pool())
                .await?;
                let counts: std::collections::HashMap<String, i64> =
                    weighted.into_iter().collect();
                let mut items = rows
                    .iter()
                    .map(row_to_group)
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .filter(|g| counts.contains_key(&g.id))
                    .collect::<Vec<_>>();
                // 相关度第一优先（稳定排序：同命中数保持 name 序）
                items.sort_by_key(|g| std::cmp::Reverse(counts.get(&g.id).copied().unwrap_or(0)));
                let total = items.len() as u64;
                let window = items
                    .iter()
                    .skip(offset.max(0) as usize)
                    .take(page_size as usize)
                    .cloned()
                    .collect();
                return Ok(GroupPage {
                    items: window,
                    total,
                    page,
                    page_size,
                });
            }
            Ok(_)
                if crate::services::fts_service::table_is_empty(
                    crate::services::fts_service::FtsTable::Groups,
                )
                .await
                .unwrap_or(false) =>
            {
                // 空表兜底：继续走下方原 LIKE
            }
            Ok(_) => {
                // 零结果/空表均不返回：落到下方原 LIKE（子串语义补 FTS 词前缀盲区）
            }
            Err(e) => {
                log::warn!("[fts] search groups failed, fallback to LIKE: {e}");
            }
        }
    }

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

pub async fn find_by_name_or_id(name_or_id: &str) -> Result<Option<Group>> {    let row = sqlx::query(sqlx::AssertSqlSafe(&*format!(
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
    let fts_text = group_fts_text(&payload.name, payload.description.as_deref());

    let mut tx = db::pool().begin().await?;
    sqlx::query(
        "INSERT INTO groups (id, name, description, servers) \
         VALUES (?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&payload.name)
    .bind(&payload.description)
    .bind(&servers_json)
    .execute(&mut *tx)
    .await?;
    crate::services::fts_service::sync_upsert_tx(
        &mut tx,
        crate::services::fts_service::FtsTable::Groups,
        &id,
        &fts_text,
    )
    .await?;
    tx.commit().await?;

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
    let fts_text = group_fts_text(&payload.name, payload.description.as_deref());

    let mut tx = db::pool().begin().await?;
    let result = sqlx::query(
        "UPDATE groups SET name=?, description=?, servers=? WHERE id=?",
    )
    .bind(&payload.name)
    .bind(&payload.description)
    .bind(&servers_json)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() == 0 {
        return Err(anyhow!("Group not found"));
    }
    // ref_id=id，改名不影响 ref_id
    crate::services::fts_service::sync_upsert_tx(
        &mut tx,
        crate::services::fts_service::FtsTable::Groups,
        id,
        &fts_text,
    )
    .await?;
    tx.commit().await?;

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
    let mut tx = db::pool().begin().await?;
    sqlx::query("DELETE FROM groups WHERE id=?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    crate::services::fts_service::sync_delete_tx(
        &mut tx,
        crate::services::fts_service::FtsTable::Groups,
        id,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// groups 的 FTS 可搜索文本（P3）：name + description
fn group_fts_text(name: &str, description: Option<&str>) -> String {
    match description {
        Some(d) if !d.is_empty() => format!("{name} {d}"),
        _ => name.to_string(),
    }
}
