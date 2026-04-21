use crate::{db, models::prompt::{BuiltinPrompt, BuiltinPromptPayload, PromptArgument}};
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
