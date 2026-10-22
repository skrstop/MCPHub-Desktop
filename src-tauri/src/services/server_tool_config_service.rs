use crate::db;
use crate::models::server::Tool;
use crate::models::server_tool_config::{ServerToolConfig, ServerToolConfigPayload};
use anyhow::Result;
use sqlx::Row;
use std::collections::HashMap;
use uuid::Uuid;

const SELECT_COLS: &str = "id, server_name, item_type, item_name, enabled, description, pinned, hidden_params, created_at, updated_at";

fn row_to_config(r: &sqlx::sqlite::SqliteRow) -> ServerToolConfig {
    ServerToolConfig {
        id: r.try_get("id").unwrap_or_default(),
        server_name: r.try_get("server_name").unwrap_or_default(),
        item_type: r.try_get("item_type").unwrap_or_default(),
        item_name: r.try_get("item_name").unwrap_or_default(),
        enabled: r.try_get::<i64, _>("enabled").unwrap_or(1) != 0,
        description: r.try_get("description").ok(),
        pinned: r.try_get::<i64, _>("pinned").unwrap_or(0) != 0,
        hidden_params: r
            .try_get::<Option<String>, _>("hidden_params")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok()),
        created_at: r.try_get("created_at").unwrap_or_default(),
        updated_at: r.try_get("updated_at").unwrap_or_default(),
    }
}

pub async fn get_config(
    server_name: &str,
    item_type: &str,
    item_name: &str,
) -> Result<Option<ServerToolConfig>> {
    let row = sqlx::query(sqlx::AssertSqlSafe(&*format!(
        "SELECT {SELECT_COLS}
         FROM server_tool_config
         WHERE server_name=? AND item_type=? AND item_name=?"
    )))
    .bind(server_name)
    .bind(item_type)
    .bind(item_name)
    .fetch_optional(db::pool())
    .await?;

    Ok(row.map(|r| row_to_config(&r)))
}

/// True when ANY tool description override exists — a cheap single-row
/// probe used to skip the per-server live-list refresh on the smart search
/// hot path when no override can change results (round 10 review).
pub async fn any_description_override() -> bool {
    sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM server_tool_config
         WHERE item_type='tool' AND description IS NOT NULL AND description != ''
         LIMIT 1",
    )
    .fetch_optional(db::pool())
    .await
    .map(|o| o.is_some())
    .unwrap_or(false)
}

/// True when ANY tool has configured hidden parameters — a cheap single-row
/// probe mirroring any_description_override, used to keep the smart-search
/// hot path from skipping the live-list refresh when schemas must change.
pub async fn any_hidden_params() -> bool {
    sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM server_tool_config
         WHERE item_type='tool' AND hidden_params IS NOT NULL AND hidden_params != '[]'
         LIMIT 1",
    )
    .fetch_optional(db::pool())
    .await
    .map(|o| o.is_some())
    .unwrap_or(false)
}

/// List all overrides for a given server (and optionally item_type).
pub async fn list_for_server(
    server_name: &str,
    item_type: Option<&str>,
) -> Result<Vec<ServerToolConfig>> {
    let rows = if let Some(t) = item_type {
        sqlx::query(sqlx::AssertSqlSafe(&*format!(
            "SELECT {SELECT_COLS}
             FROM server_tool_config WHERE server_name=? AND item_type=?"
        )))
        .bind(server_name)
        .bind(t)
        .fetch_all(db::pool())
        .await?
    } else {
        sqlx::query(sqlx::AssertSqlSafe(&*format!(
            "SELECT {SELECT_COLS}
             FROM server_tool_config WHERE server_name=?"
        )))
        .bind(server_name)
        .fetch_all(db::pool())
        .await?
    };

    Ok(rows.iter().map(row_to_config).collect())
}

/// Upsert an override (insert or update on conflict).
pub async fn upsert(p: &ServerToolConfigPayload) -> Result<ServerToolConfig> {
    // Read failure degrades to None (treat as changed) rather than failing
    // the whole write — `previous` only drives change-detection for the
    // list_changed notification (review round 9).
    let previous = get_config(&p.server_name, &p.item_type, &p.item_name).await.ok().flatten();
    let id = Uuid::new_v4().to_string();
    let enabled_i: i64 = if p.enabled { 1 } else { 0 };
    let pinned_i: i64 = if p.pinned.unwrap_or(false) { 1 } else { 0 };
    // `pinned` is intentionally absent from the DO UPDATE clause: toggle /
    // description writes (payload.pinned = None) must preserve the existing
    // pin; only set_pinned (Some) writes it, via its own statement below.
    sqlx::query(
        "INSERT INTO server_tool_config (id, server_name, item_type, item_name, enabled, description, pinned)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(server_name, item_type, item_name) DO UPDATE SET
           enabled=excluded.enabled,
           description=CASE WHEN excluded.description IS NULL THEN server_tool_config.description ELSE excluded.description END,
           updated_at=datetime('now')",
    )
    .bind(&id)
    .bind(&p.server_name)
    .bind(&p.item_type)
    .bind(&p.item_name)
    .bind(enabled_i)
    .bind(&p.description)
    .bind(pinned_i)
    .execute(db::pool())
    .await?;

    let out = get_config(&p.server_name, &p.item_type, &p.item_name)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Failed to fetch after upsert"));
    // A3: tool enable/disable changes the exposed tool list — but only notify
    // when the write changed something (same-value overwrite would trigger a
    // pointless client re-list; review round 8, 2026-10-04).
    let changed = previous.as_ref().map(|prev| {
        prev.enabled != p.enabled
            // description=None means "keep current" (SQL CASE WHEN preserve
            // semantics) — only an explicit Some value can change it.
            || p.description.as_ref().map_or(false, |d| Some(d) != prev.description.as_ref())
    }).unwrap_or(true);
    if changed {
        crate::services::subscription_hub::notify_tools_list_changed().await;
    }
    out
}

/// Update only the description of an item.
pub async fn update_description(
    server_name: &str,
    item_type: &str,
    item_name: &str,
    description: Option<&str>,
) -> Result<()> {
    // If no override exists yet, insert one (enabled by default)
    // Read failure degrades to None (treat as changed) — see upsert (round 9).
    let previous = get_config(server_name, item_type, item_name).await.ok().flatten();
    // Unconditional upsert (not read-then-insert): two concurrent calls for
    // the same (server, type, item) both see "not exists" and race the INSERT
    // — the loser fails the UNIQUE constraint. Same handling as `upsert`.
    sqlx::query(
        "INSERT INTO server_tool_config (id, server_name, item_type, item_name, enabled, description)
         VALUES (?, ?, ?, ?, 1, ?)
         ON CONFLICT(server_name, item_type, item_name)
         DO UPDATE SET description=excluded.description, updated_at=datetime('now')",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(server_name)
    .bind(item_type)
    .bind(item_name)
    .bind(description)
    .execute(db::pool())
    .await?;
    // Description overrides flow through apply_tool_filters into the exposed
    // tools/list — notify only when the value actually changed.
    let changed = previous
        .as_ref()
        .map(|prev| prev.description != description.map(|d| d.to_string()))
        .unwrap_or(true);
    if changed {
        crate::services::subscription_hub::notify_tools_list_changed().await;
    }
    Ok(())
}

/// Reset description to NULL (remove override).
pub async fn reset_description(server_name: &str, item_type: &str, item_name: &str) -> Result<()> {
    // Read failure degrades to None (treat as changed) — see upsert (round 9).
    let previous = get_config(server_name, item_type, item_name).await.ok().flatten();
    sqlx::query(
        "UPDATE server_tool_config SET description=NULL, updated_at=datetime('now')
         WHERE server_name=? AND item_type=? AND item_name=?",
    )
    .bind(server_name)
    .bind(item_type)
    .bind(item_name)
    .execute(db::pool())
    .await?;
    // Same exposed-list change as update_description — notify only when a row
    // was actually reset (0 rows / already-NULL reset is a no-op).
    let changed = previous
        .as_ref()
        .map(|prev| prev.description.is_some())
        .unwrap_or(false);
    if changed {
        crate::services::subscription_hub::notify_tools_list_changed().await;
    }
    Ok(())
}

/// Set the server-level $smart pin for an item (tools only in practice).
pub async fn set_pinned(
    server_name: &str,
    item_type: &str,
    item_name: &str,
    pinned: bool,
) -> Result<()> {
    let previous = match get_config(server_name, item_type, item_name).await {
        Ok(p) => p,
        // A transient read failure must not silently turn unpin into a
        // reported success (the row may exist with pinned=1 — the early
        // return below would skip the write and the caller would show
        // "unpinned" while $smart keeps exposing the tool). Unpin propagates
        // the real error; pin can proceed (upsert creates the row).
        Err(e) if !pinned => return Err(e),
        Err(_) => None,
    };
    // Unpinning a tool that never had an override must not CREATE a dead row
    // (R5-1): the command layer frames unpin as dead-row cleanup, so an
    // INSERT here would be the only producer of spurious override rows.
    if previous.is_none() && !pinned {
        return Ok(());
    }
    // Upsert keeps enabled/description untouched when the row exists.
    sqlx::query(
        "INSERT INTO server_tool_config (id, server_name, item_type, item_name, enabled, description, pinned)
         VALUES (?, ?, ?, ?, 1, NULL, ?)
         ON CONFLICT(server_name, item_type, item_name)
         DO UPDATE SET pinned=excluded.pinned, updated_at=datetime('now')",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(server_name)
    .bind(item_type)
    .bind(item_name)
    .bind(if pinned { 1i64 } else { 0i64 })
    .execute(db::pool())
    .await?;
    // Pin changes the exposed tools/list on $smart endpoints — notify only on
    // an actual flip (same-value writes are no-ops).
    let changed = previous
        .as_ref()
        .map(|prev| prev.pinned != pinned)
        .unwrap_or(pinned);
    if changed && item_type == "tool" {
        crate::services::subscription_hub::notify_tools_list_changed().await;
    }
    Ok(())
}

/// Server-level pinned tool names (item_type='tool', pinned=1). Ordered by
/// creation time; same-second pins fall back to alphabetical item_name
/// (deterministic, but not strictly insertion order).
pub async fn list_pinned_tools(server_name: &str) -> Result<Vec<String>> {
    let rows = sqlx::query(
        "SELECT item_name FROM server_tool_config
         WHERE server_name=? AND item_type='tool' AND pinned=1
         ORDER BY created_at, item_name",
    )
    .bind(server_name)
    .fetch_all(db::pool())
    .await?;
    Ok(rows
        .iter()
        .filter_map(|r| r.try_get::<String, _>("item_name").ok())
        .collect())
}

/// Set the top-level input parameters of a tool that models do not see
/// (#1343 parity). An empty list removes the setting. The stored list is
/// deduplicated (order preserved); required-parameter filtering happens at
/// read time against the live schema, so a stored name that later becomes
/// required simply stops taking effect instead of corrupting the setting.
/// Returns the effective stored list.
pub async fn update_hidden_params(
    server_name: &str,
    item_name: &str,
    params: Vec<String>,
) -> Result<Vec<String>> {
    let previous = get_config(server_name, "tool", item_name).await.ok().flatten();
    // Dedupe, preserve first-occurrence order.
    let mut seen = std::collections::HashSet::new();
    let deduped: Vec<String> = params.into_iter().filter(|p| seen.insert(p.clone())).collect();
    let stored: Option<Vec<String>> =
        if deduped.is_empty() { None } else { Some(deduped) };
    let stored_json = stored
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".into()));

    // Preserve enabled/description/pinned: only this column is touched.
    sqlx::query(
        "INSERT INTO server_tool_config (id, server_name, item_type, item_name, enabled, description, hidden_params)
         VALUES (?, ?, ?, ?, 1, NULL, ?)
         ON CONFLICT(server_name, item_type, item_name)
         DO UPDATE SET hidden_params=excluded.hidden_params, updated_at=datetime('now')",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(server_name)
    .bind("tool")
    .bind(item_name)
    .bind(&stored_json)
    .execute(db::pool())
    .await?;

    let changed = previous
        .as_ref()
        .map(|prev| prev.hidden_params != stored)
        .unwrap_or(true);
    if changed {
        crate::services::subscription_hub::notify_tools_list_changed().await;
    }
    Ok(stored.unwrap_or_default())
}

/// The configured hidden names that take effect: required ones never do
/// (origin `getEffectiveHiddenParams` parity).
pub fn effective_hidden_params(schema: Option<&serde_json::Value>, hidden: &[String]) -> Vec<String> {
    if hidden.is_empty() {
        return vec![];
    }
    let required: Vec<&serde_json::Value> = schema
        .and_then(|s| s.get("required"))
        .and_then(|r| r.as_array())
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    hidden
        .iter()
        .filter(|n| !required.iter().any(|r| r.as_str() == Some(n.as_str())))
        .cloned()
        .collect()
}

/// Remove the hidden top-level properties from an input schema in place
/// (origin `hideSchemaParams` parity). No-op when nothing is hidden or the
/// schema has no properties object.
pub fn hide_schema_params(schema: &mut serde_json::Value, hidden: &[String]) {
    if hidden.is_empty() {
        return;
    }
    if let Some(props) = schema.get_mut("properties").and_then(|p| p.as_object_mut()) {
        for name in hidden {
            props.remove(name);
        }
    }
}

/// Apply a tool's configured hidden parameters to its projection:
/// `tool.hidden_params` reports the CONFIGURED list (dashboard checkbox
/// state), while the schema has the EFFECTIVE list removed (required names
/// stay visible). Origin `applyHiddenParams` + projection parity.
pub fn apply_hidden_params_to_tool(tool: &mut Tool, configured: &[String]) {
    if configured.is_empty() {
        return;
    }
    tool.hidden_params = Some(configured.to_vec());
    let effective = effective_hidden_params(Some(&tool.input_schema), configured);
    hide_schema_params(&mut tool.input_schema, &effective);
}

/// Call arguments without the tool's hidden parameters, so a model that
/// guesses one cannot reach it; the upstream default applies instead
/// (origin `removeHiddenArguments` parity). Looks up the configured list by
/// bare tool name and the live schema from the shared pool cache; a missing
/// config, an empty list, or an unknown tool returns the arguments as-is.
pub async fn strip_hidden_arguments(
    server_name: &str,
    tool_name: &str,
    args: serde_json::Value,
) -> serde_json::Value {
    let configured = get_config(server_name, "tool", tool_name)
        .await
        .ok()
        .flatten()
        .and_then(|c| c.hidden_params)
        .unwrap_or_default();
    let schema = crate::mcp::pool::cached_tool_schema(server_name, tool_name).await;
    let effective = effective_hidden_params(schema.as_ref(), &configured);
    remove_hidden_arguments(args, &effective)
}

/// Pure core of [`strip_hidden_arguments`]: drop the effective hidden names
/// from object-shaped arguments; non-object arguments pass through untouched.
pub fn remove_hidden_arguments(args: serde_json::Value, effective: &[String]) -> serde_json::Value {
    if effective.is_empty() {
        return args;
    }
    match args {
        serde_json::Value::Object(map) => {
            let mut map = map;
            for name in effective {
                map.remove(name);
            }
            serde_json::Value::Object(map)
        }
        other => other,
    }
}

/// Filter and apply description overrides to a server's tool list.
/// Tools disabled via server_tool_config are excluded; description overrides are applied.
pub async fn apply_tool_filters(server_name: &str, tools: Vec<Tool>) -> Result<Vec<Tool>> {
    if tools.is_empty() {
        return Ok(tools);
    }
    let configs = list_for_server(server_name, Some("tool")).await?;
    if configs.is_empty() {
        return Ok(tools); // No overrides — return as-is
    }
    let config_map: HashMap<String, &ServerToolConfig> =
        configs.iter().map(|c| (c.item_name.clone(), c)).collect();

    Ok(tools
        .into_iter()
        .map(|mut tool| {
            if let Some(cfg) = config_map.get(&tool.name) {
                tool.enabled = cfg.enabled;
                // An explicit empty-string override is treated as "no
                // override" — matching any_description_override's probe so
                // the hot-path skip can't serve results that disagree with
                // this filter (round 12 review).
                if let Some(desc) = cfg.description.as_ref().filter(|d| !d.is_empty()) {
                    tool.description = Some(desc.clone());
                    // Mark for the smart toolset hash: override edits must
                    // trigger a re-embed (origin #1198 parity).
                    tool.description_overridden = true;
                }
                // #1343: hidden parameters — schema models receive loses the
                // effective names; the projection reports the configured list.
                if let Some(h) = cfg.hidden_params.as_ref().filter(|h| !h.is_empty()) {
                    apply_hidden_params_to_tool(&mut tool, h);
                }
            }
            tool
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::server::Tool;
    use serde_json::json;

    fn tool(name: &str, schema: serde_json::Value) -> Tool {
        Tool {
            name: name.to_string(),
            description: None,
            input_schema: schema,
            server_name: "srv".into(),
            enabled: true,
            annotations: None,
            output_schema: None,
            title: None,
            execution: None,
            icons: None,
            meta: None,
            hidden_params: None,
            description_overridden: false,
        }
    }

    #[test]
    fn effective_hidden_filters_required_and_empty() {
        let schema = json!({"properties": {"a": {}, "b": {}}, "required": ["a"]});
        // Required names never take effect.
        assert_eq!(
            effective_hidden_params(Some(&schema), &["a".into(), "b".into()]),
            vec!["b".to_string()]
        );
        // No required array → everything takes effect.
        let schema2 = json!({"properties": {"a": {}}});
        assert_eq!(
            effective_hidden_params(Some(&schema2), &["a".into()]),
            vec!["a".to_string()]
        );
        // Empty configured list → nothing.
        assert!(effective_hidden_params(Some(&schema), &[]).is_empty());
        // Missing schema → everything takes effect (store as given).
        assert_eq!(
            effective_hidden_params(None, &["x".into()]),
            vec!["x".to_string()]
        );
    }

    #[test]
    fn hide_schema_params_removes_only_top_level_properties() {
        let mut schema = json!({
            "properties": {"a": {}, "b": {"nested": {"a": {}}}},
            "required": ["a"]
        });
        hide_schema_params(&mut schema, &["a".into(), "ghost".into()]);
        assert!(schema["properties"].get("a").is_none());
        // Nested same-name objects are untouched.
        assert!(schema["properties"]["b"]["nested"].get("a").is_some());
        // required array is untouched (visibility filtering is read-time).
        assert_eq!(schema["required"], json!(["a"]));
        // No properties object → no-op, no panic.
        let mut bare = json!({"type": "object"});
        hide_schema_params(&mut bare, &["a".into()]);
        assert_eq!(bare, json!({"type": "object"}));
        // Empty hidden → no-op.
        let mut s2 = json!({"properties": {"a": {}}});
        hide_schema_params(&mut s2, &[]);
        assert!(s2["properties"].get("a").is_some());
    }

    #[test]
    fn apply_hidden_params_to_tool_reports_configured_hides_effective() {
        let schema = json!({"properties": {"a": {}, "b": {}}, "required": ["a"]});
        let mut t = tool("scrape", schema);
        apply_hidden_params_to_tool(&mut t, &["a".into(), "b".into()]);
        // Projection reports the CONFIGURED list (dashboard checkbox state).
        assert_eq!(t.hidden_params, Some(vec!["a".into(), "b".into()]));
        // Schema loses only the EFFECTIVE name (required `a` stays visible).
        assert!(t.input_schema["properties"].get("a").is_some());
        assert!(t.input_schema["properties"].get("b").is_none());
        // Empty configured list leaves everything alone.
        let mut t2 = tool("x", json!({"properties": {"a": {}}}));
        apply_hidden_params_to_tool(&mut t2, &[]);
        assert!(t2.hidden_params.is_none());
        assert!(t2.input_schema["properties"].get("a").is_some());
    }

    #[tokio::test]
    async fn update_hidden_params_round_trip_dedupe_and_clear() {
        crate::db::ensure_test_pool().await;
        let server = format!("sv_hp_{}", uuid::Uuid::new_v4().simple());
        let item = format!("tool_{}", uuid::Uuid::new_v4().simple());

        // Seed a row with a description override (must be preserved).
        update_description(&server, "tool", &item, Some("user override"))
            .await
            .unwrap();

        // Set with duplicates → deduped, order preserved.
        let stored = update_hidden_params(&server, &item, vec!["b".into(), "a".into(), "b".into()])
            .await
            .unwrap();
        assert_eq!(stored, vec!["b".to_string(), "a".to_string()]);
        let cfg = get_config(&server, "tool", &item).await.unwrap().unwrap();
        assert_eq!(cfg.hidden_params, Some(vec!["b".into(), "a".into()]));
        // Description override survived the hidden-params write.
        assert_eq!(cfg.description.as_deref(), Some("user override"));

        // Probe agrees.
        assert!(any_hidden_params().await);

        // Empty list clears the setting (NULL), other fields intact.
        let cleared = update_hidden_params(&server, &item, vec![]).await.unwrap();
        assert!(cleared.is_empty());
        let cfg2 = get_config(&server, "tool", &item).await.unwrap().unwrap();
        assert!(cfg2.hidden_params.is_none());
        assert_eq!(cfg2.description.as_deref(), Some("user override"));

        // apply_tool_filters applies the projection end-to-end.
        update_hidden_params(&server, &item, vec!["b".into()]).await.unwrap();
        let mut t = tool(&item, json!({"properties": {"a": {}, "b": {}}}));
        t.server_name = server.clone();
        let out = apply_tool_filters(&server, vec![t]).await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].hidden_params, Some(vec!["b".into()]));
        assert!(out[0].input_schema["properties"].get("b").is_none());
        assert!(out[0].input_schema["properties"].get("a").is_some());

        // Cleanup.
        sqlx::query("DELETE FROM server_tool_config WHERE server_name=?")
            .bind(&server)
            .execute(db::pool())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn strip_hidden_arguments_noop_paths() {
        crate::db::ensure_test_pool().await;
        let server = format!("sv_hp_{}", uuid::Uuid::new_v4().simple());
        let args = json!({"a": 1, "b": 2});

        // No config row → as-is.
        let out = strip_hidden_arguments(&server, "t", args.clone()).await;
        assert_eq!(out, args);

        // Config with empty list → as-is.
        update_hidden_params(&server, "t", vec![]).await.unwrap();
        let out = strip_hidden_arguments(&server, "t", args.clone()).await;
        assert_eq!(out, args);

        // Config with names but no pool entry (unknown schema) → effective
        // list is the configured list; keys are still dropped (store-as-given
        // semantics for disconnected servers).
        update_hidden_params(&server, "t", vec!["a".into()]).await.unwrap();
        let out = strip_hidden_arguments(&server, "t", args.clone()).await;
        assert_eq!(out, json!({"b": 2}));

        // Non-object arguments pass through untouched.
        let arr = json!([1, 2]);
        let out = strip_hidden_arguments(&server, "t", arr.clone()).await;
        assert_eq!(out, arr);

        // Pure core: empty effective list is a no-op; object strip drops only
        // the named keys.
        assert_eq!(remove_hidden_arguments(args.clone(), &[]), args);
        assert_eq!(
            remove_hidden_arguments(args.clone(), &["a".into()]),
            json!({"b": 2})
        );
        assert_eq!(remove_hidden_arguments(arr.clone(), &["a".into()]), arr);

        sqlx::query("DELETE FROM server_tool_config WHERE server_name=?")
            .bind(&server)
            .execute(db::pool())
            .await
            .unwrap();
    }
}
