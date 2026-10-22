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
        instructions: r.try_get("instructions").ok().flatten(),
        servers,
        created_at: r.try_get("created_at")?,
    })
}

const SELECT_COLS: &str = "id, name, description, instructions, servers, created_at";

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
        pattern = format!(
            "%{}%",
            key.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
        );
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
    let fts_text = group_fts_text(
        &payload.name,
        payload.description.as_deref(),
        payload.instructions.as_deref(),
    );

    let mut tx = db::pool().begin().await?;
    sqlx::query(
        "INSERT INTO groups (id, name, description, instructions, servers) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&payload.name)
    .bind(&payload.description)
    .bind(&payload.instructions)
    .bind(&servers_json)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        if e.to_string().contains("UNIQUE constraint failed") {
            anyhow!("Group name '{}' is already in use", payload.name)
        } else {
            e.into()
        }
    })?;
    crate::services::fts_service::sync_upsert_tx(
        &mut tx,
        crate::services::fts_service::FtsTable::Groups,
        &id,
        &fts_text,
    )
    .await?;
    tx.commit().await?;
    // New group scope exists — notify tools-list subscribers (R119 parity).
    crate::services::subscription_hub::notify_tools_list_changed().await;

    // Re-read from DB instead of fabricating created_at: the column is
    // populated by a DB default whose format/value differs from
    // chrono::Utc::now().to_rfc3339(), so the create response must match
    // what list_all/update return (update() already re-reads this way).
    match find_by_name_or_id(&id).await {
        Ok(Some(g)) => Ok(g),
        _ => Err(anyhow!("group create: row vanished after commit")),
    }
}

pub async fn update(id: &str, payload: &GroupPayload) -> Result<Group> {
    let servers_json = serde_json::to_string(&payload.servers)?;
    let fts_text = group_fts_text(
        &payload.name,
        payload.description.as_deref(),
        payload.instructions.as_deref(),
    );

    let mut tx = db::pool().begin().await?;
    // Capture the old name INSIDE the tx so the bearer-key cascade rewrites
    // the exact stored name (review round 9: allowed_groups is keyed by group
    // NAME; a rename without cascade silently revoked every key referencing
    // the old name).
    let old_name: Option<String> = sqlx::query("SELECT name FROM groups WHERE id=?")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .and_then(|r| r.try_get("name").ok());
    let result = sqlx::query(
        "UPDATE groups SET name=?, description=?, instructions=?, servers=? WHERE id=?",
    )
    .bind(&payload.name)
    .bind(&payload.description)
    .bind(&payload.instructions)
    .bind(&servers_json)
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        if e.to_string().contains("UNIQUE constraint failed") {
            anyhow!("Group name '{}' is already in use", payload.name)
        } else {
            e.into()
        }
    })?;
    if result.rows_affected() == 0 {
        return Err(anyhow!("Group not found"));
    }
    if let Some(old) = old_name.filter(|o| o != &payload.name) {
        cascade_bearer_groups_rename_tx(&mut tx, &old, &payload.name).await?;
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
    // Group membership/name changes reshape the /mcp/{group} and
    // $smart/{group} exposed tool set — tell subscribers the list changed
    // (R119: same parity as server rename/delete notification).
    crate::services::subscription_hub::notify_tools_list_changed().await;

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
    // Cascade bearer keys BEFORE the row disappears (review round 9: same
    // name-keyed storage as allowed_servers — a delete without cascade left
    // dangling group names in allowed_groups).
    let name: Option<String> = sqlx::query("SELECT name FROM groups WHERE id=?")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .and_then(|r| r.try_get("name").ok());
    if let Some(name) = name {
        cascade_bearer_groups_remove_tx(&mut tx, &name).await?;
    }
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
    // Exposed tool set of the group scope disappeared — notify subscribers.
    crate::services::subscription_hub::notify_tools_list_changed().await;
    Ok(())
}

/// groups 的 FTS 可搜索文本（P3）：name + description + instructions
fn group_fts_text(name: &str, description: Option<&str>, instructions: Option<&str>) -> String {
    let mut parts = vec![name.to_string()];
    if let Some(d) = description.filter(|d| !d.is_empty()) {
        parts.push(d.to_string());
    }
    if let Some(i) = instructions.filter(|i| !i.is_empty()) {
        parts.push(i.to_string());
    }
    parts.join(" ")
}

/// Rewrite `old_name` → `new_name` in every bearer key's `allowed_groups`
/// (group rename). Mirrors server_service's allowed_servers cascade. Must run
/// inside the caller's transaction.
async fn cascade_bearer_groups_rename_tx(
    tx: &mut sqlx::SqliteConnection,
    old_name: &str,
    new_name: &str,
) -> Result<()> {
    let rows = sqlx::query("SELECT id, allowed_groups FROM bearer_keys")
        .fetch_all(&mut *tx)
        .await?;
    for row in rows {
        let id: String = row.try_get("id")?;
        let groups_str: String = row.try_get("allowed_groups")?;
        let mut groups: Vec<String> = serde_json::from_str(&groups_str).unwrap_or_default();
        if !groups.iter().any(|g| g == old_name) {
            continue;
        }
        for g in groups.iter_mut() {
            if g == old_name {
                *g = new_name.to_string();
            }
        }
        sqlx::query("UPDATE bearer_keys SET allowed_groups=? WHERE id=?")
            .bind(serde_json::to_string(&groups).unwrap_or_else(|_| "[]".into()))
            .bind(&id)
            .execute(&mut *tx)
            .await?;
    }
    Ok(())
}

/// Remove `name` from every bearer key's `allowed_groups` (group delete).
/// Must run inside the caller's transaction.
async fn cascade_bearer_groups_remove_tx(
    tx: &mut sqlx::SqliteConnection,
    name: &str,
) -> Result<()> {
    let rows = sqlx::query("SELECT id, allowed_groups FROM bearer_keys")
        .fetch_all(&mut *tx)
        .await?;
    for row in rows {
        let id: String = row.try_get("id")?;
        let groups_str: String = row.try_get("allowed_groups")?;
        let mut groups: Vec<String> = serde_json::from_str(&groups_str).unwrap_or_default();
        let before = groups.len();
        groups.retain(|g| g != name);
        if groups.len() == before {
            continue;
        }
        sqlx::query("UPDATE bearer_keys SET allowed_groups=? WHERE id=?")
            .bind(serde_json::to_string(&groups).unwrap_or_else(|_| "[]".into()))
            .bind(&id)
            .execute(&mut *tx)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn payload(name: &str, instructions: Option<&str>) -> GroupPayload {
        GroupPayload {
            name: name.to_string(),
            description: Some("test group".to_string()),
            instructions: instructions.map(|s| s.to_string()),
            servers: vec![json!({"name": "srv-a", "tools": "all"})],
        }
    }

    /// #1340 parity: instructions must round-trip through create/update,
    /// feed the FTS searchable text, and clear back to None.
    #[tokio::test]
    async fn instructions_round_trip_update_and_fts() {
        crate::db::ensure_test_pool().await;
        let name = format!("sv_grp_{}", uuid::Uuid::new_v4().simple());
        let token = format!("svinstr{}", uuid::Uuid::new_v4().simple());

        // Create with instructions.
        let created = create(&payload(&name, Some(&format!("Always {} first", token))))
            .await
            .unwrap();
        assert_eq!(created.instructions.as_deref(), Some(format!("Always {} first", token).as_str()));

        // Read back via find_by_name_or_id (by id and by name).
        let by_id = find_by_name_or_id(&created.id).await.unwrap().unwrap();
        assert_eq!(by_id.instructions.as_deref(), created.instructions.as_deref());
        let by_name = find_by_name_or_id(&name).await.unwrap().unwrap();
        assert_eq!(by_name.instructions.as_deref(), created.instructions.as_deref());

        // Instructions feed the FTS text: searching the unique token finds the group.
        let page = search_paged(&token, 0, 10).await.unwrap();
        assert!(
            page.items.iter().any(|g| g.id == created.id),
            "unique instruction token must be FTS-searchable"
        );

        // Update rotates the instructions.
        let new_token = format!("svinstr{}", uuid::Uuid::new_v4().simple());
        let mut p = payload(&name, Some(&new_token));
        p.description = created.description.clone();
        let updated = update(&created.id, &p).await.unwrap();
        assert_eq!(updated.instructions.as_deref(), Some(new_token.as_str()));
        let page2 = search_paged(&new_token, 0, 10).await.unwrap();
        assert!(page2.items.iter().any(|g| g.id == created.id));

        // Update to None clears the field.
        let mut p2 = payload(&name, None);
        p2.description = created.description.clone();
        let cleared = update(&created.id, &p2).await.unwrap();
        assert!(cleared.instructions.is_none());

        delete(&created.id).await.unwrap();
        assert!(find_by_name_or_id(&created.id).await.unwrap().is_none());
    }

    /// Create without instructions → None (column default path).
    #[tokio::test]
    async fn create_without_instructions_is_none() {
        crate::db::ensure_test_pool().await;
        let name = format!("sv_grp_{}", uuid::Uuid::new_v4().simple());
        let created = create(&payload(&name, None)).await.unwrap();
        assert!(created.instructions.is_none());
        delete(&created.id).await.unwrap();
    }
}
