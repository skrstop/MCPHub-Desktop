use crate::{db, models::prompt::{BuiltinPrompt, BuiltinPromptPayload, PromptArgument, PromptPage}};
use anyhow::Result;
use sqlx::Row;
use uuid::Uuid;

fn row_to_prompt(r: &sqlx::sqlite::SqliteRow) -> Result<BuiltinPrompt> {
    let enabled: i64 = r.try_get("enabled")?;
    let args_json: Option<String> = r.try_get("arguments").ok();
    let arguments: Vec<PromptArgument> = args_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    Ok(BuiltinPrompt {
        id: r.try_get("id")?,
        name: r.try_get("name")?,
        title: r.try_get("title").ok().flatten(),
        description: r.try_get("description").ok().flatten(),
        template: r.try_get("template").unwrap_or_default(),
        arguments,
        enabled: enabled != 0,
        created_at: r.try_get("created_at")?,
    })
}

pub async fn list_all() -> Result<Vec<BuiltinPrompt>> {
    let rows = sqlx::query(
        "SELECT id, name, title, description, template, arguments, enabled, created_at \
         FROM builtin_prompts ORDER BY name",
    )
    .fetch_all(db::pool())
    .await?;

    rows.iter().map(row_to_prompt).collect()
}

/// Paginated builtin-prompt search: case-insensitive substring on
/// name/title/description (empty = all) + enabled filter
/// ("all" | "active" | "inactive"). SQL does the filtering + LIMIT/OFFSET;
/// `page` is 0-based. Backs the Prompts page's debounced toolbar search.
pub async fn search_paged(
    search_key: &str,
    filter: &str,
    page: u32,
    page_size: u32,
) -> Result<PromptPage> {
    let page = page.min(10_000);
    let page_size = page_size.clamp(1, 200);
    let key = search_key.trim().to_lowercase();
    let offset = (page as i64) * (page_size as i64);

    // FTS5 路径（§4.4）：key 非空时优先 FTS（rank 序），enabled 过滤 Rust 侧做；
    // 空表/Err 降级原 LIKE。builtin_prompts 行数小，Rust 分页代价可忽略。
    if !key.is_empty() {
        match crate::services::fts_service::search_ref_ids_weighted(
            crate::services::fts_service::FtsTable::Prompts,
            &key,
            500,
        )
        .await
        {
            Ok(weighted) if !weighted.is_empty() => {
                // 命中词数 map：相关度第一优先，同数保持 name 序（稳定排序）
                let all = sqlx::query(
                    "SELECT id, name, title, description, template, arguments, enabled, created_at \
                     FROM builtin_prompts ORDER BY name",
                )
                .fetch_all(db::pool())
                .await?;
                let counts: std::collections::HashMap<String, i64> =
                    weighted.iter().cloned().collect();
                let mut items = all
                    .iter()
                    .map(row_to_prompt)
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .filter(|p| {
                        counts.contains_key(&p.name)
                            && (filter == "all"
                                || (filter == "active" && p.enabled)
                                || (filter == "inactive" && !p.enabled))
                    })
                    .collect::<Vec<_>>();
                // 相关度第一优先（稳定排序：同命中数保持 name 序）
                items.sort_by_key(|p| std::cmp::Reverse(counts.get(&p.name).copied().unwrap_or(0)));
                let total = items.len() as u64;
                let window: Vec<BuiltinPrompt> = items
                    .into_iter()
                    .skip(offset.max(0) as usize)
                    .take(page_size as usize)
                    .collect();
                return Ok(PromptPage { items: window, total, page, page_size });
            }
            Ok(_)
                if crate::services::fts_service::table_is_empty(
                    crate::services::fts_service::FtsTable::Prompts,
                )
                .await
                .unwrap_or(false) =>
            {
                // 空表兜底：走原 LIKE
            }
            Ok(_) => {
                // 零结果/空表均不返回：落到下方原 LIKE（子串语义补 FTS 词前缀盲区）
            }
            Err(e) => {
                log::warn!("[fts] search prompts failed, fallback to LIKE: {e}");
            }
        }
    }

    // FTS5 路径（§4.4）：key 非空时优先 FTS（rank 序），enabled 过滤在 Rust 侧；
    // 空表/Err 降级原 LIKE。builtin 表行数小，Rust 分页代价可忽略。
    if !key.is_empty() {
        match crate::services::fts_service::search_ref_ids_weighted(
            crate::services::fts_service::FtsTable::Prompts,
            &key,
            500,
        )
        .await
        {
            Ok(weighted) if !weighted.is_empty() => {
                // 命中词数 map：相关度第一优先，同数保持 name 序（稳定排序）
                let all = sqlx::query(
                    "SELECT id, name, title, description, template, arguments, enabled, created_at \
                     FROM builtin_prompts ORDER BY name",
                )
                .fetch_all(db::pool())
                .await?;
                let counts: std::collections::HashMap<String, i64> =
                    weighted.iter().cloned().collect();
                let mut items = all
                    .iter()
                    .map(row_to_prompt)
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .filter(|p| {
                        counts.contains_key(&p.name)
                            && match filter {
                                "active" => p.enabled,
                                "inactive" => !p.enabled,
                                _ => true,
                            }
                    })
                    .collect::<Vec<_>>();
                // 相关度第一优先（稳定排序：同命中数保持 name 序）
                items.sort_by_key(|p| std::cmp::Reverse(counts.get(&p.name).copied().unwrap_or(0)));
                let total = items.len() as u64;
                let window = items
                    .into_iter()
                    .skip(offset.max(0) as usize)
                    .take(page_size as usize)
                    .collect();
                return Ok(PromptPage { items: window, total, page, page_size });
            }
            Ok(_)
                if crate::services::fts_service::table_is_empty(
                    crate::services::fts_service::FtsTable::Prompts,
                )
                .await
                .unwrap_or(false) =>
            {
                // 空表兜底：走原 LIKE
            }
            Ok(_) => {
                // 零结果/空表均不返回：落到下方原 LIKE（子串语义补 FTS 词前缀盲区）
            }
            Err(e) => {
                log::warn!("[fts] search prompts failed, fallback to LIKE: {e}");
            }
        }
    }

    let mut conds: Vec<String> = Vec::new();
    if !key.is_empty() {
        conds.push(
            "(LOWER(name) LIKE ? ESCAPE '\\' OR LOWER(COALESCE(title, '')) LIKE ? ESCAPE '\\' OR LOWER(COALESCE(description, '')) LIKE ? ESCAPE '\\')"
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
    let count_sql = format!("SELECT COUNT(*) FROM builtin_prompts {}", where_clause);
    let mut count_q = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(&*count_sql));
    if !key.is_empty() {
        count_q = count_q.bind(&pattern).bind(&pattern).bind(&pattern);
    }
    let total: i64 = count_q.fetch_one(db::pool()).await?;

    let data_sql = format!(
        "SELECT id, name, title, description, template, arguments, enabled, created_at \
         FROM builtin_prompts {} ORDER BY name LIMIT ? OFFSET ?",
        where_clause
    );
    let mut data_q = sqlx::query(sqlx::AssertSqlSafe(&*data_sql));
    if !key.is_empty() {
        data_q = data_q.bind(&pattern).bind(&pattern).bind(&pattern);
    }
    let rows = data_q
        .bind(page_size as i64)
        .bind(offset)
        .fetch_all(db::pool())
        .await?;

    let items = rows.iter().map(row_to_prompt).collect::<Result<Vec<_>>>()?;
    Ok(PromptPage {
        items,
        total: total.max(0) as u64,
        page,
        page_size,
    })
}

pub async fn find_by_id(id: &str) -> Result<Option<BuiltinPrompt>> {
    let row = sqlx::query(
        "SELECT id, name, title, description, template, arguments, enabled, created_at \
         FROM builtin_prompts WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(db::pool())
    .await?;

    match row {
        None => Ok(None),
        Some(r) => Ok(Some(row_to_prompt(&r)?)),
    }
}

pub async fn create(payload: &BuiltinPromptPayload) -> Result<BuiltinPrompt> {
    let id = Uuid::new_v4().to_string();
    let args_json = serde_json::to_string(&payload.arguments)?;
    let fts_text = prompt_fts_text(&payload.name, payload.title.as_deref(), payload.description.as_deref());

    let mut tx = db::pool().begin().await?;
    sqlx::query(
        "INSERT INTO builtin_prompts (id, name, title, description, template, arguments, enabled) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&payload.name)
    .bind(&payload.title)
    .bind(&payload.description)
    .bind(&payload.template)
    .bind(&args_json)
    .bind(payload.enabled as i64)
    .execute(&mut *tx)
    .await?;

    // FTS 同步（§4.3 铁律：同事务；ref_id=name）
    crate::services::fts_service::sync_upsert_tx(
        &mut tx,
        crate::services::fts_service::FtsTable::Prompts,
        &payload.name,
        &fts_text,
    )
    .await?;
    tx.commit().await?;

    let row = sqlx::query(
        "SELECT id, name, title, description, template, arguments, enabled, created_at \
         FROM builtin_prompts WHERE id = ?",
    )
    .bind(&id)
    .fetch_one(db::pool())
    .await?;

    row_to_prompt(&row)
}

pub async fn update(id: &str, payload: &BuiltinPromptPayload) -> Result<Option<BuiltinPrompt>> {
    let args_json = serde_json::to_string(&payload.arguments)?;
    let fts_text = prompt_fts_text(&payload.name, payload.title.as_deref(), payload.description.as_deref());

    let mut tx = db::pool().begin().await?;
    // 先读旧 name（ref_id=name：改名时需删旧插新，P5）
    let old_name: Option<String> = sqlx::query_scalar::<_, Option<String>>(
        "SELECT name FROM builtin_prompts WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .flatten();

    let affected = sqlx::query(
        "UPDATE builtin_prompts SET name = ?, title = ?, description = ?, template = ?, \
         arguments = ?, enabled = ? WHERE id = ?",
    )
    .bind(&payload.name)
    .bind(&payload.title)
    .bind(&payload.description)
    .bind(&payload.template)
    .bind(&args_json)
    .bind(payload.enabled as i64)
    .bind(id)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    if affected == 0 {
        tx.commit().await?;
        return Ok(None);
    }
    if old_name.as_deref() != Some(payload.name.as_str()) {
        if let Some(old) = &old_name {
            crate::services::fts_service::sync_delete_tx(
                &mut tx,
                crate::services::fts_service::FtsTable::Prompts,
                old,
            )
            .await?;
        }
    }
    crate::services::fts_service::sync_upsert_tx(
        &mut tx,
        crate::services::fts_service::FtsTable::Prompts,
        &payload.name,
        &fts_text,
    )
    .await?;
    tx.commit().await?;

    find_by_id(id).await
}

pub async fn delete(id: &str) -> Result<bool> {
    let mut tx = db::pool().begin().await?;
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM builtin_prompts WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .flatten();
    let affected = sqlx::query("DELETE FROM builtin_prompts WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if let Some(name) = name {
        crate::services::fts_service::sync_delete_tx(
            &mut tx,
            crate::services::fts_service::FtsTable::Prompts,
            &name,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(affected > 0)
}

/// prompts 的 FTS 可搜索文本（P3）：name + title + description
fn prompt_fts_text(name: &str, title: Option<&str>, description: Option<&str>) -> String {
    [Some(name), title, description]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Render the prompt template by substituting {{arg}} placeholders with provided values.
pub fn render_template(template: &str, args: &serde_json::Value) -> String {
    let mut result = template.to_string();
    if let Some(obj) = args.as_object() {
        for (k, v) in obj {
            let placeholder = format!("{{{{{}}}}}", k);
            let replacement = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            result = result.replace(&placeholder, &replacement);
        }
    }
    result
}
