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
    .execute(db::pool())
    .await?;

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
    .execute(db::pool())
    .await?
    .rows_affected();

    if affected == 0 {
        return Ok(None);
    }
    find_by_id(id).await
}

pub async fn delete(id: &str) -> Result<bool> {
    let affected = sqlx::query("DELETE FROM builtin_prompts WHERE id = ?")
        .bind(id)
        .execute(db::pool())
        .await?
        .rows_affected();
    Ok(affected > 0)
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
