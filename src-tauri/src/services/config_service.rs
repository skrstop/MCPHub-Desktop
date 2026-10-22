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

/// Security-relevant read: like `get()`, but a CORRUPT stored config is an
/// error instead of a silent empty object. `get()` returns Ok({}) on
/// corruption to protect the on-disk data (update path), which makes the
/// corrupt-JSON state indistinguishable from "auth disabled" for readers
/// that key off Ok/Err — bit-rot would silently turn off bearer auth and
/// the OAuth provider while the app keeps running. NULL is still an empty
/// config (a normal fresh-install state, not corruption). (Round 12.)
pub async fn get_strict() -> Result<Value> {
    let row = sqlx::query("SELECT config_json FROM system_config WHERE id=1")
        .fetch_one(db::pool())
        .await?;
    let json_str: Option<String> = row.try_get("config_json")?;
    match json_str.as_deref().map(serde_json::from_str::<Value>) {
        Some(Ok(val)) => Ok(val),
        Some(Err(e)) => Err(anyhow::anyhow!("stored config_json failed to parse: {e}")),
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

// Serialize the config-mutating tests: they snapshot/restore the same
// system_config row on the shared file-backed pool, and parallel runs
// race (one test's restore can interleave with another's snapshot,
// producing spurious decode panics — seen twice in full-suite runs).
// Module-level + cfg(test) so other files' tests (e.g. tool_call_guard)
// can share the same serialization.
#[cfg(test)]
pub(crate) static CONFIG_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::CONFIG_TEST_LOCK;
    use super::merge_json;
    use super::{get, get_strict};
    use crate::db;
    use serde_json::json;

    #[test]
    fn merge_json_null_patch_stores_null_does_not_delete_key() {
        // Documented semantics (AGENTS.md §4.5 sync notes): a null patch value
        // is STORED as null rather than removing the key — frontend readers
        // use `!= null` so the distinction is observable.
        let mut current = json!({"routing": {"enableBearerAuth": true, "keep": 1}});
        merge_json(&mut current, &json!({"routing": {"enableBearerAuth": null}}));
        assert_eq!(current["routing"]["enableBearerAuth"], serde_json::Value::Null);
        assert!(current["routing"].get("enableBearerAuth").is_some());
        assert_eq!(current["routing"]["keep"], json!(1));
    }

    #[test]
    fn merge_json_recursive_objects_and_scalar_replace() {
        let mut current = json!({"a": {"b": {"c": 1}}, "s": "old", "n": 1});
        merge_json(
            &mut current,
            &json!({"a": {"b": {"d": 2}}, "s": "new", "n": {"deep": true}}),
        );
        assert_eq!(current["a"]["b"]["c"], json!(1)); // sibling preserved
        assert_eq!(current["a"]["b"]["d"], json!(2)); // recursive add
        assert_eq!(current["s"], json!("new"));       // scalar replace
        assert_eq!(current["n"], json!({"deep": true})); // scalar -> object replace
    }

    #[test]
    fn merge_json_non_object_patch_is_noop_and_missing_key_created() {
        let mut current = json!({"routing": {"x": 1}});
        // Non-object patch (array/number) leaves current untouched.
        merge_json(&mut current, &json!([1, 2, 3]));
        assert_eq!(current, json!({"routing": {"x": 1}}));
        // New top-level key is created (entry.or_insert(Null) path).
        merge_json(&mut current, &json!({"fresh": {"k": "v"}}));
        assert_eq!(current["fresh"]["k"], json!("v"));
    }

    // get_strict semantics against the shared migrated test pool (round 13).
    // NULL row = normal fresh-install state -> Ok({}); corrupt JSON must be a
    // loud Err (bit-rot must not silently disable bearer auth) while get()
    // still returns Ok({}) to protect the update path.
    async fn write_config_json(v: Option<&str>) {
        match v {
            Some(s) => {
                sqlx::query("UPDATE system_config SET config_json=? WHERE id=1")
                    .bind(s)
                    .execute(db::pool())
                    .await
                    .expect("write config_json");
            }
            None => {
                sqlx::query("UPDATE system_config SET config_json=NULL WHERE id=1")
                    .execute(db::pool())
                    .await
                    .expect("clear config_json");
            }
        }
    }

    // Serialize the config-mutating tests: see CONFIG_TEST_LOCK above.
    #[tokio::test]
    async fn get_strict_null_row_is_ok_empty_config() {
        let _g = CONFIG_TEST_LOCK.lock().unwrap();
        crate::db::ensure_test_pool().await;
        let snap: Option<String> = sqlx::query_scalar(
            "SELECT config_json FROM system_config WHERE id=1")
            .fetch_one(db::pool()).await.expect("snapshot");
        write_config_json(None).await;
        let got = get_strict().await.expect("NULL config_json is Ok");
        assert_eq!(got, json!({}));
        // get() agrees: NULL is an empty config, not corruption.
        assert_eq!(get().await.expect("get NULL"), json!({}));
        write_config_json(snap.as_deref()).await;
    }

    #[tokio::test]
    async fn get_strict_valid_json_round_trips() {
        let _g = CONFIG_TEST_LOCK.lock().unwrap();
        crate::db::ensure_test_pool().await;
        let snap: Option<String> = sqlx::query_scalar(
            "SELECT config_json FROM system_config WHERE id=1")
            .fetch_one(db::pool()).await.expect("snapshot");
        write_config_json(Some(r#"{"routing":{"enableBearerAuth":true}}"#)).await;
        let got = get_strict().await.expect("valid config_json is Ok");
        assert_eq!(got["routing"]["enableBearerAuth"], json!(true));
        write_config_json(snap.as_deref()).await;
    }

    #[tokio::test]
    async fn get_strict_corrupt_json_is_err_while_get_falls_back_ok() {
        let _g = CONFIG_TEST_LOCK.lock().unwrap();
        crate::db::ensure_test_pool().await;
        let snap: Option<String> = sqlx::query_scalar(
            "SELECT config_json FROM system_config WHERE id=1")
            .fetch_one(db::pool()).await.expect("snapshot");
        write_config_json(Some("{not json")).await;
        assert!(get_strict().await.is_err(),
                "corrupt config_json must be a loud Err in get_strict");
        // get() keeps its protective Ok({}) fallback (update-path safety).
        assert_eq!(get().await.expect("get corrupt falls back"), json!({}));
        write_config_json(snap.as_deref()).await;
    }

    // Round 15: empty-string config_json is also corruption (serde_json
    // cannot parse ""), so get_strict must Err while get() still falls back
    // to Ok({}).  Guards the same fail-closed contract for the
    // blank-cell/zeroed-value form of bit rot, not just truncated JSON.
    #[tokio::test]
    async fn get_strict_empty_string_is_err_while_get_falls_back_ok() {
        let _g = CONFIG_TEST_LOCK.lock().unwrap();
        crate::db::ensure_test_pool().await;
        let snap: Option<String> = sqlx::query_scalar(
            "SELECT config_json FROM system_config WHERE id=1")
            .fetch_one(db::pool()).await.expect("snapshot");
        write_config_json(Some("")).await;
        assert!(get_strict().await.is_err(),
                "empty-string config_json must be a loud Err in get_strict");
        // get() keeps its protective Ok({}) fallback (update-path safety).
        assert_eq!(get().await.expect("get empty-string falls back"), json!({}));
        write_config_json(snap.as_deref()).await;
    }
}
