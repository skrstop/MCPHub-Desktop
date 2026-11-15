//! decision_service — the decision-support (决策支持) backend: enable flag,
//! model selection, structured decide + request-history persistence, history
//! cleanup, and the builtin MCP tool exposure on the "mcphub-desktop" server.
//!
//! Mirrors the RAG service's shape: an AtomicBool enable flag (sync reads from
//! the builtin tool listing paths), config-persisted state
//! (`decision.enabled` / `decision.model`), and a single dispatch entry point
//! consumed by the shared builtin-tool paths.

use crate::db;
use crate::mv::decision::{DecisionModel, DecisionOutput, DecisionRequest, DecisionType};
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
/// Keyed by model size name + a freshness fingerprint of the GGUF file
/// (mtime + len): a selection change reloads on name mismatch, and a
/// re-download of the same size (file replaced asynchronously) reloads on
/// fingerprint mismatch.
static DECISION_MODEL: Mutex<Option<(String, (std::time::SystemTime, u64), Arc<DecisionModel>)>> =
    Mutex::const_new(None);

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
pub async fn set_enabled(app: &AppHandle, enabled: bool) -> Result<()> {
    let patch = serde_json::json!({ "decision": { "enabled": enabled } });
    crate::services::config_service::update(&patch).await?;
    DECISION_ACTIVE.store(enabled, Ordering::SeqCst);
    crate::rag::service::rag_log(
        "info",
        format!("decision support {}", if enabled { "enabled" } else { "disabled" }),
    );
    // The builtin server's Smart Routing index rows must follow this toggle
    // too (mirror of the RAG toggle's sync). The decide runs under
    // BUILTIN_INDEX_SYNC_LOCK (shared with the RAG toggle) so rapid toggles
    // cannot interleave stale writes. Best-effort, same as the RAG toggle.
    {
        let h = app.clone();
        tauri::async_runtime::spawn(async move {
            // sync_builtin_index logs its own failures; nothing to propagate.
            crate::rag::service::sync_builtin_index().await;
            let _ = h;
        });
    }
    Ok(())
}

/// The selected decision model size (`decision.model`), falling back to the
/// out-of-box default ready size when unset OR when the persisted selection
/// no longer resolves to files on disk (deleted download dir / bundled size
/// removed in an update) — otherwise every decide would hard-fail "not
/// ready" until an admin manually re-selected.
pub async fn selected_model(app: &AppHandle) -> Option<String> {
    if let Some(m) = crate::mv::models::current_decision_model().await {
        let ready = crate::mv::models::resolve_model_paths_kind(
            app,
            crate::mv::models::ModelKind::Decision,
            &m,
        )
        .ok()
        .flatten()
        .is_some();
        if ready {
            return Some(m);
        }
        crate::rag::service::rag_log(
            "warn",
            format!("decision model '{m}' is not ready; falling back to the default size"),
        );
    }
    crate::mv::models::default_decision_size(app)
}

/// The persisted selection's name when it exists but its files no longer
/// resolve (deleted download dir / removed bundled size). Pure so the
/// UI-facing "lost selection" derivation is unit-testable without an
/// AppHandle — the decide path's ready-fallback hides this state from
/// `selected_model`, but the settings UI must warn about it.
pub fn selection_lost(persisted: Option<String>, persisted_ready: bool) -> Option<String> {
    match persisted {
        Some(name) if !persisted_ready => Some(name),
        _ => None,
    }
}

/// The UI-facing decision status fields (pure derivation — the AppHandle-
/// dependent resolve calls stay in the command). Keeping the effective-vs-
/// persisted distinction here pins the bug class where the lost-model
/// warning silently dies: the effective model's readiness must NOT decide
/// `selection_lost` (the decide path's ready-fallback makes it always true).
pub struct DecisionStatusView {
    pub model: Option<String>,
    pub model_ready: bool,
    pub selection_lost: Option<String>,
}

/// A resolved model fact (name + whether its files resolve on disk). The
/// struct pairing keeps name/ready together — the bare-`bool` parameter list
/// made an effective/persisted swap at the call site compile silently.
pub struct ResolvedModel {
    pub name: Option<String>,
    pub ready: bool,
}

/// Position-distinct wrappers: swapping the effective and persisted models
/// at the `decision_status_view` call site is a COMPILE error, not a silent
/// selection_lost derivation swap.
pub struct EffectiveModel(pub ResolvedModel);
pub struct PersistedModel(pub ResolvedModel);

pub fn decision_status_view(
    effective: EffectiveModel,
    persisted: PersistedModel,
) -> DecisionStatusView {
    let selection_lost = selection_lost(persisted.0.name, persisted.0.ready);
    DecisionStatusView {
        model: effective.0.name,
        model_ready: effective.0.ready,
        selection_lost,
    }
}

/// Load (or fetch from cache) the selected decision model.
///
/// The cache is keyed by size name AND a freshness fingerprint of the GGUF
/// (mtime + len): re-downloading the currently-selected size replaces the
/// file on disk asynchronously (download_model_kind is fire-and-forget), so
/// a name-only key would keep serving the old weights until restart. The
/// fingerprint check detects any replacement on the next decide.
async fn cached_model(app: &AppHandle, name: &str) -> Result<Arc<DecisionModel>> {
    let path = DecisionModel::resolve_model_path(app, name)?;
    let fingerprint = model_file_fingerprint(&path);
    // Fast path: cache hit under a short lock. A cold load must NOT hold the
    // mutex across the multi-second spawn_blocking — that would head-of-line
    // block every concurrent decide (including instant cache hits) and stall
    // invalidate_model_cache.
    {
        let guard = DECISION_MODEL.lock().await;
        if let Some((cached, fp, m)) = guard.as_ref() {
            if cached == name && *fp == fingerprint {
                return Ok(m.clone());
            }
        }
    }
    // A downloaded dir contains only model.gguf; the bundled size dir (same
    // family/size under the bundled root) ships the official tokenizer.json —
    // pass it as the loader's fallback (the GGUF-embedded tokenizer is wrong
    // for decision models).
    let bundled_tokenizer = crate::mv::models::bundled_size_dir_kind(
        app,
        crate::mv::models::ModelKind::Decision,
        name,
    )
    .map(|d| d.join("tokenizer.json"))
    .filter(|p| p.exists());
    let device = candle_core::Device::Cpu;
    let model = tokio::task::spawn_blocking({
        let path = path.clone();
        move || -> Result<DecisionModel> {
            let mut file = std::fs::File::open(&path)?;
            let dir = path
                .parent()
                .ok_or_else(|| anyhow!("model path has no parent"))?
                .to_path_buf();
            DecisionModel::load(&mut file, &dir, &device, bundled_tokenizer.as_deref())
        }
    })
    .await
    .map_err(|e| anyhow!("decision model load failed: {e}"))??;
    let arc = Arc::new(model);
    // Re-acquire only to publish. Duplicate concurrent loads of the same size
    // are possible but bounded (each caller loads once, last writer wins) —
    // the same trade the mv runtime registry makes for its cold path.
    *DECISION_MODEL.lock().await = Some((name.to_string(), fingerprint, arc.clone()));
    Ok(arc)
}

/// (mtime, len) fingerprint of the model GGUF — the cache-freshness key.
/// Unreadable file → a unique degenerate fingerprint (forces a reload
/// attempt, whose own error surfaces).
fn model_file_fingerprint(path: &std::path::Path) -> (std::time::SystemTime, u64) {
    match std::fs::metadata(path) {
        Ok(m) => (m.modified().unwrap_or(std::time::UNIX_EPOCH), m.len()),
        Err(_) => (std::time::UNIX_EPOCH, u64::MAX),
    }
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
    let _permit = try_begin_decide()?;
    let model = selected_model(app)
        .await
        .ok_or_else(|| anyhow!("no decision model is ready - download one first"))?;
    let m = cached_model(app, &model).await?;
    let start = std::time::Instant::now();
    let req_for_task = req.clone();
    let out = tokio::task::spawn_blocking(move || m.decide(&req_for_task))
        .await
        .map_err(|e| anyhow!("decision failed: {e}"))??;
    let duration_ms = start.elapsed().as_millis() as i64;
    record_history(&req, &out, duration_ms, source).await;
    Ok(out)
}

/// At most one decide runs at a time (model memory + forward tensors are
/// per-call costs; a desktop app has no use for parallel decides).
static DECIDE_SEMAPHORE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// Gate + concurrency bound for decide calls, extracted from
/// `decide_and_record` for testability (the AppHandle-taking wrapper cannot
/// run in unit tests). Disabled → Err; permit held elsewhere → busy Err.
/// Concurrency bound rationale: each decide holds a 168 MB model +
/// O(seq×hidden) forward tensors. Unbounded parallel calls (HTTP allows
/// unlimited concurrency) could OOM the desktop app — serialize decides;
/// extra callers fail fast with a busy error instead of queueing forever.
fn try_begin_decide() -> Result<tokio::sync::SemaphorePermit<'static>> {
    if !is_enabled() {
        return Err(anyhow!("decision support is not enabled"));
    }
    DECIDE_SEMAPHORE
        .try_acquire()
        .map_err(|_| anyhow!("decision runtime is busy, try again shortly"))
}

/// Insert one history row (best-effort — a history write failure must not
/// fail the decision itself; the failure is logged). Persisted fields are
/// truncated: the input caps count chars (a CJK payload reaches ~4 bytes per
/// char), so raw persistence could store ~700 KB per row and the row cap
/// alone would permit tens of GB of growth.
fn truncate_for_history(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

const HISTORY_FIELD_MAX_BYTES: usize = 16 * 1024;

pub async fn record_history(
    req: &DecisionRequest,
    out: &DecisionOutput,
    duration_ms: i64,
    source: &str,
) {
    let id = uuid::Uuid::new_v4().to_string();
    let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let state = truncate_for_history(&req.state, HISTORY_FIELD_MAX_BYTES);
    let question = truncate_for_history(&req.question, HISTORY_FIELD_MAX_BYTES);
    let options: Vec<String> = req
        .options
        .iter()
        .map(|o| truncate_for_history(o, 1024))
        .collect();
    let options = serde_json::to_string(&options).unwrap_or_else(|_| "[]".into());
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
    .bind(&state)
    .bind(&question)
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
/// Returns the number removed. Busy-retry: the pool's 5s busy_timeout can be
/// exceeded by a burst of activity-log writes holding the write lock (same
/// failure mode `remove_doc_sql` guards against) — retry instead of silently
/// skipping this cycle's prune.
pub async fn cleanup_history(days: i64) -> Result<u64> {
    cleanup_history_with_now(days, Local::now()).await
}

/// `cleanup_history` with an explicit clock — extracted so the boundary
/// tests are deterministic (a wall-clock second tick between the test's
/// fixture timestamps and the call would shift the cutoff and flip the
/// `<`-vs-`<=` boundary assertions).
pub(crate) async fn cleanup_history_with_now(
    days: i64,
    now: chrono::DateTime<Local>,
) -> Result<u64> {
    let days = days.clamp(1, 3650);
    let cutoff = (now - chrono::Duration::days(days))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let mut last_err: Option<sqlx::Error> = None;
    for attempt in 0..4 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        }
        match sqlx::query("DELETE FROM decision_history WHERE created_at < ?")
            .bind(&cutoff)
            .execute(db::pool())
            .await
        {
            Ok(r) => {
                if r.rows_affected() > 0 {
                    crate::rag::service::rag_log(
                        "info",
                        format!("cleaned {} decision history rows older than {days}d", r.rows_affected()),
                    );
                }
                // Hard row-cap prune (independent of age): bounds the table
                // even when decides arrive faster than the retention window
                // expires. Best-effort — a failure here must not fail the
                // age prune's result.
                if let Err(e) = prune_history_row_cap().await {
                    crate::rag::service::rag_log(
                        "warn",
                        format!("decision history row-cap prune failed: {e:#}"),
                    );
                }
                return Ok(r.rows_affected());
            }
            Err(e) => last_err = Some(e),
        }
    }
    Err(anyhow!(
        "decision history cleanup failed after 4 attempts: {}",
        last_err.map(|e| e.to_string()).unwrap_or_default()
    ))
}

/// Hard upper bound on retained history rows (independent of the 15-day age
/// prune): a chatty MCP client deciding in a loop could otherwise grow the
/// table to multi-GB within the retention window. Enforced during each
/// cleanup cycle — the table is bounded at MAX_ROWS plus ≤6h of growth.
const HISTORY_MAX_ROWS: i64 = 100_000;

/// Drop rows beyond the newest `HISTORY_MAX_ROWS` (rowid-ordered — newest
/// kept). Best-effort bound, run inside the periodic cleanup cycle.
async fn prune_history_row_cap() -> Result<u64> {
    let r = sqlx::query(
        "DELETE FROM decision_history WHERE rowid NOT IN (
             SELECT rowid FROM decision_history ORDER BY created_at DESC, id DESC LIMIT ?
         )",
    )
    .bind(HISTORY_MAX_ROWS)
    .execute(db::pool())
    .await?;
    if r.rows_affected() > 0 {
        crate::rag::service::rag_log(
            "info",
            format!(
                "pruned {} decision history rows over the {HISTORY_MAX_ROWS}-row cap",
                r.rows_affected()
            ),
        );
    }
    Ok(r.rows_affected())
}

// ── builtin MCP tool exposure ───────────────────────────────────────────────

/// The decision tool catalog — the full `Tool` definition regardless of the
/// enable flag. The "view tools" dialog documents what the tool accepts even
/// while decision support is off; enable gating applies to the MCP surface
/// (`builtin_decision_tools`) and the dispatch path, not to documentation.
pub fn decision_tool_catalog() -> Vec<Tool> {
    let lang = crate::services::tool_i18n::current_tool_language();
    vec![Tool {
        hidden_params: None,
        name: "decision_decide".to_string(),
        description: Some(crate::services::tool_i18n::decision_tool_description(&lang).to_string()),
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

/// The decision tool as a proper `Tool` on the builtin "mcphub-desktop"
/// server. Empty when decision support is disabled — gated here on
/// `is_enabled()`; callers rely on this self-gating so the tools/list
/// surfaces never advertise a tool whose dispatch would reject.
pub fn builtin_decision_tools() -> Vec<Tool> {
    if !is_enabled() {
        return Vec::new();
    }
    decision_tool_catalog()
}

/// Parse + validate the tool arguments (pure — unit-testable without an app
/// handle). Unknown tool names are rejected here too so the dispatch entry
/// can fail fast before touching the model.
/// Sentinel prefix marking CALLER-side argument validation failures (as
/// opposed to model/load failures). The /mcp bridge maps these to JSON-RPC
/// -32602 (INVALID_PARAMS) so clients can distinguish "fix your arguments"
/// from "retryable server error".
pub const INVALID_PARAMS_PREFIX: &str = "[invalid params] ";

/// Whether an error message carries the invalid-params sentinel.
pub fn is_invalid_params_error(msg: &str) -> bool {
    msg.starts_with(INVALID_PARAMS_PREFIX)
}

pub fn parse_decision_tool_args(
    tool_name: &str,
    args: &serde_json::Value,
) -> Result<DecisionRequest> {
    if tool_name != "decision_decide" {
        // Match the established not-found message shape ("Tool 'x' not
        // found") so REST faces classify it as 404 like unknown rag_* tools.
        return Err(anyhow!("Tool '{}' not found", tool_name));
    }
    let req: DecisionRequest = serde_json::from_value(args.clone())
        .map_err(|e| anyhow!("{INVALID_PARAMS_PREFIX}invalid arguments: {e}"))?;
    // Mirror the schema contract (minItems/maxItems/non-empty) and the
    // model's own sequence() validation — fail fast with a precise error
    // instead of a late model-level failure.
    if req.options.len() < 2 || req.options.len() > 20 {
        return Err(anyhow!(
            "{INVALID_PARAMS_PREFIX}options must contain 2-20 entries (got {})",
            req.options.len()
        ));
    }
    if req.options.iter().any(|o| o.is_empty()) {
        return Err(anyhow!("{INVALID_PARAMS_PREFIX}options must be non-empty"));
    }
    // Field + total char caps (mirror of sequence()'s guard): rejecting here
    // avoids paying a cold 168 MB model load for an oversized request.
    const MAX_FIELD_CHARS: usize = 64 * 1024;
    const MAX_TOTAL_CHARS: usize = 128 * 1024;
    if req.question.chars().count() > MAX_FIELD_CHARS
        || req.state.chars().count() > MAX_FIELD_CHARS
        || req.options.iter().any(|o| o.chars().count() > MAX_FIELD_CHARS)
    {
        return Err(anyhow!(
            "{INVALID_PARAMS_PREFIX}question/state/options exceed the {MAX_FIELD_CHARS}-character limit"
        ));
    }
    let total: usize = req.question.chars().count()
        + req.state.chars().count()
        + req.options.iter().map(|o| o.chars().count()).sum::<usize>();
    if total > MAX_TOTAL_CHARS {
        return Err(anyhow!(
            "{INVALID_PARAMS_PREFIX}question+state+options exceed the {MAX_TOTAL_CHARS}-character total limit"
        ));
    }
    // Official validate_row contract: noul decisions are the fixed
    // [false, true] option pair — anything else reaches the model with a
    // meaningless type embedding.
    if req.decision_type == DecisionType::Noul
        && req.options != vec!["false".to_string(), "true".to_string()]
    {
        return Err(anyhow!(
            "{INVALID_PARAMS_PREFIX}noul decisions require exactly the option pair [\"false\", \"true\"]"
        ));
    }
    Ok(req)
}

/// The decision tool as JSON tool definitions (name / description /
/// inputSchema) — the shape the `decision_tools` command returns and the
/// "view tools" dialog renders. Always returns the catalog, regardless of
/// the enable flag: the dialog documents the tool even while decision
/// support is off (enable gating applies to the MCP surface only).
pub fn decision_tool_definitions() -> Vec<serde_json::Value> {
    decision_tool_catalog()
        .into_iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name,
                "description": t.description.unwrap_or_default(),
                "inputSchema": t.input_schema,
            })
        })
        .collect()
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
    // serde_json rejects NaN/Infinity floats — a degenerate forward pass must
    // surface as a tool error, never as an empty "success" text.
    let text = serde_json::to_string_pretty(&out)
        .map_err(|e| anyhow!("decision output serialization failed: {e}"))?;
    Ok(ToolCallResult {
        raw_meta: None,
        content: vec![serde_json::json!({ "type": "text", "text": text })],
        is_error: false,
        structured_content: None,
    })
}

/// Serializes every test that touches the process-global DECISION_ACTIVE
/// flag or the shared decision_history table — parallel `cargo test`
/// otherwise races on both (flag flips + DELETE/INSERT interleavings).
/// `pub(crate)` so commands::decision's tests can hold it too (the resting
/// flag state is then deterministic: every lock-holder restores false).
#[cfg(test)]
static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    match TEST_LOCK.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn decision_tool_definitions_shape_and_gating() {
        let _g = test_lock();
        // Disabled → the catalog is STILL returned: the "view tools" dialog
        // documents the tool even while decision support is off. Enable
        // gating applies to the MCP surface (builtin_decision_tools, covered
        // by decision_tools_gated_on_flag), not to documentation.
        DECISION_ACTIVE.store(false, Ordering::SeqCst);
        let defs_off = decision_tool_definitions();
        assert_eq!(defs_off.len(), 1);
        assert_eq!(defs_off[0]["name"], "decision_decide");
        // Enabled → identical entry (the catalog must be flag-independent):
        // name / description / inputSchema, schema mirroring the parser.
        DECISION_ACTIVE.store(true, Ordering::SeqCst);
        let defs = decision_tool_definitions();
        assert_eq!(defs, defs_off, "catalog output must not depend on the enable flag");
        assert_eq!(defs.len(), 1);
        let d = &defs[0];
        assert_eq!(d["name"], "decision_decide");
        assert!(d["description"].as_str().is_some_and(|s| !s.is_empty()));
        let schema = &d["inputSchema"];
        assert_eq!(schema["type"], "object");
        let props = schema["properties"].as_object().unwrap();
        for k in ["decisionType", "state", "question", "options"] {
            assert!(props.contains_key(k), "schema property {k} missing");
        }
        assert_eq!(schema["required"], serde_json::json!(["state", "question", "options"]));
        assert_eq!(props["decisionType"]["enum"], serde_json::json!(["choice", "score", "noul"]));
        DECISION_ACTIVE.store(false, Ordering::SeqCst);
    }

    #[test]
    fn decision_tool_catalog_is_flag_independent() {
        let _g = test_lock();
        // The catalog seam itself: always exactly the decision_decide tool,
        // regardless of the enable flag (gating lives in builtin_decision_tools).
        DECISION_ACTIVE.store(false, Ordering::SeqCst);
        let off = decision_tool_catalog();
        DECISION_ACTIVE.store(true, Ordering::SeqCst);
        let on = decision_tool_catalog();
        DECISION_ACTIVE.store(false, Ordering::SeqCst);
        // Field-wise identity (Tool has no PartialEq derive): the catalog
        // must be a pure constant regardless of the flag.
        assert_eq!(off.len(), on.len());
        for (a, b) in off.iter().zip(on.iter()) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.description, b.description);
            assert_eq!(a.input_schema, b.input_schema);
            assert_eq!(a.server_name, b.server_name);
        }
        assert_eq!(off.len(), 1);
        assert_eq!(off[0].name, "decision_decide");
        assert_eq!(off[0].server_name, crate::rag::service::BUILTIN_SERVER_NAME);
        assert!(off[0].enabled);
    }

    #[test]
    fn selection_lost_only_when_persisted_unready() {
        // Ready selection → no warning. Unready persisted name → surfaced
        // (the effective-model fallback hides it from `model`). No persisted
        // selection → never "lost".
        assert_eq!(selection_lost(Some("julia/1".into()), true), None);
        assert_eq!(
            selection_lost(Some("user/2".into()), false),
            Some("user/2".to_string())
        );
        assert_eq!(selection_lost(None, true), None);
        assert_eq!(selection_lost(None, false), None);
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

    #[test]
    fn parse_decision_tool_args_rejects_oversized_fields() {
        let _g = test_lock();
        // Per-field cap: 64k chars + 1 rejected; exactly 64k accepted.
        let ok_q = "q".repeat(64 * 1024);
        let over_q = "q".repeat(64 * 1024 + 1);
        assert!(parse_decision_tool_args(
            "decision_decide",
            &serde_json::json!({"state": "s", "question": ok_q, "options": ["a", "b"]})
        )
        .is_ok());
        assert!(parse_decision_tool_args(
            "decision_decide",
            &serde_json::json!({"state": "s", "question": over_q, "options": ["a", "b"]})
        )
        .is_err());
        // Total cap: two under-cap fields whose sum exceeds 128k rejected.
        let half = "s".repeat(70 * 1024);
        assert!(parse_decision_tool_args(
            "decision_decide",
            &serde_json::json!({"state": half, "question": "q".repeat(70 * 1024), "options": ["a", "b"]})
        )
        .is_err());
    }

    #[test]
    fn parse_decision_tool_args_noul_rejects_reversed_pair_and_unknown_type() {
        let _g = test_lock();
        // The noul pair is order-sensitive: ["true", "false"] must be rejected
        // (a set-membership regression would accept it and feed the model a
        // wrong type embedding).
        assert!(parse_decision_tool_args(
            "decision_decide",
            &serde_json::json!({"decisionType": "noul", "state": "s", "question": "q", "options": ["true", "false"]})
        )
        .is_err());
        // The exact pair is the only accepted noul option set.
        assert!(parse_decision_tool_args(
            "decision_decide",
            &serde_json::json!({"decisionType": "noul", "state": "s", "question": "q", "options": ["false", "true"]})
        )
        .is_ok());
        // Unknown decisionType values are rejected by serde (no fallback) —
        // pins the schema enum contract against a silent alias/`other` arm.
        assert!(parse_decision_tool_args(
            "decision_decide",
            &serde_json::json!({"decisionType": "bogus", "state": "s", "question": "q", "options": ["a", "b"]})
        )
        .is_err());
        assert!(parse_decision_tool_args(
            "decision_decide",
            &serde_json::json!({"decisionType": "CHOICE", "state": "s", "question": "q", "options": ["a", "b"]})
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
        // Explicit clock (same `now` the fixtures derive from) — a wall-clock
        // tick during the inserts must not shift the cutoff and flip the
        // boundary assertions.
        let n = cleanup_history_with_now(1, now).await.unwrap();
        assert_eq!(n, 1, "only the strictly-older row is deleted");
        let remaining: Vec<(String,)> = sqlx::query_as("SELECT id FROM decision_history")
            .fetch_all(pool)
            .await
            .unwrap();
        let remaining: Vec<String> = remaining.into_iter().map(|(id,)| id).collect();
        assert!(remaining.contains(&"at".to_string()), "row exactly at cutoff must survive (< not <=)");
        assert!(remaining.contains(&"new".to_string()));
        // days=0 / negative clamp to 1 → nothing new deleted.
        assert_eq!(cleanup_history_with_now(0, now).await.unwrap(), 0);
        assert_eq!(cleanup_history_with_now(-5, now).await.unwrap(), 0);
        // Upper clamp: days=99999 must clamp to 3650 — a 4000d-old row falls
        // inside the clamped cutoff and is deleted (without the clamp the
        // cutoff would reach 99999d back and the row would survive), while a
        // 3600d-old row must SURVIVE — pinning the 3650 constant from both
        // sides (a clamp lowered below ~3650 would delete it).
        sqlx::query("INSERT INTO decision_history (id, created_at, decision_type, question, options_json, scores_json, probabilities_json, selected_index, selected_option) VALUES ('ancient', ?, 'choice', 'q', '[]', '[]', '[]', 0, 'x')")
            .bind(fmt(now - chrono::Duration::days(4000)))
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO decision_history (id, created_at, decision_type, question, options_json, scores_json, probabilities_json, selected_index, selected_option) VALUES ('oldish', ?, 'choice', 'q', '[]', '[]', '[]', 0, 'x')")
            .bind(fmt(now - chrono::Duration::days(3600)))
            .execute(pool)
            .await
            .unwrap();
        assert_eq!(cleanup_history_with_now(99_999, now).await.unwrap(), 1);
        let remaining: Vec<(String,)> = sqlx::query_as("SELECT id FROM decision_history")
            .fetch_all(pool)
            .await
            .unwrap();
        let ids: Vec<String> = remaining.into_iter().map(|(id,)| id).collect();
        assert!(!ids.contains(&"ancient".to_string()), "4000d row must be deleted at the 3650d clamp");
        assert!(ids.contains(&"oldish".to_string()), "3600d row must survive the 3650d clamp");
        assert!(ids.contains(&"at".to_string()));
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
        // Pin the UPPER clamp: with >500 rows, a huge limit must return
        // exactly 500 (a missing clamp would return all rows; a tighter
        // clamp would return fewer).
        sqlx::query("DELETE FROM decision_history").execute(pool).await.unwrap();
        for i in 0..502 {
            sqlx::query("INSERT INTO decision_history (id, created_at, decision_type, question, options_json, scores_json, probabilities_json, selected_index, selected_option) VALUES (?, ?, 'choice', ?, '[]', '[]', '[]', 0, 'x')")
                .bind(format!("clamp-{i}"))
                .bind(format!("2026-01-01 00:{:02}:{:02}", i / 60 % 60, i % 60))
                .bind(format!("q{i}?"))
                .execute(pool)
                .await
                .unwrap();
        }
        assert_eq!(list_history(10_000, 0).await.unwrap().len(), 500, "limit clamps to 500");
        assert_eq!(list_history(10_000, 500).await.unwrap().len(), 2, "offset past the clamp page reaches the tail");
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
        // Explicit type round-trips (noul requires the exact pair).
        let req = parse_decision_tool_args(
            "decision_decide",
            &serde_json::json!({
                "decisionType": "noul",
                "state": "s",
                "question": "q",
                "options": ["false", "true"]
            }),
        )
        .unwrap();
        assert_eq!(req.decision_type, DecisionType::Noul);
        // noul with any other option set is rejected.
        assert!(parse_decision_tool_args(
            "decision_decide",
            &serde_json::json!({
                "decisionType": "noul",
                "state": "s",
                "question": "q",
                "options": ["false", "true", "extra"]
            })
        )
        .is_err());
    }

    #[test]
    fn truncate_for_history_respects_byte_cap_and_char_boundary() {
        // Under cap: verbatim.
        assert_eq!(truncate_for_history("short", 16), "short");
        // ASCII over cap: hard cut at the cap.
        let ascii = "a".repeat(20);
        assert_eq!(truncate_for_history(&ascii, 16).len(), 16);
        // CJK (3 bytes/char): cap 7 must back off to 6, never panic on a
        // non-char-boundary slice.
        let cjk = "汉".repeat(10);
        let t = truncate_for_history(&cjk, 7);
        assert_eq!(t.len(), 6);
        assert!(t.is_char_boundary(t.len()));
        // Empty input.
        assert_eq!(truncate_for_history("", 16), "");
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

    #[test]
    fn invalid_params_sentinel_linkage() {
        // Every parse_decision_tool_args rejection (except the unknown-tool
        // 404 shape) must carry the sentinel so REST faces classify it 400.
        let cases = vec![
            serde_json::json!({}),                                     // missing fields
            serde_json::json!({"state": "s", "question": "q", "options": ["a"]}), // too few
            serde_json::json!({"state": 1, "question": "q", "options": ["a", "b"]}), // bad type
        ];
        for args in cases {
            let err = parse_decision_tool_args("decision_decide", &args).unwrap_err();
            let msg = format!("{err:#}");
            assert!(
                is_invalid_params_error(&msg),
                "error missing sentinel: {msg}"
            );
        }
        // Unknown tool is 404-classified — must NOT carry the 400 sentinel.
        let err = parse_decision_tool_args("not_a_tool", &serde_json::json!({})).unwrap_err();
        assert!(!is_invalid_params_error(&format!("{err:#}")));
        // Sentinel helpers agree on the exact constant.
        assert!(is_invalid_params_error(INVALID_PARAMS_PREFIX));
        assert!(!is_invalid_params_error("[invalid params]"));
        assert!(!is_invalid_params_error(""));
    }

    #[tokio::test]
    async fn try_begin_decide_disabled_and_busy_paths() {
        let _g = test_lock();
        // Disabled → gate error, regardless of semaphore state.
        DECISION_ACTIVE.store(false, Ordering::SeqCst);
        let err = try_begin_decide().unwrap_err();
        assert!(format!("{err:#}").contains("not enabled"));
        // Enabled → permit acquired; a second concurrent caller fails fast.
        DECISION_ACTIVE.store(true, Ordering::SeqCst);
        let permit = try_begin_decide().expect("first acquire must succeed");
        let err = try_begin_decide().unwrap_err();
        assert!(format!("{err:#}").contains("busy"));
        // Released → acquirable again.
        drop(permit);
        let _again = try_begin_decide().expect("release must restore availability");
        DECISION_ACTIVE.store(false, Ordering::SeqCst);
    }

    #[tokio::test]
    async fn prune_history_row_cap_below_cap_is_noop() {
        let _g = test_lock();
        crate::db::ensure_test_pool().await;
        let pool = crate::db::pool();
        sqlx::query("DELETE FROM decision_history").execute(pool).await.unwrap();
        let req = DecisionRequest {
            decision_type: DecisionType::Choice,
            state: "s".into(),
            question: "q".into(),
            options: vec!["a".into(), "b".into()],
        };
        let out = DecisionOutput {
            decision_type: DecisionType::Choice,
            scores: vec![0.1, 0.2],
            probabilities: vec![0.3, 0.7],
            index: 1,
            option: "b".into(),
        };
        for i in 0..5 {
            record_history(&req, &out, i, "mcp").await;
        }
        // Well below HISTORY_MAX_ROWS → nothing pruned.
        let pruned = prune_history_row_cap().await.unwrap();
        assert_eq!(pruned, 0);
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM decision_history")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(n, 5);
        sqlx::query("DELETE FROM decision_history").execute(pool).await.unwrap();
    }
}
