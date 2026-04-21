use crate::db;
use anyhow::Result;
use serde_json::Value;
use sqlx::Row;

pub async fn get() -> Result<Value> {
    let row = sqlx::query("SELECT config_json FROM system_config WHERE id=1")
        .fetch_one(db::pool())
        .await?;
    let json_str: Option<String> = row.try_get("config_json")?;
    let val: Value = json_str
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| Value::Object(Default::default()));
    Ok(val)
}

/// Deep-merge `patch` into `current`: object keys are merged recursively,
/// non-object values are replaced.
fn merge_json(current: &mut Value, patch: &Value) {
    if let (Value::Object(base_map), Value::Object(patch_map)) = (current, patch) {
        for (k, v) in patch_map {
            let entry = base_map.entry(k.clone()).or_insert(Value::Null);
            if v.is_object() && entry.is_object() {
                merge_json(entry, v);
            } else {
                *entry = v.clone();
            }
        }
    }
}

/// Merge `patch` into the stored config (partial update) and return the full config.
pub async fn update(patch: &Value) -> Result<Value> {
    let mut current = get().await?;
    merge_json(&mut current, patch);
    let json_str = serde_json::to_string(&current)?;
    sqlx::query(
        "UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE id=1",
    )
    .bind(&json_str)
    .execute(db::pool())
    .await?;
    Ok(current)
}
