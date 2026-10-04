use crate::db;
use anyhow::Result;
use serde_json::Value;
use sqlx::Row;

/// Millisecond timestamp for backup filenames (no chrono dep at this layer).
fn chrono_like_stamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

pub async fn get() -> Result<Value> {
    let row = sqlx::query("SELECT config_json FROM system_config WHERE id=1")
        .fetch_one(db::pool())
        .await?;
    let json_str: Option<String> = row.try_get("config_json")?;
    match json_str.as_deref().map(serde_json::from_str::<Value>) {
        Some(Ok(val)) => Ok(val),
        // Corrupt stored config: DO NOT silently fall back to {} — a later
        // update() would overwrite the on-disk data with an empty object and
        // the original content would be unrecoverable. Persist a backup copy
        // beside the app data dir and log loudly instead.
        Some(Err(e)) => {
            let raw = json_str.unwrap_or_default();
            match crate::services::runtime_env::app_data_dir("backups") {
                Some(dir) => {
                    // Timestamped name: repeated corrupt states must not
                    // overwrite a previous backup.
                    let path = dir.join(format!(
                        "system_config.corrupt.{}.json",
                        chrono_like_stamp()
                    ));
                    match std::fs::write(&path, raw.as_bytes()) {
                        Ok(()) => log::error!(
                            "[config] stored config_json failed to parse ({e}); raw content backed up to {}",
                            path.display()
                        ),
                        Err(werr) => log::error!(
                            "[config] stored config_json failed to parse ({e}); backup write failed ({werr}); raw head: {:?}",
                            &raw.chars().take(400).collect::<String>()
                        ),
                    }
                }
                None => log::error!(
                    "[config] stored config_json failed to parse ({e}); no backup dir; raw head: {:?}",
                    &raw.chars().take(400).collect::<String>()
                ),
            }
            Ok(Value::Object(Default::default()))
        }
        None => Ok(Value::Object(Default::default())),
    }
}

/// Check if headless mode is enabled (disables built-in Web UI)
pub async fn is_headless() -> bool {
    get()
        .await
        .ok()
        .and_then(|c| {
            c.get("routing")
                .and_then(|r| r.get("headless"))
                .and_then(|v| v.as_bool())
        })
        .unwrap_or(false)
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

fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Merge `patch` into the stored config (partial update) and return the full config.
/// Runs SELECT → merge → UPDATE inside a single `BEGIN IMMEDIATE` transaction so
/// concurrent writers serialize (a plain read-then-write race would silently drop
/// one side's patch).
pub async fn update(patch: &Value) -> Result<Value> {
    let mut conn = db::pool().acquire().await?;
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
    let result: Result<Value> = async {
        let row = sqlx::query("SELECT config_json FROM system_config WHERE id=1")
            .fetch_one(&mut *conn)
            .await?;
        let json_str: Option<String> = row.try_get("config_json")?;
        // Corrupt stored config must abort the update (fail-closed) — merging
        // into an empty object would overwrite the on-disk data irrecoverably.
        let mut current: Value = match json_str.as_deref().map(serde_json::from_str::<Value>) {
            Some(Ok(v)) => v,
            Some(Err(e)) => return Err(anyhow::anyhow!("stored config_json is corrupt: {e}")),
            None => Value::Object(Default::default()),
        };
        // A stored config that parses but is NOT an object (e.g. `true`/`[]`
        // written by a past bug) would make merge_json a no-op: update()
        // would report success while silently dropping the entire patch.
        // Fail closed like the corrupt path above.
        if !current.is_object() {
            return Err(anyhow::anyhow!(
                "stored config_json is not a JSON object (type: {}); refusing to merge patch",
                json_type_name(&current)
            ));
        }
        // Symmetry with the stored-config check: a non-object patch (array /
        // scalar) would make merge_json a silent no-op while update() reported
        // success — fail closed instead (review round 9).
        if !patch.is_object() {
            return Err(anyhow::anyhow!(
                "config patch must be a JSON object (got type: {})",
                json_type_name(patch)
            ));
        }
        merge_json(&mut current, patch);
        let merged = serde_json::to_string(&current)?;
        sqlx::query(
            "UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE id=1",
        )
        .bind(&merged)
        .execute(&mut *conn)
        .await?;
        Ok(current)
    }
    .await;
    match result {
        Ok(current) => {
            sqlx::query("COMMIT").execute(&mut *conn).await?;
            Ok(current)
        }
        Err(e) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            Err(e)
        }
    }
}
