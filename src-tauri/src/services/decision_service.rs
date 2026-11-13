//! decision_service — the decision-support (决策支持) backend: enable flag,
//! model selection, structured decide + request-history persistence, history
//! cleanup, and the builtin MCP tool exposure on the "mcphub-desktop" server.
//!
//! Mirrors the RAG service's shape: an AtomicBool enable flag (sync reads from
//! the builtin tool listing paths), config-persisted state
//! (`decision.enabled` / `decision.model`), and a single dispatch entry point
//! consumed by the shared builtin-tool paths.

use crate::db;
use crate::mv::decision::{DecisionModel, DecisionOutput, DecisionRequest};
use crate::models::server::{Tool, ToolCallResult};
use anyhow::{anyhow, Result};
use chrono::Local;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::AppHandle;
use tokio::sync::Mutex;

/// Sync enable flag — mirrors `rag::service::RAG_ACTIVE`. The builtin tool
/// listing paths are sync, so the flag is kept in memory and synced from the
/// persisted config at startup and on every toggle.
static DECISION_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Process-wide cached model (168 MB GGUF — load once, reuse across calls).
/// Keyed by model size name; a selection change reloads on next use.
static DECISION_MODEL: Mutex<Option<(String, Arc<DecisionModel>)>> = Mutex::const_new(None);

pub fn is_enabled() -> bool {
    DECISION_ACTIVE.load(Ordering::SeqCst)
}

/// Sync the in-memory flag from the persisted config (`decision.enabled`,
/// default false). Called at startup and after every toggle.
pub async fn sync_enabled_flag() {
    let enabled = crate::services::config_service::get()
        .await
        .ok()
        .and_then(|c| {
            c.get("decision")
                .and_then(|d| d.get("enabled"))
                .and_then(|v| v.as_bool())
        })
        .unwrap_or(false);
    DECISION_ACTIVE.store(enabled, Ordering::SeqCst);
}

/// Toggle the decision runtime. Persisted first; the in-memory flag follows
/// only on a successful persist (fail-closed on config errors).
pub async fn set_enabled(enabled: bool) -> Result<()> {
    let patch = serde_json::json!({ "decision": { "enabled": enabled } });
    crate::services::config_service::update(&patch).await?;
    DECISION_ACTIVE.store(enabled, Ordering::SeqCst);
    crate::rag::service::rag_log(
        "info",
        format!("decision support {}", if enabled { "enabled" } else { "disabled" }),
    );
    Ok(())
}

/// The selected decision model size (`decision.model`), falling back to the
/// out-of-box default ready size when unset/gone.
pub async fn selected_model(app: &AppHandle) -> Option<String> {
    crate::mv::models::current_decision_model()
        .await
        .or_else(|| crate::mv::models::default_decision_size(app))
}

/// Load (or fetch from cache) the selected decision model.
async fn cached_model(app: &AppHandle, name: &str) -> Result<Arc<DecisionModel>> {
    let mut guard = DECISION_MODEL.lock().await;
    if let Some((cached, m)) = guard.as_ref() {
        if cached == name {
            return Ok(m.clone());
        }
    }
    let path = DecisionModel::resolve_model_path(app, name)?;
    let device = candle_core::Device::Cpu;
    let model = tokio::task::spawn_blocking({
        let path = path.clone();
        move || -> Result<DecisionModel> {
            let mut file = std::fs::File::open(&path)?;
            let dir = path
                .parent()
                .ok_or_else(|| anyhow!("model path has no parent"))?
                .to_path_buf();
            DecisionModel::load(&mut file, &dir, &device)
        }
    })
    .await
    .map_err(|e| anyhow!("join load task: {e}"))??;
    let arc = Arc::new(model);
    *guard = Some((name.to_string(), arc.clone()));
    Ok(arc)
}

/// Evict the cached model (selection change / model file replaced).
pub async fn invalidate_model_cache() {
    *DECISION_MODEL.lock().await = None;
}

// ── history ─────────────────────────────────────────────────────────────────

/// One recorded decision request (row of `decision_history`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionHistoryEntry {
    pub id: String,
    pub created_at: String,
    pub decision_type: String,
    pub state: Option<String>,
    pub question: String,
    pub options: Vec<String>,
    pub scores: Vec<f64>,
    pub probabilities: Vec<f64>,
    pub selected_index: i64,
    pub selected_option: String,
    pub duration_ms: Option<i64>,
    pub source: Option<String>,
}

fn parse_json_string_array(raw: &str) -> Vec<String> {
    serde_json::from_str(raw).unwrap_or_default()
}

fn parse_json_f64_array(raw: &str) -> Vec<f64> {
    serde_json::from_str(raw).unwrap_or_default()
}

fn row_to_entry(
    id: String,
    created_at: String,
    decision_type: String,
    state: Option<String>,
    question: String,
    options_json: String,
    scores_json: String,
    probabilities_json: String,
    selected_index: i64,
    selected_option: String,
    duration_ms: Option<i64>,
    source: Option<String>,
) -> DecisionHistoryEntry {
    DecisionHistoryEntry {
        id,
        created_at,
        decision_type,
        state,
        question,
        options: parse_json_string_array(&options_json),
        scores: parse_json_f64_array(&scores_json),
        probabilities: parse_json_f64_array(&probabilities_json),
        selected_index,
        selected_option,
        duration_ms,
        source,
    }
}

/// Run one structured decision against the selected model and record the
/// request + result into the history table. `source` labels the caller
/// ("mcp" for the builtin tool path, "ui" for the Tauri command).
pub async fn decide_and_record(
    app: &AppHandle,
    req: DecisionRequest,
    source: &str,
) -> Result<DecisionOutput> {
    if !is_enabled() {
        return Err(anyhow!("decision support is not enabled"));
    }
    let model = selected_model(app)
        .await
        .ok_or_else(|| anyhow!("no decision model available"))?;
    let m = cached_model(app, &model).await?;
    let start = std::time::Instant::now();
    let req_for_task = req.clone();
    let out = tokio::task::spawn_blocking(move || m.decide(&req_for_task))
        .await
        .map_err(|e| anyhow!("join decide task: {e}"))??;
    let duration_ms = start.elapsed().as_millis() as i64;
    record_history(&req, &out, duration_ms, source).await;
    Ok(out)
}

/// Insert one history row (best-effort — a history write failure must not
/// fail the decision itself; the failure is logged).
pub async fn record_history(
    req: &DecisionRequest,
    out: &DecisionOutput,
    duration_ms: i64,
    source: &str,
) {
    let id = uuid::Uuid::new_v4().to_string();
    let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let options = serde_json::to_string(&req.options).unwrap_or_else(|_| "[]".into());
    let scores = serde_json::to_string(&out.scores).unwrap_or_else(|_| "[]".into());
    let probs = serde_json::to_string(&out.probabilities).unwrap_or_else(|_| "[]".into());
    let r = sqlx::query(
        "INSERT INTO decision_history (id, created_at, decision_type, state, question, \
         options_json, scores_json, probabilities_json, selected_index, selected_option, \
         duration_ms, source) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&now)
    .bind(out.decision_type.as_str())
    .bind(&req.state)
    .bind(&req.question)
    .bind(&options)
    .bind(&scores)
    .bind(&probs)
    .bind(out.index as i64)
    .bind(&out.option)
    .bind(duration_ms)
    .bind(source)
    .execute(db::pool())
    .await;
    if let Err(e) = r {
        crate::rag::service::rag_log("warn", format!("failed to record decision history: {e}"));
    }
}

/// History rows, newest first.
pub async fn list_history(limit: i64, offset: i64) -> Result<Vec<DecisionHistoryEntry>> {
    let limit = limit.clamp(1, 500);
    let offset = offset.max(0);
    let rows = sqlx::query_as::<_, (
        String,
        String,
        String,
        Option<String>,
        String,
        String,
        String,
        String,
        i64,
        String,
        Option<i64>,
        Option<String>,
    )>(
        "SELECT id, created_at, decision_type, state, question, options_json, scores_json, \
         probabilities_json, selected_index, selected_option, duration_ms, source \
         FROM decision_history ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(db::pool())
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, created_at, decision_type, state, question, options_json, scores_json, probabilities_json, selected_index, selected_option, duration_ms, source)| {
                row_to_entry(
                    id,
                    created_at,
                    decision_type,
                    state,
                    question,
                    options_json,
                    scores_json,
                    probabilities_json,
                    selected_index,
                    selected_option,
                    duration_ms,
                    source,
                )
            },
        )
        .collect())
}

/// One history row by id.
pub async fn get_history(id: &str) -> Result<Option<DecisionHistoryEntry>> {
    let rows = sqlx::query_as::<_, (
        String,
        String,
        String,
        Option<String>,
        String,
        String,
        String,
        String,
        i64,
        String,
        Option<i64>,
        Option<String>,
    )>(
        "SELECT id, created_at, decision_type, state, question, options_json, scores_json, \
         probabilities_json, selected_index, selected_option, duration_ms, source \
         FROM decision_history WHERE id = ?",
    )
    .bind(id)
    .fetch_all(db::pool())
    .await?;
    Ok(rows.into_iter().next().map(
        |(id, created_at, decision_type, state, question, options_json, scores_json, probabilities_json, selected_index, selected_option, duration_ms, source)| {
            row_to_entry(
                id,
                created_at,
                decision_type,
                state,
                question,
                options_json,
                scores_json,
                probabilities_json,
                selected_index,
                selected_option,
                duration_ms,
                source,
            )
        },
    ))
}

/// Delete ALL history rows. Returns the number removed.
pub async fn clear_history() -> Result<u64> {
    let r = sqlx::query("DELETE FROM decision_history")
        .execute(db::pool())
        .await?;
    Ok(r.rows_affected())
}

/// Delete history rows older than `days` (global periodic cleanup hook).
/// Returns the number removed.
pub async fn cleanup_history(days: i64) -> Result<u64> {
    let days = days.clamp(1, 3650);
    let cutoff = (Local::now() - chrono::Duration::days(days))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let r = sqlx::query("DELETE FROM decision_history WHERE created_at < ?")
        .bind(&cutoff)
        .execute(db::pool())
        .await?;
    if r.rows_affected() > 0 {
        crate::rag::service::rag_log(
            "info",
            format!("cleaned {} decision history rows older than {days}d", r.rows_affected()),
        );
    }
    Ok(r.rows_affected())
}

// ── builtin MCP tool exposure ───────────────────────────────────────────────

/// The decision tool as a proper `Tool` on the builtin "mcphub-desktop"
/// server. Empty when decision support is disabled (the caller gates on
/// `is_enabled()`).
pub fn builtin_decision_tools() -> Vec<Tool> {
    if !is_enabled() {
        return Vec::new();
    }
    vec![Tool {
        hidden_params: None,
        name: "decision_decide".to_string(),
        description: Some(
            "Run a structured decision with the local decision model: given a question type, \
             context state, question and 2-20 candidate options, returns per-option scores, \
             probabilities and the selected option."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "decisionType": {
                    "type": "string",
                    "enum": ["choice", "score", "noul"],
                    "description": "Question type: choice (pick one), score (rate each), noul (choice with a no-answer option pair [false, true])."
                },
                "state": { "type": "string", "description": "Context the decision is grounded in (text or JSON string)." },
                "question": { "type": "string", "description": "The question to decide." },
                "options": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 2,
                    "maxItems": 20,
                    "description": "2-20 non-empty candidate descriptions."
                }
            },
            "required": ["state", "question", "options"]
        }),
        server_name: crate::rag::service::BUILTIN_SERVER_NAME.to_string(),
        enabled: true,
        annotations: None,
        output_schema: None,
        title: None,
        execution: None,
        icons: None,
        meta: None,
        description_overridden: false,
    }]
}

/// Parse + validate the tool arguments (pure — unit-testable without an app
/// handle). Unknown tool names are rejected here too so the dispatch entry
/// can fail fast before touching the model.
pub fn parse_decision_tool_args(
    tool_name: &str,
    args: &serde_json::Value,
) -> Result<DecisionRequest> {
    if tool_name != "decision_decide" {
        return Err(anyhow!("unknown decision tool: {}", tool_name));
    }
    let req: DecisionRequest = serde_json::from_value(args.clone())
        .map_err(|e| anyhow!("invalid arguments: {e}"))?;
    // Mirror the schema contract (minItems/maxItems/non-empty) and the
    // model's own sequence() validation — fail fast with a precise error
    // instead of a late model-level failure.
    if req.options.len() < 2 || req.options.len() > 20 {
        return Err(anyhow!(
            "options must contain 2-20 entries (got {})",
            req.options.len()
        ));
    }
    if req.options.iter().any(|o| o.is_empty()) {
        return Err(anyhow!("options must be non-empty"));
    }
    Ok(req)
}

/// Dispatch a builtin decision tool call. Gated on the enable flag (fail
/// closed with a clear message). Returns the structured decision as pretty
/// JSON text content.
pub async fn call_builtin_decision_tool(
    app: &AppHandle,
    tool_name: &str,
    args: &serde_json::Value,
) -> Result<ToolCallResult> {
    let req = parse_decision_tool_args(tool_name, args)?;
    if !is_enabled() {
        return Err(anyhow!("decision support is not enabled"));
    }
    let out = decide_and_record(app, req, "mcp").await?;
    let text = serde_json::to_string_pretty(&out).unwrap_or_default();
    Ok(ToolCallResult {
        raw_meta: None,
        content: vec![serde_json::json!({ "type": "text", "text": text })],
        is_error: false,
        structured_content: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mv::decision::DecisionType;

    /// Serializes every test that touches the process-global DECISION_ACTIVE
    /// flag or the shared decision_history table — parallel `cargo test`
    /// otherwise races on both (flag flips + DELETE/INSERT interleavings).
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        match TEST_LOCK.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    #[test]
    fn decision_tools_gated_on_flag() {
        let _g = test_lock();
        // Flag defaults off → no tools.
        DECISION_ACTIVE.store(false, Ordering::SeqCst);
        assert!(builtin_decision_tools().is_empty());
        DECISION_ACTIVE.store(true, Ordering::SeqCst);
        let tools = builtin_decision_tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "decision_decide");
        assert_eq!(tools[0].server_name, crate::rag::service::BUILTIN_SERVER_NAME);
        assert!(tools[0].input_schema.get("required").is_some());
        DECISION_ACTIVE.store(false, Ordering::SeqCst);
    }

    #[test]
    fn parse_decision_tool_args_rejects_out_of_range_and_empty_options() {
        let _g = test_lock();
        let base = |opts: serde_json::Value| {
            serde_json::json!({ "state": "s", "question": "q", "options": opts })
        };
        // Too few / empty
        assert!(parse_decision_tool_args("decision_decide", &base(serde_json::json!(["a"]))).is_err());
        assert!(parse_decision_tool_args("decision_decide", &base(serde_json::json!([]))).is_err());
        // Too many (21)
        let twenty_one: Vec<String> = (0..21).map(|i| format!("o{i}")).collect();
        assert!(parse_decision_tool_args("decision_decide", &base(serde_json::json!(twenty_one))).is_err());
        // Boundary 20 accepted
        let twenty: Vec<String> = (0..20).map(|i| format!("o{i}")).collect();
        assert!(parse_decision_tool_args("decision_decide", &base(serde_json::json!(twenty))).is_ok());
        // Empty option string rejected
        assert!(parse_decision_tool_args("decision_decide", &base(serde_json::json!(["a", ""]))).is_err());
        // Wrong types
        assert!(parse_decision_tool_args("decision_decide", &base(serde_json::json!([1, 2]))).is_err());
        assert!(parse_decision_tool_args("decision_decide", &base(serde_json::json!("ab"))).is_err());
        assert!(parse_decision_tool_args(
            "decision_decide",
            &serde_json::json!({"state": 1, "question": "q", "options": ["a","b"]})
        )
        .is_err());
    }

    #[tokio::test]
    async fn cleanup_history_cutoff_boundary_and_day_clamp() {
        let _g = test_lock();
        db::ensure_test_pool().await;
        let pool = db::pool();
        sqlx::query("DELETE FROM decision_history").execute(pool).await.unwrap();
        let now = Local::now();
        let fmt = |t: chrono::DateTime<Local>| t.format("%Y-%m-%d %H:%M:%S").to_string();
        let cutoff = fmt(now - chrono::Duration::days(1));
        for (id, ts) in [
            ("at", cutoff.clone()),
            ("old", fmt(now - chrono::Duration::days(1) - chrono::Duration::seconds(1))),
            ("new", fmt(now)),
        ] {
            sqlx::query("INSERT INTO decision_history (id, created_at, decision_type, question, options_json, scores_json, probabilities_json, selected_index, selected_option) VALUES (?, ?, 'choice', 'q', '[]', '[]', '[]', 0, 'x')")
                .bind(id)
                .bind(ts)
                .execute(pool)
                .await
                .unwrap();
        }
        let n = cleanup_history(1).await.unwrap();
        assert_eq!(n, 1, "only the strictly-older row is deleted");
        let remaining: Vec<(String,)> = sqlx::query_as("SELECT id FROM decision_history")
            .fetch_all(pool)
            .await
            .unwrap();
        let remaining: Vec<String> = remaining.into_iter().map(|(id,)| id).collect();
        assert!(remaining.contains(&"at".to_string()), "row exactly at cutoff must survive (< not <=)");
        assert!(remaining.contains(&"new".to_string()));
        // days=0 / negative clamp to 1 → nothing new deleted.
        assert_eq!(cleanup_history(0).await.unwrap(), 0);
        assert_eq!(cleanup_history(-5).await.unwrap(), 0);
        sqlx::query("DELETE FROM decision_history").execute(pool).await.unwrap();
    }

    #[tokio::test]
    async fn list_history_clamps_zero_negative_limit_and_negative_offset() {
        let _g = test_lock();
        db::ensure_test_pool().await;
        let pool = db::pool();
        sqlx::query("DELETE FROM decision_history").execute(pool).await.unwrap();
        for i in 0..2 {
            let req = DecisionRequest {
                decision_type: DecisionType::Choice,
                state: format!("s{i}"),
                question: format!("q{i}?"),
                options: vec!["x".into(), "y".into()],
            };
            let out = DecisionOutput {
                decision_type: DecisionType::Choice,
                scores: vec![0.0, 0.0],
                probabilities: vec![0.5, 0.5],
                index: 0,
                option: "x".into(),
            };
            record_history(&req, &out, 0, "ui").await;
        }
        assert_eq!(list_history(0, 0).await.unwrap().len(), 1, "limit 0 clamps to 1, not an empty page");
        assert_eq!(list_history(-10, 0).await.unwrap().len(), 1, "negative limit clamps to 1");
        assert_eq!(list_history(10, -5).await.unwrap().len(), 2, "negative offset clamps to 0");
        sqlx::query("DELETE FROM decision_history").execute(pool).await.unwrap();
    }

    #[test]
    fn parse_json_columns_tolerate_malformed_and_mistyped_payloads() {
        let _g = test_lock();
        assert_eq!(parse_json_string_array("not json"), Vec::<String>::new());
        assert_eq!(parse_json_string_array("{\"a\":1}"), Vec::<String>::new());
        assert_eq!(parse_json_f64_array("[1, \"x\"]"), Vec::<f64>::new());
        assert_eq!(parse_json_f64_array("null"), Vec::<f64>::new());
        assert_eq!(
            parse_json_string_array("[\"a\",\"b\"]"),
            vec!["a".to_string(), "b".to_string()]
        );
        let e = row_to_entry(
            "id".into(), "t".into(), "choice".into(), None, "q".into(),
            "not-json".into(), "[1,\"x\"]".into(), "[0.5]".into(), 0, "x".into(), None, None,
        );
        assert!(e.options.is_empty() && e.scores.is_empty());
        assert_eq!(e.probabilities, vec![0.5]);
    }

    #[test]
    fn decision_tool_schema_contract() {
        let _g = test_lock();
        DECISION_ACTIVE.store(true, Ordering::SeqCst);
        let s = builtin_decision_tools()[0].input_schema.clone();
        assert_eq!(s["type"], "object");
        assert_eq!(
            s["required"],
            serde_json::json!(["state", "question", "options"]),
            "decisionType must be optional (defaults to choice)"
        );
        assert_eq!(
            s["properties"]["decisionType"]["enum"],
            serde_json::json!(["choice", "score", "noul"])
        );
        assert_eq!(s["properties"]["options"]["minItems"], 2);
        assert_eq!(s["properties"]["options"]["maxItems"], 20);
        assert_eq!(s["properties"]["options"]["items"]["type"], "string");
        DECISION_ACTIVE.store(false, Ordering::SeqCst);
    }

    #[test]
    fn parse_decision_tool_args_rejects_unknown_and_bad_payload() {
        let _g = test_lock();
        assert!(parse_decision_tool_args("not_a_tool", &serde_json::json!({})).is_err());
        // Missing required fields.
        assert!(parse_decision_tool_args("decision_decide", &serde_json::json!({})).is_err());
        // Valid minimal payload (decisionType defaults to choice).
        let req = parse_decision_tool_args(
            "decision_decide",
            &serde_json::json!({
                "state": "ctx",
                "question": "q?",
                "options": ["a", "b"]
            }),
        )
        .unwrap();
        assert_eq!(req.decision_type, DecisionType::Choice);
        assert_eq!(req.options.len(), 2);
        // Explicit type round-trips.
        let req = parse_decision_tool_args(
            "decision_decide",
            &serde_json::json!({
                "decisionType": "noul",
                "state": "s",
                "question": "q",
                "options": ["false-opt", "true-opt"]
            }),
        )
        .unwrap();
        assert_eq!(req.decision_type, DecisionType::Noul);
    }

    #[tokio::test]
    async fn history_crud_roundtrip() {
        let _g = test_lock();
        db::ensure_test_pool().await;
        let pool = db::pool();
        sqlx::query("DELETE FROM decision_history").execute(pool).await.unwrap();
        let req = DecisionRequest {
            decision_type: DecisionType::Choice,
            state: "ctx".into(),
            question: "q?".into(),
            options: vec!["a".into(), "b".into()],
        };
        let out = DecisionOutput {
            decision_type: DecisionType::Choice,
            scores: vec![1.0, 2.0],
            probabilities: vec![0.25, 0.75],
            index: 1,
            option: "b".into(),
        };
        record_history(&req, &out, 12, "ui").await;
        let list = list_history(10, 0).await.unwrap();
        assert_eq!(list.len(), 1);
        let e = &list[0];
        assert_eq!(e.question, "q?");
        assert_eq!(e.options, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(e.scores, vec![1.0, 2.0]);
        assert_eq!(e.selected_index, 1);
        assert_eq!(e.selected_option, "b");
        assert_eq!(e.duration_ms, Some(12));
        assert_eq!(e.source.as_deref(), Some("ui"));
        let got = get_history(&e.id).await.unwrap().unwrap();
        assert_eq!(got.id, e.id);
        // Cleanup with a huge window keeps recent rows; a 0-day cutoff (clamped
        // to 1 day) still keeps rows created "now".
        let n = cleanup_history(3650).await.unwrap();
        assert_eq!(n, 0);
        let n = clear_history().await.unwrap();
        assert_eq!(n, 1);
        assert!(list_history(10, 0).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn list_history_pagination_clamps() {
        let _g = test_lock();
        db::ensure_test_pool().await;
        let pool = db::pool();
        sqlx::query("DELETE FROM decision_history").execute(pool).await.unwrap();
        for i in 0..3 {
            let req = DecisionRequest {
                decision_type: DecisionType::Score,
                state: format!("s{i}"),
                question: format!("q{i}?"),
                options: vec!["x".into(), "y".into()],
            };
            let out = DecisionOutput {
                decision_type: DecisionType::Score,
                scores: vec![0.0, 0.0],
                probabilities: vec![0.5, 0.5],
                index: 0,
                option: "x".into(),
            };
            record_history(&req, &out, i, "mcp").await;
        }
        let page = list_history(2, 0).await.unwrap();
        assert_eq!(page.len(), 2);
        let rest = list_history(2, 2).await.unwrap();
        assert_eq!(rest.len(), 1);
        // limit clamped to 500, offset floored at 0
        let big = list_history(10_000, 0).await.unwrap();
        assert_eq!(big.len(), 3);
        sqlx::query("DELETE FROM decision_history").execute(pool).await.unwrap();
    }
}
