//! High-level RAG service: lifecycle + document + search operations.
//!
//! Lifecycle is driven by `toggle(enabled)`:
//!   enable  -> `check_memory_sufficient()` -> load `Embedder` (candle) -> open
//!             `VectorDb` -> store in the global runtime. Blocks until ready.
//!   disable -> drop the runtime (frees the candle model + closes lancedb).
//!
//! The runtime is held in a global `tokio::sync::Mutex<Option<Runtime>>`.
//! Document metadata lives on disk under `<app_data_dir>/rag/files` (one
//! content file + one `.meta` JSON per doc) so the list works even when RAG
//! is OFF. Chunks + embeddings live in lancedb.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{anyhow, Result};
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::models::rag::{
    BatchPreview, RagDoc, RagDocInfo, RagFolderScan, RagPickedFile, RagScanFile, RagScanGroup,
    RagSearchResult, RagSettings, RagStatus, RagDocPage, RagTagPage, RagTagStat, RagUpdateCheck,
};
use crate::rag::chunker::chunk_document;
use crate::rag::embedder::{check_memory_sufficient, detect_format, load_embedder, read_max_context, Embedder};
use crate::rag::vectordb::{ChunkInput, VectorDb};

/// Write a RAG log line to both the env logger and the DB log panel (visible
/// in the Logs page, filterable by server = "rag"). `level` is "info"/"warn"/"error".
/// `pub(crate)` so sibling modules (`rag::extract::*`) log their extraction /
/// OCR progress into the same pipeline.
pub(crate) fn rag_log(level: &str, msg: impl std::fmt::Display) {
    let line = format!("[RAG] {}", msg);
    match level {
        "warn" => log::warn!("{}", line),
        "error" => log::error!("{}", line),
        _ => log::info!("{}", line),
    }
    crate::services::app_logger::log_to_db(level, &line);
}

/// Per-file upload progress emitted to the frontend during indexing so the UI
/// can show a SECOND progress bar (character-based) under the per-file bar.
/// Frontend listens on `rag://upload-progress` (see `useRagData.tsx`).
///
/// `chars_done` / `chars_total` advance as each embedding batch finishes; the
/// service caps `chars_done` at `chars_total` (chunk overlap double-counts a
/// little, so the raw sum can slightly overshoot — the bar should never jump
/// past 100% mid-file). `name` lets the UI match the event to the file the
/// outer upload loop is currently on.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RagUploadProgress {
    /// Display name of the file currently being indexed (matches the outer
    /// loop's `uploadProgress.name`).
    name: String,
    /// Characters of the document embedded so far.
    chars_done: u64,
    /// Total characters in the document.
    chars_total: u64,
}

const UPLOAD_PROGRESS_EVENT: &str = "rag://upload-progress";

fn emit_upload_progress(app: &AppHandle, name: &str, chars_done: u64, chars_total: u64) {
    let payload = RagUploadProgress {
        name: name.to_string(),
        chars_done: chars_done.min(chars_total),
        chars_total,
    };
    if let Err(e) = app.emit(UPLOAD_PROGRESS_EVENT, &payload) {
        log::warn!("[RAG] emit upload-progress failed: {e}");
    }
}

/// File-level progress emitted during `reindex_all` (re-embedding all docs
/// after a model swap). The frontend reuses the upload overlay (the same UI as
/// importing) and listens on `rag://reindex-progress` to drive the per-file
/// bar; `rag://upload-progress` (char-level) is emitted by `reindex_doc`
/// inside the loop for the second bar.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RagReindexProgress {
    /// 0-based index of the doc currently being re-embedded.
    current: u32,
    /// Total docs to re-embed.
    total: u32,
    /// Display name of the current doc.
    name: String,
}

const REINDEX_PROGRESS_EVENT: &str = "rag://reindex-progress";

fn emit_reindex_progress(app: &AppHandle, current: u32, total: u32, name: &str) {
    let payload = RagReindexProgress {
        current,
        total,
        name: name.to_string(),
    };
    if let Err(e) = app.emit(REINDEX_PROGRESS_EVENT, &payload) {
        log::warn!("[RAG] emit reindex-progress failed: {e}");
    }
}

/// File-level + char-level progress emitted during the async batch-update pass
/// (re-indexing docs whose source changed). The frontend's batch-update
/// progress dialog reuses the upload overlay style and listens on
/// `rag://batch-update-progress`. `phase` = "checking" before reading the
/// source md5, "reindexing" while `reindex_doc` runs (the char-level
/// `rag://upload-progress` events fire inside reindex_doc for the sub-bar),
/// "done" once the whole batch finishes.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RagBatchUpdateProgress {
    /// 0-based index of the doc currently being processed.
    current: u32,
    /// Total docs in the batch.
    total: u32,
    /// Display name of the current doc.
    name: String,
    /// "checking" | "reindexing" | "done".
    phase: String,
}

const BATCH_UPDATE_PROGRESS_EVENT: &str = "rag://batch-update-progress";

fn emit_batch_update_progress(
    app: &AppHandle,
    current: u32,
    total: u32,
    name: &str,
    phase: &str,
) {
    let payload = RagBatchUpdateProgress {
        current,
        total,
        name: name.to_string(),
        phase: phase.to_string(),
    };
    if let Err(e) = app.emit(BATCH_UPDATE_PROGRESS_EVENT, &payload) {
        log::warn!("[RAG] emit batch-update-progress failed: {e}");
    }
}

/// Single running batch-update task handle. Guards against a second trigger
/// restarting the task while one is in flight (the frontend button re-open the
/// progress dialog instead of re-triggering, but this is the backend guard).
static BATCH_UPDATE_RUNNING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Serializes all doc-mutating operations (update from original/file, MCP
/// rag_file_update, delete, set-tags) so concurrent read-modify-write of the
/// same `{id}.meta` can't interleave. Lock ordering with the runtime lock is
/// ALWAYS meta_lock -> runtime: every holder acquires this first, then awaits
/// `reindex_doc`/vector ops (which take the runtime lock internally). Never
/// acquire in the reverse order (deadlock).
static META_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

async fn meta_lock() -> &'static Mutex<()> {
    META_LOCK.get_or_init(|| Mutex::new(()))
}

/// Dedup gate for the deferred lancedb prune. LanceDB is append-only: `delete`
/// and overwrite just create new dataset versions that omit the old rows, and
/// the old versions (with their embeddings) linger on disk until a prune runs.
/// `optimize()` runs that prune — but it rewrites every surviving version and
/// is slow (seconds), so calling it inline in `delete_doc`/overwrite blocks the
/// IPC response for that whole duration, which is exactly why the doc list
/// "doesn't refresh" after delete (the `await` never returns until prune ends).
///
/// Instead we kick off ONE deduped background prune: the first caller arms a
/// spawned task that waits briefly (so a burst of deletes coalesces into a
/// single prune) and then prunes once. While that task is pending, further
/// callers are no-ops — they know a prune is already queued, so one prune
/// covers them all. The caller's request returns immediately, and disk space
/// is reclaimed a moment later without blocking the UI refresh.
static PRUNE_PENDING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
/// Wakes the parked prune task when a fresh delete/overwrite arrives after the
/// coalesce delay has already elapsed but before the task re-checks the gate.
static PRUNE_NOTIFY: OnceLock<tokio::sync::Notify> = OnceLock::new();

fn prune_notify() -> &'static tokio::sync::Notify {
    PRUNE_NOTIFY.get_or_init(|| tokio::sync::Notify::new())
}

/// Schedule a deduped background prune. Safe to call on every delete/overwrite;
/// extra calls while one is queued or running are coalesced into that single
/// prune. The caller does NOT wait — this returns immediately. If the runtime
/// was just dropped (RAG disabled) between the caller's check and the task's,
/// the task acquires the runtime lock, sees `None`, and exits harmlessly.
fn schedule_deferred_prune(app: &AppHandle) {
    // CAS: only the FIRST caller after an idle period arms a task. Concurrent
    // callers (burst delete) lose the CAS and return — they rely on the armed
    // task's prune (which will see their deletes too, since delete_by_doc
    // already wrote the new versions before we get here).
    if PRUNE_PENDING
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .is_err()
    {
        return; // a prune is already queued/running — nothing to do
    }
    // Move the AppHandle into the spawned task so it owns 'static data (kept
    // for future prune-event diagnostics; the prune itself only needs the
    // runtime, which it re-acquires via the global lock below). Prefixed `_`
    // because no current prune path reads it, but holding it prevents a future
    // emit/diagnostics addition from silently dropping the handle.
    let _app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Coalesce window: a burst of deletes (batch delete, overwrite-many)
        // should fold into ONE prune. Wait briefly so later deletes that lost
        // the CAS land before we prune. Short enough that disk reclaim feels
        // immediate, long enough that a 50-doc batch delete prunes once not
        // 50×. The Notify lets a delete arriving just after this delay cut the
        // wait short so reclaim still happens promptly in the single-doc case.
        let _ = tokio::time::timeout(
            std::time::Duration::from_millis(800),
            prune_notify().notified(),
        )
        .await;
        // Take the runtime lock (same meta->runtime ordering invariant: we hold
        // no meta lock here, and the inline delete paths have already released
        // theirs, so there's no ordering risk). Holding it for the prune
        // serializes against a concurrent reindex_doc/search on the same table.
        let guard = runtime().lock().await;
        let Some(rt) = guard.as_ref() else {
            // RAG was disabled between the delete and the prune. The deleted
            // rows' old versions will be reclaimed on next enable's optimize or
            // a reset's drop_table; no orphan vectors are left queryable (the
            // delete already wrote a version omitting them).
            PRUNE_PENDING.store(false, std::sync::atomic::Ordering::SeqCst);
            return;
        };
        if let Err(e) = rt.db.optimize().await {
            rag_log("warn", format!("deferred prune failed: {:#}", e));
        }
        // Clear the gate LAST so a delete that arrived during this prune (and
        // lost the CAS) leaves PRUNE_PENDING=true — the next idle caller arms a
        // fresh prune for those newer deletes.
        PRUNE_PENDING.store(false, std::sync::atomic::Ordering::SeqCst);
        // If a delete arrived during the prune (lost CAS, still pending), wake
        // so the next armed task doesn't sit through the full coalesce delay.
        prune_notify().notify_one();
    });
}

/// Atomically write a `.meta` file: write to `{path}.tmp` then rename over the
/// target. Rename-on-same-filesystem is atomic on all supported platforms, so
/// a crash mid-write can never leave a truncated/half JSON that would make the
/// doc disappear from `list_docs` (metas are parsed strictly).
fn write_meta_atomic(meta_path: &Path, meta: &DocMeta) -> Result<()> {
    let bytes = serde_json::to_vec(meta)?;
    let tmp = meta_path.with_extension("meta.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, meta_path)?;
    Ok(())
}

/// The loaded runtime. Dropped on disable to release resources. `model` is a
/// GGUF `Embedder` (candle) selected at load by `embedder::load_embedder`.
struct Runtime {
    model: Box<dyn Embedder>,
    db: VectorDb,
    /// Prefix prepended to each search query before embedding (from the loaded
    /// model's deploy.json `searchQueryPrefix`). "" for symmetric models. Used
    /// in `search` on the query side of an asymmetric embedding model.
    search_query_prefix: String,
    /// Prefix prepended to each imported document chunk before embedding (from the loaded
    /// model's deploy.json `importDocPrefix`). "" for symmetric models. Used
    /// in `reindex_doc` on the document side.
    import_doc_prefix: String,
    /// Model-author-recommended chunk size in tokens (deploy.json `chunkSize`),
    /// `None` if unset. Used as the default chunk size when the user's global
    /// `chunk_size` setting is `0` ("auto"); a positive user setting overrides.
    deploy_chunk_size: Option<u32>,
    /// Model-author-recommended chunk overlap (deploy.json `chunkOverlap`),
    /// `None` if unset. Same auto-vs-override semantics as `deploy_chunk_size`.
    deploy_chunk_overlap: Option<u32>,
}

static RUNTIME: OnceLock<Mutex<Option<Runtime>>> = OnceLock::new();
static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static INITIALIZING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Set when the vector table was recreated on the last enable (model swapped
/// to a different embedding dim). The frontend reads `RagStatus.needs_reindex`
/// and triggers `reindex_all`; that clears the flag when done. Old doc
/// embeddings are gone (table recreated), so the docs are still on disk but
/// search returns nothing until re-indexed with the new model.
static NEEDS_REINDEX: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn runtime() -> &'static Mutex<Option<Runtime>> {
    RUNTIME.get_or_init(|| Mutex::new(None))
}

/// `<app_data_dir>/rag` — holds `files/` (doc content + meta) and `lancedb/`.
pub fn data_dir(app: &AppHandle) -> Result<PathBuf> {
    Ok(app.path().app_data_dir()?.join("rag"))
}

fn files_dir(app: &AppHandle) -> Result<PathBuf> {
    Ok(data_dir(app)?.join("files"))
}

fn lancedb_dir(app: &AppHandle) -> Result<PathBuf> {
    Ok(data_dir(app)?.join("lancedb"))
}

/// Bundled model root. In **dev** (`tauri dev`) the source dir
/// (`CARGO_MANIFEST_DIR/runtimes/rag/model`) is preferred so edits to the
/// model files take effect live WITHOUT tauri recopying them to the target
/// dir - and so deleted size dirs don't linger as stale copies under
/// `target/debug/runtimes/` (tauri copies resources to target but does NOT
/// remove dirs deleted from source, which made phantom `f16`/`q4` entries
/// appear in the dropdown). In a packaged build the source path doesn't exist
/// on the user's machine, so we fall back to the bundled resource dir.
fn model_root(app: &AppHandle) -> Result<PathBuf> {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("runtimes")
        .join("rag")
        .join("model");
    if src.exists() {
        return Ok(src);
    }
    if let Ok(resource) = app.path().resource_dir() {
        let p = resource.join("runtimes").join("rag").join("model");
        if p.exists() {
            return Ok(p);
        }
    }
    Ok(src)
}

/// Writable store for DOWNLOADED models: `<app_data>/rag/models/<family>/<size>/`.
/// The bundled resource dir is read-only in a signed app, so models fetched via
/// `download.url` land here. Mirrors the bundled layout.
fn download_root(app: &AppHandle) -> Result<PathBuf> {
    Ok(data_dir(app)?.join("models"))
}

/// The out-of-box default ready size: the size whose `deploy.json` has
/// `"default": true` (and is ready), else the first ready size. Also the
/// fallback when the persisted selection is gone (a model deleted in a later
/// version). Sync (scans the model dirs).
fn default_size(app: &AppHandle) -> Option<String> {
    let models = list_models(app).ok()?;
    models
        .iter()
        .find(|m| m.is_default && m.ready)
        .or_else(|| models.iter().find(|m| m.ready))
        .map(|m| m.size.clone())
}

/// The model's context window in tokens, from the SELECTED (or default) size
/// dir's `config.json` `max_position_embeddings`. Used by the UI to cap the
/// `chunk_size` input. Async because it reads the persisted selection; reads
/// the file directly (no runtime) so the bound is available with RAG off.
/// GGUF size dirs ship a config.json, so this works without loading the model.
/// Falls back to 2048 if no size resolves.
pub async fn model_max_context(app: &AppHandle) -> u32 {
    let size = current_model().await.or_else(|| default_size(app));
    let dir = size.and_then(|s| resolve_model_paths(app, &s).ok().flatten());
    dir.map(|d| read_max_context(&d)).unwrap_or(2048)
}

/// The loaded model's chunk-size recommendation: `(max_context, chunk_size?,
/// chunk_overlap?)`. The chunk values come from the selected size's
/// `deploy.json` (`chunkSize` / `chunkOverlap`) — the model author's
/// recommended retrieval granularity. `None` when the size dir isn't ready or
/// the deploy.json omits the fields (the service falls back to 1024/100).
/// Surfaced via `rag_model_limits` so the frontend's Auto mode can SHOW the
/// resolved values (sliders disabled) and seed them when the user switches to
/// manual. `max_context` is read the same way as `model_max_context`.
pub async fn model_chunk_recommendation(app: &AppHandle) -> (u32, Option<u32>, Option<u32>) {
    let size = current_model().await.or_else(|| default_size(app));
    let dir = size.and_then(|s| resolve_model_paths(app, &s).ok().flatten());
    let Some(d) = dir else {
        return (2048, None, None);
    };
    let max_ctx = read_max_context(&d);
    let cfg = crate::rag::embedder::read_deploy_config(&d);
    (max_ctx, cfg.chunk_size, cfg.chunk_overlap)
}

/// Sum the on-disk size of the model file(s) in a ready size dir: any `*.gguf`
/// file (bundled `model.gguf` or downloaded `model.gguf`). Small aux files
/// (tokenizer.json/config.json) are excluded so the number reflects the model
/// payload only.
fn model_file_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".gguf") {
                if let Ok(m) = e.metadata() {
                    total += m.len();
                }
            }
        }
    }
    total
}

/// The parsed `download.url` (stage-18 JSON format): a `type` (must be "gguf")
/// + an array of model-file URLs to fetch.
#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DownloadUrl {
    #[serde(default, rename = "type")]
    format: String,
    #[serde(default)]
    model_url: Vec<String>,
}

fn read_download_url(path: &Path) -> Result<DownloadUrl> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow!("read {}: {}", path.display(), e))?;
    let dl = serde_json::from_str::<DownloadUrl>(&text)
        .map_err(|e| anyhow!("parse {} as JSON ({{type, modelUrl}}): {}", path.display(), e))?;
    if !dl.format.is_empty() && dl.format != "gguf" {
        return Err(anyhow!(
            "download.url in {} has type '{}' - only 'gguf' is supported (ONNX backend was removed)",
            path.display(),
            dl.format
        ));
    }
    Ok(dl)
}

// ── model selection ─────────────────────────────────────────────────────────

/// One selectable model size, surfaced to the frontend dropdown.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RagModelInfo {
    /// Size key, e.g. "default", "q4", "f16". Used as the selection key
    /// (persisted in `config_json.rag.model`).
    pub size: String,
    /// Dropdown label, e.g. "model_q4".
    pub label: String,
    /// "ready" (model file present, bundled or downloaded) | "downloadable"
    /// (only download.url, not yet downloaded) | "unavailable".
    pub status: String,
    /// True if the model file (*.gguf) is available now
    /// (selectable).
    pub ready: bool,
    /// True if a download.url exists (can be fetched).
    pub downloadable: bool,
    /// Backend format: "gguf" | "" (not ready). Drives the strategy
    /// (`embedder::load_embedder`) and shows as a dropdown badge. For
    /// downloadable sizes the future format is read from download.url's `type`
    /// so the badge shows even before download.
    #[serde(default)]
    pub format: String,
    /// Total size in bytes of the model file(s) on disk (ready sizes only);
    /// 0 for downloadable sizes (unknown until downloaded). Shown in the
    /// dropdown.
    #[serde(default)]
    pub file_size: u64,
    /// Human description from the size dir's `deploy.json` `"description"` field
    /// (shown in the dropdown next to the file size). Empty when absent.
    #[serde(default)]
    pub description: String,
    /// True if this size's `deploy.json` has `"default": true` - the out-of-box
    /// model and the fallback when the persisted selection is gone (a model
    /// deleted in a later version). Surfaced so the dropdown can badge it.
    #[serde(default, rename = "default")]
    pub is_default: bool,
    /// Sort order from deploy.json `"sort"` (lower = higher in dropdown; 0 default).
    #[serde(default)]
    pub sort: i32,
}

/// Scan `model/<family>/<size>/` and return one entry per size. A size is
/// "ready" if its size dir (downloaded copy preferred, else the bundled dir)
/// contains a `*.gguf` file; "downloadable" if a `download.url` is
/// present but no model file yet. `format` is detected from the file present
/// (ready) or read from download.url's `type` (downloadable).
pub fn list_models(app: &AppHandle) -> Result<Vec<RagModelInfo>> {
    let root = model_root(app)?;
    let dl_root = download_root(app)?;
    let mut out: Vec<RagModelInfo> = Vec::new();
    if !root.exists() {
        return Ok(out);
    }
    for fam in std::fs::read_dir(&root)? {
        let fam_path = fam?.path();
        if !fam_path.is_dir() {
            continue;
        }
        let fam_name = fam_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        for sz in std::fs::read_dir(&fam_path)? {
            let sz_path = sz?.path();
            if !sz_path.is_dir() {
                continue;
            }
            let size = sz_path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            if size.is_empty() {
                continue;
            }
            let downloaded_dir = dl_root.join(fam_name).join(&size);
            // Ready dir = downloaded copy if it has a model file, else bundled.
            // `detect_format` returns "gguf"/"" by file presence.
            let ready_dir = if !detect_format(&downloaded_dir).is_empty() {
                Some(downloaded_dir)
            } else if !detect_format(&sz_path.clone()).is_empty() {
                Some(sz_path.clone())
            } else {
                None
            };
            let (ready, format, file_size) = match &ready_dir {
                Some(d) => (true, detect_format(d).to_string(), model_file_size(d)),
                None => (false, String::new(), 0u64),
            };
            let downloadable = sz_path.join("download.url").exists() && !ready;
            // For downloadable sizes, surface the FUTURE format from
            // download.url's `type` so the badge shows before download.
            let format = if !format.is_empty() {
                format
            } else if downloadable {
                read_download_url(&sz_path.join("download.url"))
                    .map(|d| d.format)
                    .unwrap_or_default()
            } else {
                String::new()
            };
            let deploy_cfg = crate::rag::embedder::read_deploy_config(&sz_path);
            out.push(RagModelInfo {
                // Label = "<family-dir>-<size-dir>" (e.g. "embeddinggemma-default"),
                // so multiple families/sizes are distinguishable in the dropdown.
                label: format!("{}-{}", fam_name, size),
                status: if ready {
                    "ready".into()
                } else if downloadable {
                    "downloadable".into()
                } else {
                    "unavailable".into()
                },
                ready,
                downloadable,
                format,
                file_size,
                // Description, default flag, sort order from the bundled size
                // dir's deploy.json (one read).
                description: deploy_cfg.description.clone(),
                is_default: deploy_cfg.is_default,
                sort: deploy_cfg.sort,
                size,
            });
        }
    }
    // `read_dir` returns entries in filesystem (arbitrary) order; sort by label
    // for a stable ASCII-ordered dropdown (default < f16 < q4 < quantized ...).
    // Sort by deploy.json `sort` (lower = higher), then label as tiebreaker.
    out.sort_by(|a, b| a.sort.cmp(&b.sort).then(a.label.cmp(&b.label)));
    Ok(out)
}

/// Resolve the size dir for the given size: the downloaded copy (under
/// `<app_data>/rag/models/<family>/<size>/`) if it has a model file, else the
/// bundled size dir if it has a model file. Returns `None` if the size isn't
/// found or not ready (download.url only). Each size dir is self-contained
/// (holds a *.gguf file + tokenizer.json + config.json).
fn resolve_model_paths(app: &AppHandle, size: &str) -> Result<Option<PathBuf>> {
    let root = model_root(app)?;
    let dl_root = download_root(app)?;
    for fam in std::fs::read_dir(&root)? {
        let fam_path = fam?.path();
        if !fam_path.is_dir() {
            continue;
        }
        let fam_name = fam_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let bundled = fam_path.join(size);
        if !bundled.is_dir() {
            continue;
        }
        let downloaded_dir = dl_root.join(fam_name).join(size);
        let size_dir = if !detect_format(&downloaded_dir).is_empty() {
            downloaded_dir
        } else if !detect_format(&bundled).is_empty() {
            bundled
        } else {
            return Ok(None);
        };
        return Ok(Some(size_dir));
    }
    Ok(None)
}

/// The persisted selected model size (`config_json.rag.model`), or None.
pub async fn current_model() -> Option<String> {
    crate::services::config_service::get()
        .await
        .ok()
        .and_then(|c| {
            c.get("rag")
                .and_then(|r| r.get("model"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
}

/// Persist `rag.model = <size>` (deep-merge; leaves other rag settings intact).
async fn set_current_model(size: &str) {
    let patch = json!({ "rag": { "model": size } });
    if let Err(e) = crate::services::config_service::update(&patch).await {
        rag_log("warn", format!("failed to persist rag.model: {}", e));
    }
}

/// Select a model size: persist it, then auto-restart RAG if currently enabled
/// (stop + start reloads the new model). If RAG is off, the next enable loads
/// it. Returns the post-restart status (with `needs_reindex` if the dim
/// changed). Errors if the size isn't ready.
pub async fn select_model(app: &AppHandle, size: &str) -> Result<RagStatus> {
    if resolve_model_paths(app, size)?.is_none() {
        return Err(anyhow!(
            "model '{}' is not ready - download it first",
            size
        ));
    }
    set_current_model(size).await;
    rag_log("info", format!("selected model size '{}'", size));
    if is_enabled() {
        stop().await;
        start(app).await?;
    }
    Ok(status())
}

// ── model download ──────────────────────────────────────────────────────────

/// Progress for a model download, emitted on `rag://model-download` so the
/// dropdown's download item can show a rich progress bar. `phase` is
/// "downloading" | "done" | "error". The bar shows total %, speed (B/s), ETA
/// (seconds), and which file out of how many.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RagModelDownloadProgress {
    size: String,
    phase: String,
    /// Bytes downloaded so far (phase "downloading"), cumulative across files.
    downloaded: u64,
    /// Total bytes across all files (0 if unknown).
    total: u64,
    /// 0..100 (best-effort; 0 when total unknown).
    percent: u8,
    /// Download speed in bytes/sec (sliding-window estimate; 0 at the start).
    speed: u64,
    /// Estimated seconds remaining (0 if unknown / just started).
    eta: u64,
    /// 1-based index of the file currently downloading.
    file_current: u32,
    /// Total number of files in this model (length of download.url modelUrl).
    file_total: u32,
    /// Human message (current file name / "done").
    message: Option<String>,
}

const MODEL_DOWNLOAD_EVENT: &str = "rag://model-download";

fn emit_model_download(app: &AppHandle, p: RagModelDownloadProgress) {
    if let Err(e) = app.emit(MODEL_DOWNLOAD_EVENT, &p) {
        log::warn!("[RAG] emit model-download failed: {e}");
    }
}

/// Download a model size via its `download.url` (stage-18 JSON format:
/// `{"type":"gguf", "modelUrl":[...]}`). Each URL is streamed directly into
/// `<app_data>/rag/models/<family>/<size>/`: file 0 -> `model.gguf`; additional
/// URLs (if any) keep their URL basename. After success the size becomes
/// "ready" and selectable. Emits `rag://model-download` throughout with
/// cumulative %, speed, ETA, and file index/total.
pub async fn download_model(app: &AppHandle, size: &str) -> Result<()> {
    // Locate the download.url + family for this size.
    let root = model_root(app)?;
    let dl_root = download_root(app)?;
    let mut family: Option<String> = None;
    let mut dl_url: Option<DownloadUrl> = None;
    for fam in std::fs::read_dir(&root)? {
        let fam_path = fam?.path();
        if !fam_path.is_dir() {
            continue;
        }
        let url_file = fam_path.join(size).join("download.url");
        if url_file.exists() {
            family = fam_path
                .file_name()
                .and_then(|n| n.to_str())
                .map(String::from);
            dl_url = Some(read_download_url(&url_file)?);
            break;
        }
    }
    let family = family.ok_or_else(|| anyhow!("family dir not found for '{}'", size))?;
    let dl_url = dl_url.ok_or_else(|| anyhow!("no download.url found for model '{}'", size))?;
    if dl_url.model_url.is_empty() {
        return Err(anyhow!(
            "download.url for '{}' has no modelUrl entries",
            size
        ));
    }
    let fmt = dl_url.format.as_str();
    let urls = dl_url.model_url.clone();
    let file_total = urls.len() as u32;
    let target_dir = dl_root.join(&family).join(size);
    std::fs::create_dir_all(&target_dir)?;

    rag_log(
        "info",
        format!(
            "downloading model '{}': format={} files={}",
            size, fmt, urls.len()
        ),
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3600))
        .build()
        .map_err(|e| anyhow!("http client: {}", e))?;

    // HEAD each URL first to learn the total size (for the cumulative % bar +
    // ETA). Missing/zero content-length is tolerated (total stays 0 -> bar
    // shows speed + file count but not %).
    let mut sizes: Vec<u64> = Vec::with_capacity(urls.len());
    for url in &urls {
        let len = client
            .head(url.as_str())
            .send()
            .await
            .ok()
            .and_then(|r| r.error_for_status().ok())
            .and_then(|r| r.content_length())
            .unwrap_or(0);
        sizes.push(len);
    }
    let total: u64 = sizes.iter().sum();

    // Download each file sequentially, accumulating cumulative progress across
    // files. Speed/ETA use a sliding window reset every emit tick.
    let mut cumulative: u64 = 0;
    for (idx, url) in urls.iter().enumerate() {
        let file_current = (idx as u32) + 1;
        // Output filename: file 0 -> model.gguf; others keep URL basename.
        let out_name = if idx == 0 {
            "model.gguf".to_string()
        } else {
            url_basename(url)
        };
        let out_path = target_dir.join(&out_name);

        emit_model_download(
            app,
            RagModelDownloadProgress {
                size: size.to_string(),
                phase: "downloading".into(),
                downloaded: cumulative,
                total,
                percent: pct(cumulative, total),
                speed: 0,
                eta: 0,
                file_current,
                file_total,
                message: Some(out_name.clone()),
            },
        );

        let resp = client
            .get(url.as_str())
            .send()
            .await
            .map_err(|e| anyhow!("{} request: {}", out_name, e))?
            .error_for_status()
            .map_err(|e| anyhow!("{} status: {}", out_name, e))?;
        {
            use futures_util::StreamExt;
            use tokio::io::AsyncWriteExt;
            let mut file = tokio::fs::File::create(&out_path)
                .await
                .map_err(|e| anyhow!("create {}: {}", out_name, e))?;
            let mut stream = resp.bytes_stream();
            let mut last_emit = std::time::Instant::now();
            let mut since_emit: u64 = 0; // bytes since last speed sample
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| anyhow!("{} stream: {}", out_name, e))?;
                file.write_all(&chunk)
                    .await
                    .map_err(|e| anyhow!("write {}: {}", out_name, e))?;
                let n = chunk.len() as u64;
                cumulative = cumulative.saturating_add(n);
                since_emit = since_emit.saturating_add(n);
                if last_emit.elapsed() >= std::time::Duration::from_millis(200) {
                    let elapsed_secs = last_emit.elapsed().as_secs_f64().max(0.001);
                    let speed = (since_emit as f64 / elapsed_secs) as u64;
                    let downloaded = cumulative;
                    let percent = pct(downloaded, total);
                    let eta = if speed > 0 && total > downloaded {
                        (total - downloaded) / speed
                    } else {
                        0
                    };
                    emit_model_download(
                        app,
                        RagModelDownloadProgress {
                            size: size.to_string(),
                            phase: "downloading".into(),
                            downloaded,
                            total,
                            percent,
                            speed,
                            eta,
                            file_current,
                            file_total,
                            message: Some(out_name.clone()),
                        },
                    );
                    last_emit = std::time::Instant::now();
                    since_emit = 0;
                }
            }
            file.flush()
                .await
                .map_err(|e| anyhow!("flush {}: {}", out_name, e))?;
        }
        let _ = &sizes[idx]; // per-file size available if needed for logging
    }

    // Verify the model file landed.
    let model_file = "model.gguf";
    if !target_dir.join(model_file).exists() {
        return Err(anyhow!("{} download failed for '{}'", model_file, size));
    }

    rag_log(
        "info",
        format!("model '{}' downloaded ({} files)", size, file_total),
    );
    emit_model_download(
        app,
        RagModelDownloadProgress {
            size: size.to_string(),
            phase: "done".into(),
            downloaded: total,
            total,
            percent: 100,
            speed: 0,
            eta: 0,
            file_current: file_total,
            file_total,
            message: Some("done".into()),
        },
    );
    Ok(())
}

/// 0..100 percent of `done / total`; 0 when total is unknown.
fn pct(done: u64, total: u64) -> u8 {
    if total == 0 {
        0
    } else {
        ((done as f64 / total as f64) * 100.0).min(100.0) as u8
    }
}

/// Basename of a URL's path - used to save an additional model file under its
/// URL basename. Strips a trailing query string first; falls back to
/// "model_data.bin".
fn url_basename(url: &str) -> String {
    let path = url.split('?').next().unwrap_or(url);
    path.rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("model_data.bin")
        .to_string()
}

// ── lifecycle ───────────────────────────────────────────────────────────────

/// Enable or disable RAG. Enabling blocks until the model + vector DB are
/// ready (the frontend shows "opening" while this runs). Persists the
/// intent to `config_json.rag.enabled` so it survives restarts.
pub async fn toggle(app: &AppHandle, enabled: bool) -> Result<RagStatus> {
    if enabled {
        start(app).await?;
    } else {
        stop().await;
    }
    // Persist the intent (only reached on success — start() returns early on
    // failure, so a failed enable leaves the previous intent unchanged).
    persist_enabled(app, enabled).await;
    Ok(status())
}

/// Read the persisted `rag.enabled` intent from config (used at startup to
/// decide whether to auto-restore the RAG runtime).
pub async fn config_enabled() -> bool {
    crate::services::config_service::get()
        .await
        .ok()
        .and_then(|c| c.get("rag").and_then(|r| r.get("enabled")).and_then(|v| v.as_bool()))
        .unwrap_or(false)
}

/// Persist `rag.enabled` (deep-merges into the rag config, leaving weights etc.
/// intact).
async fn persist_enabled(app: &AppHandle, enabled: bool) {
    let _ = app; // app available for future path needs; config write is global.
    let patch = json!({ "rag": { "enabled": enabled } });
    if let Err(e) = crate::services::config_service::update(&patch).await {
        rag_log("warn", format!("failed to persist rag.enabled: {}", e));
    }
}

pub async fn start(app: &AppHandle) -> Result<()> {
    INITIALIZING.store(true, std::sync::atomic::Ordering::SeqCst);
    rag_log("info", "enabling RAG (loading embedding model + opening vector DB)…");
    let res = async {
        check_memory_sufficient()?;
        // Resolve the selected model size: the persisted selection, else the
        // out-of-box default (the size whose deploy.json has "default": true).
        // If the persisted size can't be resolved (deleted in a later version),
        // fall back to the default and persist it so we don't keep trying the
        // gone one. The size dir is self-contained (*.gguf file +
        // tokenizer + config); `embedder::load_embedder` detects the format.
        let size = {
            let persisted = current_model().await;
            let resolved = persisted
                .as_ref()
                .and_then(|s| resolve_model_paths(app, s).ok().flatten());
            match resolved {
                // Persisted selection is still ready - use it.
                Some(_) => persisted.unwrap(),
                None => {
                    // No selection, OR the persisted one is gone (deleted) -
                    // fall back to the default ready size and persist it.
                    let default = default_size(app)
                        .ok_or_else(|| anyhow!("no ready model - download one first"))?;
                    if persisted.as_deref() != Some(&default) {
                        rag_log(
                            "info",
                            format!(
                                "selected model '{}' not available - falling back to default '{}'",
                                persisted.as_deref().unwrap_or("(none)"),
                                default
                            ),
                        );
                        set_current_model(&default).await;
                    }
                    default
                }
            }
        };
        let size_dir = resolve_model_paths(app, &size)?
            .ok_or_else(|| anyhow!("model '{}' not ready - download it first", size))?;
        let model = load_embedder(&size_dir)?;
        // Read the model's deploy.json once for the asymmetric embedding
        // prefixes (searchQueryPrefix / importDocPrefix) - applied per-call in
        // `search` (query side) and `reindex_doc` (document side). "" for
        // symmetric models (Gemma) so behavior is unchanged when unset.
        let deploy = crate::rag::embedder::read_deploy_config(&size_dir);
        // Model name = "<family>-<size>" (the dropdown label, e.g.
        // "embeddinggemma-default") - derived from the resolved size dir so it
        // works for the GGUF backend and matches what the user sees.
        let family = size_dir
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("");
        let model_name = format!("{}-{}", family, size);
        // Surface the model + backend + execution-provider decision to the Logs
        // page so slow imports can be diagnosed. `backend()` = gguf;
        // `ep_label()` carries the EP detail (e.g. "Metal", "CoreML+CPU", "CPU").
        // This is THE signal for whether the model is on GPU or stuck on CPU.
        rag_log(
            "info",
            format!(
                "model loaded: name={} dim={} max_context={} backend={} ep={} query_prefix={:?} doc_prefix={:?}",
                model_name,
                model.embed_dim(),
                model.max_context(),
                model.backend(),
                model.ep_label(),
                deploy.search_query_prefix,
                deploy.import_doc_prefix,
            ),
        );
        // Open the vector DB for the loaded model's embed_dim. If an existing
        // table has a different dim (model swapped), it's dropped+recreated —
        // `db.needs_reindex()` then tells us to re-index all docs (old
        // embeddings are gone / meaningless under the new model).
        let db = VectorDb::open(&lancedb_dir(app)?, model.embed_dim()).await?;
        let needs_reindex = db.needs_reindex();
        if needs_reindex {
            // Old embeddings are gone; zero the on-disk `.meta` chunk_count so
            // the list view reflects reality (0) until reindex repopulates.
            zero_all_chunk_counts(app)?;
        }
        let mut guard = runtime().lock().await;
        *guard = Some(Runtime {
            model,
            db,
            search_query_prefix: deploy.search_query_prefix,
            import_doc_prefix: deploy.import_doc_prefix,
            deploy_chunk_size: deploy.chunk_size,
            deploy_chunk_overlap: deploy.chunk_overlap,
        });
        let rss_after_load = crate::rag::embedder::process_rss_mib().unwrap_or(0);
        rag_log("info", format!("model stored in runtime (RSS: {} MiB)", rss_after_load));
        Ok::<_, anyhow::Error>(needs_reindex)
    }
    .await;
    INITIALIZING.store(false, std::sync::atomic::Ordering::SeqCst);
    let needs_reindex = match res {
        Ok(r) => r,
        Err(e) => {
            rag_log("error", format!("failed to enable RAG: {:#}", e));
            return Err(e);
        }
    };
    ENABLED.store(true, std::sync::atomic::Ordering::SeqCst);
    NEEDS_REINDEX.store(needs_reindex, std::sync::atomic::Ordering::SeqCst);
    // Startup reconciliation: rebuild the SQL mirror tables (rag_docs /
    // rag_doc_tags / rag_tags) from the on-disk .meta files. Cheap (one scan +
    // one transaction) and heals any drift from a crash mid-CRUD or manual
    // .meta edits. Non-fatal: queries fall back to whatever is in the tables.
    if let Err(e) = rebuild_rag_sql_index(app).await {
        rag_log("warn", format!("startup rebuild_rag_sql_index failed: {}", e));
    }
    if needs_reindex {
        rag_log("info", "RAG enabled (model + vector DB ready); embedding dim changed — reindex required (frontend will prompt)");
    } else {
        rag_log("info", "RAG enabled (model + vector DB ready)");
    }
    // Re-arm the auto doc-update timer now that the runtime is up (a no-op if
    // the setting is off — the loop exits on its first settings read). The
    // generation bump retires any loop left over from a previous enable.
    restart_auto_update_timer(app);
    Ok(())
}

pub async fn stop() {
    let rss_before = crate::rag::embedder::process_rss_mib().unwrap_or(0);
    let mut guard = runtime().lock().await;
    if let Some(rt) = guard.take() {
        rag_log("info", "stop: runtime found, dropping model + db");
        let Runtime { model, db, .. } = rt;
        drop(db);    // lancedb Connection -> freed
        // Drop the model (candle GgufEmbedder). For candle:
        //   CPU: Tensors (CpuStorage Vec<f32>) freed by Rust drop; mi_collect
        //        returns freed pages to OS.
        //   Metal: Tensors (MetalStorage Arc<Buffer>) freed when the buffer
        //        pool Arc hits 0 (all MetalDevice clones dropped). Metal
        //        framework releases GPU buffers (not mimalloc-managed).
        drop(model);
    } else {
        rag_log("info", "stop: no runtime (model was never loaded or already stopped)");
    }
    drop(guard);
    ENABLED.store(false, std::sync::atomic::Ordering::SeqCst);
    NEEDS_REINDEX.store(false, std::sync::atomic::Ordering::SeqCst);
    // Use the `libmimalloc-sys` binding (not a raw extern): referencing the
    // crate forces its static archive into this (cdylib) link, so the
    // `mi_collect` symbol resolves. See Cargo.toml for why the bin's
    // #[global_allocator] alone isn't enough for the lib.
    unsafe { libmimalloc_sys::mi_collect(true); }
    // Wait 5s for mimalloc's purge_delay (default 10ms) to complete so the
    // RSS measurement reflects the actual freed memory (MADV_DONTNEED returns
    // pages to OS asynchronously on macOS).
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    let rss_after = crate::rag::embedder::process_rss_mib().unwrap_or(0);
    rag_log(
        "info",
        format!(
            "RAG disabled (RSS: {} -> {} MiB, freed {} MiB, mi_collect done)",
            rss_before, rss_after, rss_before.saturating_sub(rss_after)
        ),
    );
}

// `mi_collect` is provided by `libmimalloc-sys` (the `extended` feature) —
// used above to force mimalloc to return freed pages to the OS after RAG
// shutdown. See Cargo.toml for the linkage rationale.

pub fn status() -> RagStatus {
    RagStatus {
        enabled: ENABLED.load(std::sync::atomic::Ordering::SeqCst),
        initializing: INITIALIZING.load(std::sync::atomic::Ordering::SeqCst),
        needs_reindex: NEEDS_REINDEX.load(std::sync::atomic::Ordering::SeqCst),
    }
}

/// Whether RAG is enabled (runtime loaded). Used by the MCP layer to decide
/// whether to advertise `rag_search` / `rag_get`.
pub fn is_enabled() -> bool {
    ENABLED.load(std::sync::atomic::Ordering::SeqCst)
}

/// The app-level RAG tool definitions (name / description / inputSchema) that
/// the MCP `tools/list` advertises while RAG is enabled. Single source of
/// truth - consumed both by the HTTP MCP layer (`dispatch_mcp`) and by the
/// `rag_tools` command that powers the "view tools" dialog in the UI.
pub fn tool_definitions() -> Vec<serde_json::Value> {
    vec![
        json!({
            "name": "rag_search",
            "description": "Search RAG documents by semantic similarity. Returns matching text fragments with their document ids, titles, and similarity scores. Optionally filter to documents that have any of the given tags.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "The search query." },
                    "tags": { "type": "array", "items": { "type": "string" }, "description": "Optional tag filter: only return documents that have at least one of these tags." }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "rag_get",
            "description": "Get the full text content of a RAG document by its id (as returned by rag_search).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "docId": { "type": "string", "description": "The document id." }
                },
                "required": ["docId"]
            }
        }),
        json!({
            "name": "rag_tag_search",
            "description": "List distinct tags in the RAG library. Pass `search_key` (an array of strings) to filter tags by case-insensitive substring (returns tags matching any key); omit/empty to return all tags.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "search_key": { "type": "array", "items": { "type": "string" }, "description": "Optional substring filters; returns tags matching any of them." }
                },
                "required": []
            }
        }),
        json!({
            "name": "rag_file_create",
            "description": "Create a new RAG document from UTF-8 text content and index it for semantic search. The file is stored as {docName}.{docType} (e.g. readme.md). Returns the created docId. Overwrites any existing document with the same filename.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "docName": { "type": "string", "description": "Document name (without extension, e.g. \"readme\"). Path separators are sanitized to dashes." },
                    "docType": { "type": "string", "description": "File type as a bare extension without dot, e.g. \"md\", \"java\", \"py\", \"txt\"." },
                    "docContent": { "type": "string", "description": "Full text content of the document (UTF-8). Encoding detection is skipped - the input must already be valid UTF-8." },
                    "tags": { "type": "array", "items": { "type": "string" }, "description": "Optional tags to attach to the document (used for tag-filtered searches)." }
                },
                "required": ["docName", "docType", "docContent"]
            }
        }),
        json!({
            "name": "rag_file_update",
            "description": "Update an existing RAG document by docId. Any of docName/docType/docContent can be omitted (only supplied fields are updated). When docContent is provided, docContentAppend decides whether to append to the existing content (true) or replace it (false, default); the document is re-indexed so semantic search reflects the new text. docId stays the same.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "docId": { "type": "string", "description": "The document id (as returned by rag_file_create or rag_search)." },
                    "docName": { "type": "string", "description": "New document name (optional)." },
                    "docType": { "type": "string", "description": "New file type as a bare extension, e.g. \"md\" (optional)." },
                    "docContent": { "type": "string", "description": "New text content (optional). When set, docContentAppend controls append vs replace." },
                    "docContentAppend": { "type": "boolean", "description": "Only meaningful when docContent is set. true = append to existing content; false (default) = replace. Ignored if docContent is omitted.", "default": false },
                    "addTags": { "type": "array", "items": { "type": "string" }, "description": "Optional tags to add to the document (case-insensitive dedup; preserves existing order)." },
                    "removeTags": { "type": "array", "items": { "type": "string" }, "description": "Optional tags to remove from the document (case-insensitive match)." }
                },
                "required": ["docId"]
            }
        }),
        json!({
            "name": "rag_file_delete",
            "description": "Delete a RAG document by docId. Removes its content file, metadata, and all its vector chunks from the index. Irreversible.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "docId": { "type": "string", "description": "The document id (as returned by rag_file_create or rag_search)." }
                },
                "required": ["docId"]
            }
        }),
    ]
}

/// The reserved name of the builtin "mcphub-desktop" server. This single
/// virtual server bundles the app's built-in capabilities - RAG tools (when
/// RAG is enabled) + the builtin prompts + the builtin resources - so groups
/// manage them uniformly as one server's capabilities (per-server tool/
/// prompt/resource selection), not via separate group-level fields. Custom
/// servers may not use this name (server_service rejects it on create/update).
pub const BUILTIN_SERVER_NAME: &str = "mcphub-desktop";

/// The RAG tools as proper `Tool` structs (for `list_servers`, which injects
/// the builtin RAG server into the server list so the frontend can treat it
/// uniformly - select it in groups, view its tools, etc.).
pub fn builtin_tools() -> Vec<crate::models::server::Tool> {
    use crate::models::server::Tool;
    tool_definitions()
        .into_iter()
        .map(|v| Tool {
            name: v.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(),
            description: v.get("description").and_then(|d| d.as_str()).map(String::from),
            input_schema: v.get("inputSchema").cloned().unwrap_or(json!({})),
            server_name: BUILTIN_SERVER_NAME.to_string(),
            enabled: true,
            annotations: None,
            output_schema: None,
        })
        .collect()
}

/// Dispatch a builtin RAG tool call (`rag_search` / `rag_get` / `rag_tag_search`
/// / `rag_file_create` / `rag_file_update` / `rag_file_delete`) and return a
/// `ToolCallResult`. Single source of truth - used by both the HTTP MCP
/// dispatch path (`http_server.rs`) and the Tauri `call_tool` command
/// (`pool::call_tool` routes builtin server names here, so invoking a RAG tool
/// from the "servers" panel works instead of erroring "not connected").
///
/// Requires RAG enabled (same guard as the HTTP path); `rag_get`/`rag_tag_search`
/// are read-only but kept under the same flag for consistency.
pub async fn call_builtin_tool(
    app: &AppHandle,
    tool_name: &str,
    args: &serde_json::Value,
) -> Result<crate::models::server::ToolCallResult> {
    use crate::models::server::ToolCallResult;
    if !is_enabled() {
        return Err(anyhow!("RAG is not enabled"));
    }
    let text_content = |s: String| vec![serde_json::json!({ "type": "text", "text": s })];
    let ok = |s: String| ToolCallResult {
        content: text_content(s),
        is_error: false,
        structured_content: None,
    };
    let str_arr = |key: &str| -> Vec<String> {
        args.get(key)
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|t| t.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default()
    };
    let doc_id = || {
        args.get("docId")
            .and_then(|v| v.as_str())
            .or_else(|| args.get("id").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string()
    };
    match tool_name {
        "rag_search" => {
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let results = search(query.to_string(), str_arr("tags")).await?;
            let text = serde_json::to_string_pretty(&results).unwrap_or_default();
            Ok(ok(text))
        }
        "rag_get" => {
            let id = doc_id();
            match get_doc(app, &id).await? {
                Some(doc) => Ok(ok(doc.content)),
                None => Err(anyhow!("document not found: {}", id)),
            }
        }
        "rag_tag_search" => {
            let tags = list_tags(str_arr("search_key")).await?;
            let text = serde_json::to_string_pretty(&tags).unwrap_or_else(|_| "[]".to_string());
            Ok(ok(text))
        }
        "rag_file_create" => {
            let doc_name = args.get("docName").and_then(|v| v.as_str()).unwrap_or("");
            let doc_type = args.get("docType").and_then(|v| v.as_str()).unwrap_or("");
            let doc_content = args.get("docContent").and_then(|v| v.as_str()).unwrap_or("");
            if doc_name.is_empty() || doc_type.is_empty() || doc_content.is_empty() {
                return Err(anyhow!("docName, docType and docContent are all required"));
            }
            let id = create_doc_from_content(app, doc_name, doc_type, doc_content, str_arr("tags")).await?;
            let text = serde_json::to_string(&serde_json::json!({ "docId": id })).unwrap_or_default();
            Ok(ok(text))
        }
        "rag_file_update" => {
            let id = doc_id();
            if id.is_empty() {
                return Err(anyhow!("docId is required"));
            }
            let name = args.get("docName").and_then(|v| v.as_str());
            let doc_type = args.get("docType").and_then(|v| v.as_str());
            let content = args.get("docContent").and_then(|v| v.as_str());
            let append = args.get("docContentAppend").and_then(|v| v.as_bool()).unwrap_or(false);
            update_doc(app, &id, name, doc_type, content, append, str_arr("addTags"), str_arr("removeTags")).await?;
            let text = serde_json::to_string(&serde_json::json!({ "docId": id, "updated": true })).unwrap_or_default();
            Ok(ok(text))
        }
        "rag_file_delete" => {
            let id = doc_id();
            if id.is_empty() {
                return Err(anyhow!("docId is required"));
            }
            delete_doc(app, &id).await?;
            let text = serde_json::to_string(&serde_json::json!({ "docId": id, "deleted": true })).unwrap_or_default();
            Ok(ok(text))
        }
        _ => Err(anyhow!("Tool '{}' not found", tool_name)),
    }
}

/// A synthetic `ServerInfo` for the "mcphub-desktop" builtin server, always
/// shown in the server list. No DB row, no process - purely virtual. It
/// bundles the app's built-in capabilities as one server's capabilities:
///   - tools: the RAG tools (only while RAG is enabled; empty otherwise)
///   - prompts: all builtin prompts (prompt_service)
///   - resources: all builtin resources (resource_service)
/// The frontend renders it like any server (with management actions disabled);
/// groups select its tools/prompts/resources per-server like any server.
pub async fn builtin_server_info() -> Option<crate::models::server::ServerInfo> {
    use crate::models::server::{ServerConfig, ServerInfo, ServerStatus, ServerType};
    // Tools: RAG tools only while RAG is enabled. Always-empty when off so the
    // server still shows (with its prompts/resources).
    let tools = if is_enabled() { builtin_tools() } else { Vec::new() };
    let tool_count = tools.len();
    // Prompts/resources: the builtin library (always available, independent of RAG).
    let prompts = crate::services::prompt_service::list_all().await.unwrap_or_default();
    let resources = crate::services::resource_service::list_all().await.unwrap_or_default();
    Some(ServerInfo {
        config: ServerConfig {
            id: String::new(),
            name: BUILTIN_SERVER_NAME.to_string(),
            server_type: ServerType::Builtin,
            description: Some("Built-in capabilities (RAG, prompts, resources)".to_string()),
            command: None,
            args: None,
            env: None,
            url: None,
            headers: None,
            options: None,
            openapi: None,
            per_session_client: None,
            start_on_demand: None,
            idle_timeout_ms: None,
            proxy: None,
            enabled: true,
        },
        status: ServerStatus {
            name: BUILTIN_SERVER_NAME.to_string(),
            connected: true,
            starting: false,
            start_on_demand: false,
            tool_count,
            error: None,
            last_connected: None,
            server_version: None,
        },
        tools,
        prompts,
        resources,
    })
}

// ── documents ───────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct DocMeta {
    id: String,
    name: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    size: u64,
    uploaded_at: String,
    #[serde(default)]
    chunk_count: u32,
    /// Content version. 1 on first upload, incremented on each `update_doc`
    /// (content overwrite). Lets the UI show "vN" + track which content rev a
    /// doc's embeddings correspond to (esp. after a re-embed).
    #[serde(default = "default_version")]
    version: u32,
    /// Persisted file-type label (e.g. "Markdown", "Java"). Set by
    /// `rag_file_create`/`rag_file_update` from the caller-supplied docType so
    /// the UI's renderer picks the right viewer even when the filename has no
    /// extension. `None` for legacy docs / uploads -> falls back to
    /// `file_type_label(name)` at read time.
    #[serde(default)]
    file_type: Option<String>,
    /// Import method: "symlink" (only records original_path, no copied file) or
    /// "copy" (copies bytes into rag/files). `None` for legacy docs (pre-feature)
    /// -> treated as "copy" at read time.
    #[serde(default)]
    method: Option<String>,
    /// Absolute path of the original imported file. For "symlink" docs this is
    /// the live source read at index/view time; for "copy" docs it's recorded
    /// for update detection (md5 compare) + "open original location". `None` for
    /// legacy docs that predate this field.
    #[serde(default)]
    original_path: Option<String>,
    /// MD5 (hex) of the source file content captured at import time. Used for
    /// update detection: re-hash the source and compare. `None` for legacy docs
    /// -> treated as "has update" by `check_rag_update`.
    #[serde(default)]
    md5: Option<String>,
}

/// Default content version for a freshly-uploaded doc + back-compat fallback
/// for legacy `.meta` files written before the `version` field existed.
fn default_version() -> u32 {
    1
}

/// List distinct tags with their document counts. Reads from the
/// `rag_tags` table (kept in sync by `upsert_doc_sql`/`remove_doc_sql`/
/// `rebuild_rag_sql_index`; ordered by file_count DESC, created_at DESC,
/// tag ASC). When `search_keys` is non-empty, only returns tags that contain
/// (case-insensitive) any of the keys — used by the `rag_tag_search` MCP tool.
/// Unbounded (no pagination) — kept for the MCP tool contract.
pub async fn list_tags(search_keys: Vec<String>) -> Result<Vec<RagTagStat>> {
    let pool = crate::db::pool();
    let rows = sqlx::query(
        "SELECT tag, file_count FROM rag_tags ORDER BY file_count DESC, created_at DESC, tag",
    )
    .fetch_all(pool)
        .await?;
    let keys: Vec<String> = search_keys
        .into_iter()
        .map(|k| k.trim().to_lowercase())
        .filter(|k| !k.is_empty())
        .collect();
    let mut out = Vec::new();
    for row in rows {
        let tag: String = sqlx::Row::try_get(&row, "tag")?;
        let file_count: i64 = sqlx::Row::try_get(&row, "file_count")?;
        if !keys.is_empty() && !keys.iter().any(|k| tag.to_lowercase().contains(k)) {
            continue;
        }
        out.push(RagTagStat {
            tag,
            file_count: file_count.max(0) as u32,
        });
    }
    Ok(out)
}

/// Paginated tag search for the frontend's searchable dropdowns. Filters at
/// the SQL level (case-insensitive LIKE) + LIMIT/OFFSET so even a huge tag
/// library stays cheap. `search_key` is a single substring (empty = all);
/// `page` is 0-based; `page_size` is clamped to a sane range. Returns the
/// page's items + the total matching count (so the UI can show "load more"
/// / stop fetching).
pub async fn list_tags_paged(
    search_key: String,
    page: u32,
    page_size: u32,
) -> Result<RagTagPage> {
    let pool = crate::db::pool();
    let page = page.min(10_000);
    // Clamp page_size: at least 1, at most 200 — defends against accidental
    // huge fetches while still allowing a comfortable dropdown page.
    let page_size = page_size.clamp(1, 200);
    let key = search_key.trim().to_lowercase();
    // LIKE pattern is case-insensitive for ASCII in SQLite by default for the
    // ASCII range; we also wrap the column in LOWER() so non-ASCII comparison
    // matches the trimmed-lowercased key.
    let pattern = format!("%{}%", key.replace('%', "\\%").replace('_', "\\_"));
    let offset = (page as i64) * (page_size as i64);

    let total: i64 = if key.is_empty() {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM rag_tags")
            .fetch_one(pool)
            .await?
    } else {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM rag_tags WHERE LOWER(tag) LIKE ? ESCAPE '\\'",
        )
        .bind(&pattern)
        .fetch_one(pool)
        .await?
    };

    // Most-used tags first (file_count DESC), then newest first (created_at
    // DESC), tag ASC as the deterministic tie-breaker - matches the dropdown's
    // "关联文件数倒序 + 创建时间倒序" requirement and is served by
    // idx_rag_tags_file_count.
    let rows = if key.is_empty() {
        sqlx::query(
            "SELECT tag, file_count FROM rag_tags ORDER BY file_count DESC, created_at DESC, tag LIMIT ? OFFSET ?",
        )
        .bind(page_size as i64)
        .bind(offset)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query(
            "SELECT tag, file_count FROM rag_tags \
             WHERE LOWER(tag) LIKE ? ESCAPE '\\' ORDER BY file_count DESC, created_at DESC, tag LIMIT ? OFFSET ?",
        )
        .bind(&pattern)
        .bind(page_size as i64)
        .bind(offset)
        .fetch_all(pool)
        .await?
    };

    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let tag: String = sqlx::Row::try_get(&row, "tag")?;
        let file_count: i64 = sqlx::Row::try_get(&row, "file_count")?;
        items.push(RagTagStat {
            tag,
            file_count: file_count.max(0) as u32,
        });
    }
    Ok(RagTagPage {
        items,
        total: total.max(0) as u64,
        page,
        page_size,
    })
}

// ── SQL 镜像维护（rag_docs / rag_doc_tags / rag_tags）──────────────────────
//
// `.meta` 文件仍是唯一事实源；这三张 SQLite 表是随文档 CRUD **增量维护**的
// 查询索引（供标签下拉、文件搜索等 SQL 分页查询）。启动时
// `rebuild_rag_sql_index` 全量对账一次，兜底崩溃残留/手工改动导致的漂移。
//   - rag_doc_tags(doc_id, tag)：标签-文档关联表
//   - rag_tags(tag, file_count)：独立标签表，file_count 随关联表 CRUD 增减，
//     减到 0 的标签行直接删除（"关联文件数为 0 则该标签删除"）
//   - rag_docs：文档元数据镜像（文件搜索用）

/// 读取一个文档当前在 SQL 关联表里的标签集合（无记录 = 新文档/漂移）。
async fn sql_doc_tags(
    tx: &mut sqlx::SqliteConnection,
    doc_id: &str,
) -> Result<std::collections::HashSet<String>> {
    let rows = sqlx::query("SELECT tag FROM rag_doc_tags WHERE doc_id = ?")
        .bind(doc_id)
        .fetch_all(&mut *tx)
        .await?;
    rows.into_iter()
        .map(|r| sqlx::Row::try_get::<String, _>(&r, "tag").map_err(Into::into))
        .collect()
}

/// 把一个文档的元数据 + 标签增量同步进 SQL 镜像表（同一事务）：
/// - 标签 diff：移除的计数 -1（归零删行），新增的 UPSERT 计数 +1
/// - `rag_doc_tags`：该 doc_id 的关联行全量替换
/// - `rag_docs`：UPSERT 元数据行
///
/// 调用方负责先落盘 `.meta`（事实源）；失败向上传播，调用点用 warn 日志兜底。
async fn upsert_doc_sql(meta: &DocMeta) -> Result<()> {
    let pool = crate::db::pool();
    let mut tx = pool.begin().await?;

    let old_tags = sql_doc_tags(&mut tx, &meta.id).await?;
    // dedup（同 rebuild 的语义：同文档重复标签只计一次）
    let new_tags: std::collections::HashSet<String> = meta.tags.iter().cloned().collect();

    for tag in old_tags.difference(&new_tags) {
        sqlx::query("UPDATE rag_tags SET file_count = file_count - 1 WHERE tag = ?")
            .bind(tag)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM rag_tags WHERE tag = ? AND file_count <= 0")
            .bind(tag)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM rag_doc_tags WHERE doc_id = ? AND tag = ?")
            .bind(&meta.id)
            .bind(tag)
            .execute(&mut *tx)
            .await?;
    }
    for tag in new_tags.difference(&old_tags) {
        // created_at 记录标签首次创建时间（毫秒时间戳）：新标签打上当前时间，
        // 已有标签保留原值（ON CONFLICT 只递增计数，不覆盖 created_at）。
        let now_ms = chrono::Utc::now().timestamp_millis().to_string();
        sqlx::query(
            "INSERT INTO rag_tags (tag, file_count, created_at) VALUES (?, 1, ?) \
             ON CONFLICT(tag) DO UPDATE SET file_count = file_count + 1",
        )
        .bind(tag)
        .bind(&now_ms)
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT OR IGNORE INTO rag_doc_tags (doc_id, tag) VALUES (?, ?)")
            .bind(&meta.id)
            .bind(tag)
            .execute(&mut *tx)
            .await?;
    }

    sqlx::query(
        "INSERT INTO rag_docs (id, name, size, uploaded_at, file_type, method, original_path, md5, version, chunk_count) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
           name = excluded.name, size = excluded.size, uploaded_at = excluded.uploaded_at, \
           file_type = excluded.file_type, method = excluded.method, \
           original_path = excluded.original_path, md5 = excluded.md5, \
           version = excluded.version, chunk_count = excluded.chunk_count",
    )
    .bind(&meta.id)
    .bind(&meta.name)
    .bind(meta.size as i64)
    .bind(&meta.uploaded_at)
    .bind(&meta.file_type)
    .bind(&meta.method)
    .bind(&meta.original_path)
    .bind(&meta.md5)
    .bind(meta.version as i64)
    .bind(meta.chunk_count as i64)
    .execute(&mut *tx)
    .await?;

    // FTS 同步（§4.3 铁律：与 rag_docs 镜像同事务；ref_id=id，文本=name）
    crate::services::fts_service::sync_upsert_tx(
        &mut tx,
        crate::services::fts_service::FtsTable::RagDocs,
        &meta.id,
        &meta.name,
    )
    .await?;

    tx.commit().await?;
    Ok(())
}

/// 从 SQL 镜像表移除一个文档：其所有标签计数 -1（归零删行），删关联行 + rag_docs 行。
async fn remove_doc_sql(doc_id: &str) -> Result<()> {
    let pool = crate::db::pool();
    let mut tx = pool.begin().await?;

    let old_tags = sql_doc_tags(&mut tx, doc_id).await?;
    for tag in &old_tags {
        sqlx::query("UPDATE rag_tags SET file_count = file_count - 1 WHERE tag = ?")
            .bind(tag)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM rag_tags WHERE tag = ? AND file_count <= 0")
            .bind(tag)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("DELETE FROM rag_doc_tags WHERE doc_id = ?")
        .bind(doc_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM rag_docs WHERE id = ?")
        .bind(doc_id)
        .execute(&mut *tx)
        .await?;
    // FTS 同步（§4.3 铁律：同事务删 fts_rag_docs）
    crate::services::fts_service::sync_delete_tx(
        &mut tx,
        crate::services::fts_service::FtsTable::RagDocs,
        doc_id,
    )
    .await?;

    tx.commit().await?;
    Ok(())
}

/// 全量对账：扫描所有 `.meta` 文件，整表重建 `rag_docs` / `rag_doc_tags` /
/// `rag_tags`（单一事务 DELETE + 批量 INSERT）。用于 RAG 启动时兜底漂移，以及
/// reindex_all / batch_update 结束后的安全网。计数为 0 的标签自然不落表。
pub async fn rebuild_rag_sql_index(app: &AppHandle) -> Result<()> {
    let dir = files_dir(app)?;
    let mut docs: Vec<DocMeta> = Vec::new();
    if dir.exists() {
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("meta") {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else { continue };
            let Ok(meta) = serde_json::from_slice::<DocMeta>(&bytes) else { continue };
            docs.push(meta);
        }
    }

    // 标签计数 + 关联行（同文档重复标签去重）
    let mut counts: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
    let mut assoc: Vec<(String, String)> = Vec::new();
    for meta in &docs {
        let mut seen = std::collections::HashSet::new();
        for tag in &meta.tags {
            if !tag.is_empty() && seen.insert(tag.clone()) {
                *counts.entry(tag.clone()).or_insert(0) += 1;
                assoc.push((meta.id.clone(), tag.clone()));
            }
        }
    }

    let pool = crate::db::pool();
    let mut tx = pool.begin().await?;
    // 保留现有标签的 created_at（对账重建不应重置排序依据）；新标签用当前
    // 时间，与增量 upsert_doc_sql 的语义一致。
    let old_created: std::collections::HashMap<String, String> = sqlx::query(
        "SELECT tag, created_at FROM rag_tags",
    )
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .filter_map(|r| {
        let tag: String = sqlx::Row::try_get(&r, "tag").ok()?;
        let created: String = sqlx::Row::try_get(&r, "created_at").unwrap_or_default();
        Some((tag, created))
    })
    .collect();
    let now_ms = chrono::Utc::now().timestamp_millis().to_string();

    sqlx::query("DELETE FROM rag_docs").execute(&mut *tx).await?;
    sqlx::query("DELETE FROM rag_doc_tags").execute(&mut *tx).await?;
    sqlx::query("DELETE FROM rag_tags").execute(&mut *tx).await?;
    for meta in &docs {
        sqlx::query(
            "INSERT INTO rag_docs (id, name, size, uploaded_at, file_type, method, original_path, md5, version, chunk_count) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&meta.id)
        .bind(&meta.name)
        .bind(meta.size as i64)
        .bind(&meta.uploaded_at)
        .bind(&meta.file_type)
        .bind(&meta.method)
        .bind(&meta.original_path)
        .bind(&meta.md5)
        .bind(meta.version as i64)
        .bind(meta.chunk_count as i64)
        .execute(&mut *tx)
        .await?;
    }
    for (doc_id, tag) in &assoc {
        sqlx::query("INSERT OR IGNORE INTO rag_doc_tags (doc_id, tag) VALUES (?, ?)")
            .bind(doc_id)
            .bind(tag)
            .execute(&mut *tx)
            .await?;
    }
    for (tag, count) in &counts {
        let created = old_created.get(tag).cloned().unwrap_or_else(|| now_ms.clone());
        sqlx::query("INSERT INTO rag_tags (tag, file_count, created_at) VALUES (?, ?, ?)")
            .bind(tag)
            .bind(*count as i64)
            .bind(&created)
            .execute(&mut *tx)
            .await?;
    }

    // FTS 同步（§4.3 铁律：对账重建含 fts_rag_docs，同一事务）
    crate::services::fts_service::clear_table_tx(&mut tx, crate::services::fts_service::FtsTable::RagDocs).await?;
    for meta in &docs {
        crate::services::fts_service::sync_upsert_tx(
            &mut tx,
            crate::services::fts_service::FtsTable::RagDocs,
            &meta.id,
            &meta.name,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// List all uploaded documents (metadata only — no content). Works with RAG
/// OFF (reads the filesystem, not the vector DB).
pub async fn list_docs(app: &AppHandle) -> Result<Vec<RagDocInfo>> {
    let dir = files_dir(app)?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    // Clone the dir into the read_dir closure so `dir` stays borrowable below
    // (used to resolve the on-disk file name per doc).
    let dir_clone = dir.clone();
    let mut entries = tokio::task::spawn_blocking(move || std::fs::read_dir(&dir_clone)).await??;
    while let Some(entry) = entries.next() {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("meta") {
            continue;
        }
        // Skip unreadable/corrupt metas instead of failing the whole listing -
        // consistent with every other scanner (rebuild_rag_sql_index /
        // run_batch_update / preview_batch_update / reindex_all /
        // find_doc_ids_by_name). A single bad meta (partial write, crash,
        // manual edit) must not blank the entire doc list.
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let Ok(meta): Result<DocMeta, _> = serde_json::from_slice(&bytes) else { continue };
        out.push(doc_info_from_meta(&dir, meta));
    }
    out.sort_by(|a, b| b.uploaded_at.cmp(&a.uploaded_at));
    Ok(out)
}

/// Build the list-view `RagDocInfo` for one parsed `.meta`: resolves the
/// on-disk file name (uuid for uploads, meta.name for rag_file_create) and the
/// filesystem-derived flags (lost_original / content_available). Shared by
/// `list_docs` (full scan) and `search_docs_paged` (page enrichment only).
fn doc_info_from_meta(dir: &Path, meta: DocMeta) -> RagDocInfo {
    // The actual on-disk file name - content_path_for resolves which exists.
    // Surfaced so the user can match the file when its folder is opened.
    // Empty for "symlink" docs (no copied file - content lives at original_path).
    let is_symlink = meta.method.as_deref() == Some("symlink");
    let file_name = if is_symlink {
        String::new()
    } else {
        content_path_for(dir, &meta.id, &meta.name)
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .unwrap_or_default()
    };
    // lost_original: the recorded original_path is missing on disk (both
    // symlink AND copy docs count - the badge + auto-update skip apply to
    // both). content_available: can the content be read RIGHT NOW? symlink
    // = original exists; copy = the copy file exists in rag/files. A copy
    // doc whose original vanished still has its copy -> content_available
    // true (view/open stay enabled), only auto-update is off.
    let original_exists = meta
        .original_path
        .as_deref()
        .map(|p| !p.is_empty() && Path::new(p).exists())
        .unwrap_or(false);
    let has_original_path = meta
        .original_path
        .as_deref()
        .map(|p| !p.is_empty())
        .unwrap_or(false);
    let lost_original = has_original_path && !original_exists;
    let content_available = if is_symlink {
        original_exists
    } else {
        // copy: the imported copy lives in rag/files. Check the resolved
        // content path (handles {id}.{ext} / {id} / {meta_name}).
        content_path_for(dir, &meta.id, &meta.name).exists()
    };
    // Compute the display file_type label BEFORE the struct moves `name`.
    let file_type = meta.file_type.clone().unwrap_or_else(|| file_type_label(&meta.name));
    RagDocInfo {
        id: meta.id,
        name: meta.name,
        size: meta.size,
        uploaded_at: meta.uploaded_at,
        tags: meta.tags,
        chunk_count: meta.chunk_count,
        file_type,
        version: meta.version.max(1),
        file_name,
        method: meta.method.clone().unwrap_or_default(),
        original_path: meta.original_path.clone().unwrap_or_default(),
        md5: meta.md5.clone().unwrap_or_default(),
        lost_original,
        content_available,
    }
}

/// Paginated doc search over the `rag_docs` SQL mirror: `search_key` is a
/// case-insensitive substring on the doc name (empty = all), `tags` is an
/// ANY-match filter via the `rag_doc_tags` association table. SQL does the
/// filtering / ordering (uploaded_at DESC) / LIMIT+OFFSET; the returned page's
/// doc ids are then enriched from their `.meta` files (file_name /
/// lost_original / content_available are filesystem-derived). `page` is
/// 0-based. Works with RAG off (the mirror is rebuilt on the last RAG enable).
pub async fn search_docs_paged(
    app: &AppHandle,
    search_key: String,
    tags: Vec<String>,
    page: u32,
    page_size: u32,
) -> Result<RagDocPage> {
    let dir = files_dir(app)?;
    let page = page.min(10_000);
    let page_size = page_size.clamp(1, 200);
    let key = search_key.trim().to_lowercase();
    // Escape LIKE wildcards in the user input (same rules as list_tags_paged).
    let pattern = format!("%{}%", key.replace('%', "\\%").replace('_', "\\_"));
    let offset = (page as i64) * (page_size as i64);
    let want_tags: Vec<String> = tags
        .into_iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();

    let mut conds: Vec<String> = Vec::new();
    if !key.is_empty() {
        conds.push("LOWER(name) LIKE ? ESCAPE '\\'".to_string());
    }
    if !want_tags.is_empty() {
        let placeholders = vec!["?"; want_tags.len()].join(", ");
        conds.push(format!(
            "id IN (SELECT doc_id FROM rag_doc_tags WHERE tag IN ({}))",
            placeholders
        ));
    }
    let where_clause = if conds.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conds.join(" AND "))
    };

    // FTS5 路径（中/英/拼音分词；§4.4）：key 非空时优先走 FTS，tag 过滤仍走
    // SQL 镜像（rag_doc_tags），结果按 FTS rank 序排列；空表/Err 降级原 LIKE。
    if !key.is_empty() {
        match crate::services::fts_service::search_ref_ids_weighted(
            crate::services::fts_service::FtsTable::RagDocs,
            &key,
            5000,
        )
        .await
        {
            Ok(weighted) if !weighted.is_empty() => {
                // 命中词数 map：相关度第一优先（命中越多越靠前），
                // 同命中数保持原生 uploaded_at DESC, id 序（稳定排序）
                let counts: std::collections::HashMap<String, i64> =
                    weighted.into_iter().collect();
                // tag 条件 → doc_id 白名单（SQL 镜像查询）
                let allowed: Option<std::collections::HashSet<String>> =
                    if want_tags.is_empty() {
                        None
                    } else {
                        let mut qb = sqlx::QueryBuilder::new(
                            "SELECT DISTINCT doc_id FROM rag_doc_tags WHERE tag IN (",
                        );
                        let mut sep = qb.separated(", ");
                        for t in &want_tags {
                            sep.push_bind(t);
                        }
                        qb.push(")");
                        let rows = qb.build().fetch_all(crate::db::pool()).await?;
                        Some(
                            rows.iter()
                                .filter_map(|r| sqlx::Row::try_get(r, "doc_id").ok())
                                .collect(),
                        )
                    };
                // 原生序（uploaded_at DESC, id）过滤命中集与 tag 白名单，
                // 再稳定排序按命中词数降序——同相关度保持原生序
                let nat_rows = sqlx::query("SELECT id FROM rag_docs ORDER BY uploaded_at DESC, id")
                    .fetch_all(crate::db::pool())
                    .await?;
                let mut ordered: Vec<(String, i64)> = Vec::new();
                for r in &nat_rows {
                    let id: String = sqlx::Row::try_get(r, "id")?;
                    if let Some(c) = counts.get(&id) {
                        ordered.push((id, *c));
                    }
                }
                ordered.retain(|(id, _)| allowed.as_ref().map_or(true, |s| s.contains(id)));
                // 相关度第一优先（稳定排序：同命中数保持 uploaded_at DESC, id 序）
                ordered.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
                let total = ordered.len() as u64;
                let window: Vec<String> = ordered
                    .iter()
                    .skip(offset.max(0) as usize)
                    .take(page_size as usize)
                    .map(|(id, _)| id.clone())
                    .collect();
                let mut items = Vec::with_capacity(window.len());
                for id in window {
                    let meta_path = dir.join(format!("{}.meta", id));
                    let Ok(bytes) = std::fs::read(&meta_path) else { continue };
                    let Ok(meta): Result<DocMeta, _> = serde_json::from_slice(&bytes) else { continue };
                    items.push(doc_info_from_meta(&dir, meta));
                }
                return Ok(RagDocPage { items, total, page, page_size });
            }
            Ok(_)
                if crate::services::fts_service::table_is_empty(
                    crate::services::fts_service::FtsTable::RagDocs,
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
                log::warn!("[fts] search rag_docs failed, fallback to LIKE: {e}");
            }
        }
    }

    let pool = crate::db::pool();
    // Count query (total across all pages, not just this one).
    let count_sql = format!("SELECT COUNT(*) FROM rag_docs {}", where_clause);
    let mut count_q = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(&*count_sql));
    if !key.is_empty() {
        count_q = count_q.bind(&pattern);
    }
    for t in &want_tags {
        count_q = count_q.bind(t);
    }
    let total: i64 = count_q.fetch_one(pool).await?;

    // Page query: id list ordered like list_docs (uploaded_at DESC, id as the
    // deterministic tie-breaker).
    let data_sql = format!(
        "SELECT id FROM rag_docs {} ORDER BY uploaded_at DESC, id LIMIT ? OFFSET ?",
        where_clause
    );
    let mut data_q = sqlx::query(sqlx::AssertSqlSafe(&*data_sql));
    if !key.is_empty() {
        data_q = data_q.bind(&pattern);
    }
    for t in &want_tags {
        data_q = data_q.bind(t);
    }
    let rows = data_q
        .bind(page_size as i64)
        .bind(offset)
        .fetch_all(pool)
        .await?;

    // Enrich just this page from the .meta files (source of truth). Skip
    // unreadable/corrupt metas - same policy as list_docs.
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let id: String = sqlx::Row::try_get(&row, "id")?;
        let meta_path = dir.join(format!("{}.meta", id));
        let Ok(bytes) = std::fs::read(&meta_path) else { continue };
        let Ok(meta): Result<DocMeta, _> = serde_json::from_slice(&bytes) else { continue };
        items.push(doc_info_from_meta(&dir, meta));
    }
    Ok(RagDocPage {
        items,
        total: total.max(0) as u64,
        page,
        page_size,
    })
}

/// Get the full content of a document (for the View dialog).
pub async fn get_doc(app: &AppHandle, id: &str) -> Result<Option<RagDoc>> {
    // Unpaged read: the whole content in one go (rag_get MCP tool + internal
    // callers). The UI uses `get_doc_paged` instead.
    match get_doc_inner(app, id, None).await? {
        Some((doc, _full)) => Ok(Some(doc)),
        None => Ok(None),
    }
}

/// Read a document's content as a page: `offset` UTF-8 bytes are skipped, then
/// up to `limit_bytes` are returned (the limit is char-boundary-aligned so the
/// slice is always valid UTF-8). Returns the RagDoc with `truncated` /
/// `next_offset` / `content_total_bytes` set, plus the FULL decoded content
/// (the caller — the command layer — drops it; kept internal so unpaged and
/// paged reads share one code path).
pub async fn get_doc_paged(
    app: &AppHandle,
    id: &str,
    offset_bytes: u64,
    limit_bytes: u64,
) -> Result<Option<RagDoc>> {
    let limit = if limit_bytes == 0 {
        // 0 → fall back to the user's configured page size.
        (get_settings().await.unwrap_or_default().doc_load_chunk_kb as u64) * 1024
    } else {
        limit_bytes
    };
    match get_doc_inner(app, id, Some((offset_bytes, limit))).await? {
        Some((doc, _full)) => Ok(Some(doc)),
        None => Ok(None),
    }
}

/// Shared core of `get_doc` (whole content) and `get_doc_paged` (a window).
/// `range = Some((offset, limit))` slices the decoded UTF-8 content by byte
/// window, backing off to the nearest char boundary so the prefix/suffix is
/// never a broken multi-byte sequence. The second tuple element is the full
/// content length in bytes (already reflected in `content_total_bytes`; the
/// tuple keeps callers from re-reading).
async fn get_doc_inner(
    app: &AppHandle,
    id: &str,
    range: Option<(u64, u64)>,
) -> Result<Option<(RagDoc, u64)>> {
    let dir = files_dir(app)?;
    let meta_path = dir.join(format!("{}.meta", id));
    let Ok(meta_bytes) = std::fs::read(&meta_path) else {
        return Ok(None);
    };
    let meta: DocMeta = serde_json::from_slice(&meta_bytes)?;
    let is_symlink = meta.method.as_deref() == Some("symlink");
    // lost_original: original_path missing on disk, regardless of method (so
    // copy docs whose source vanished also show as lost in the UI + are skipped
    // by batch update). See `classify_original` for the shared definition.
    let original_exists = meta
        .original_path
        .as_deref()
        .map(|p| !p.is_empty() && Path::new(p).exists())
        .unwrap_or(false);
    let has_original_path = meta
        .original_path
        .as_deref()
        .map(|p| !p.is_empty())
        .unwrap_or(false);
    let lost_original = has_original_path && !original_exists;
    // Content: for symlink docs read from original_path (if it exists); for
    // copy docs read from the rag/files copy via content_path_for. Symlink +
    // lost -> empty content (the UI greys "view" via content_available).
    let (content, content_available) = if is_symlink {
        if let Some(op) = meta.original_path.as_deref() {
            if !op.is_empty() && Path::new(op).exists() {
                (std::fs::read_to_string(op).unwrap_or_default(), true)
            } else {
                (String::new(), false)
            }
        } else {
            (String::new(), false)
        }
    } else {
        let content_path = content_path_for(&dir, id, &meta.name);
        let avail = content_path.exists();
        (std::fs::read_to_string(&content_path).unwrap_or_default(), avail)
    };
    let total_bytes = content.len() as u64;
    // Apply the byte window (paged read). `is_char_boundary` guarantees the
    // slices stay valid UTF-8 even when offset/limit land mid multi-byte char.
    let (paged, truncated, next_offset) = match range {
        Some((offset, limit)) => {
            let offset = (offset.min(total_bytes)) as usize;
            // Back off to the char boundary at-or-before the offset.
            let mut start = offset;
            while start > 0 && !content.is_char_boundary(start) {
                start -= 1;
            }
            let limit = (limit as usize).min((total_bytes as usize).saturating_sub(start));
            // Extend to the char boundary at-or-after the limit end.
            let mut end = start + limit;
            while end < content.len() && !content.is_char_boundary(end) {
                end += 1;
            }
            let truncated = end < content.len();
            (content[start..end].to_string(), truncated, end as u64)
        }
        None => (content.clone(), false, total_bytes),
    };
    Ok(Some((
        RagDoc {
            id: meta.id,
            name: meta.name.clone(),
            size: meta.size,
            content: paged,
            uploaded_at: meta.uploaded_at,
            tags: meta.tags,
            chunk_count: meta.chunk_count,
            file_type: meta.file_type.clone().unwrap_or_else(|| file_type_label(&meta.name)),
            method: meta.method.clone().unwrap_or_default(),
            original_path: meta.original_path.clone().unwrap_or_default(),
            lost_original,
            content_available,
            truncated,
            next_offset,
            content_total_bytes: total_bytes,
        },
        total_bytes,
    )))
}

/// Read a document's chunks (index + text, no embeddings) for the "view chunks"
/// dialog. Returns chunks ordered by `chunk_index`. Requires RAG enabled
/// (chunks live in lancedb). Returns an empty vec if the doc has no chunks
/// (not yet indexed / model swapped + not re-indexed).
pub async fn get_doc_chunks(id: &str) -> Result<Vec<crate::models::rag::RagChunk>> {
    // Unpaged: all chunks (kept for any internal caller).
    let (chunks, _total) = get_doc_chunks_inner(id, 0, u32::MAX).await?;
    Ok(chunks)
}

/// Paginated chunks for the "view chunks" dialog: `offset` chunks are skipped,
/// then up to `page_size` returned (clamped to [1, 200]), plus the total chunk
/// count so the UI can stop loading when all pages are in. Ordering matches
/// `get_doc_chunks` (chunk_index ASC).
pub async fn get_doc_chunks_paged(
    id: &str,
    offset: u32,
    page_size: u32,
) -> Result<crate::models::rag::RagChunkPage> {
    let page_size = page_size.clamp(1, 200);
    let (items, total) = get_doc_chunks_inner(id, offset, page_size).await?;
    Ok(crate::models::rag::RagChunkPage {
        items,
        total,
        offset,
        page_size,
    })
}

/// Shared core: read the doc's chunk records, sort by index, slice to the
/// [offset, offset+limit) window, and return (page items, total count).
/// Reading all records then slicing in memory is intentional: the records are
/// already in lancedb's scan order and the per-doc chunk counts are small
/// (hundreds at most — chunked text, one row per chunk), while a SQL-style
/// LIMIT pushdown isn't available on the vector store query API.
async fn get_doc_chunks_inner(
    id: &str,
    offset: u32,
    limit: u32,
) -> Result<(Vec<crate::models::rag::RagChunk>, u64)> {
    let guard = runtime().lock().await;
    let Some(rt) = guard.as_ref() else {
        return Err(anyhow!("RAG is not enabled - turn on RAG before viewing chunks"));
    };
    let mut records = rt.db.read_chunks_by_doc(id).await?;
    records.sort_by_key(|r| r.chunk_index);
    let total = records.len() as u64;
    let items = records
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .map(|r| crate::models::rag::RagChunk {
            chunk_index: r.chunk_index,
            chunk_text: r.chunk_text,
        })
        .collect();
    Ok((items, total))
}

/// Open the OS multi-file picker. No extension filter — validation is
/// content-based (extract strategies for PDF/Office/images, `is_likely_text`
/// sniff otherwise), so the user may pick any file and get a clear rejection
/// or a parse failure if it can't be processed. Returns the picked paths +
/// display names; no bytes cross IPC (backend reads disk).
pub fn pick_files(app: &AppHandle) -> Vec<RagPickedFile> {
    use tauri_plugin_dialog::DialogExt;
    let b = app.dialog().file().set_title("Select documents");
    let Some(paths) = b.blocking_pick_files() else { return Vec::new() };
    paths
        .into_iter()
        .filter_map(|fp| {
            let p = fp.into_path().ok()?;
            let name = p.file_name()?.to_string_lossy().to_string();
            Some(RagPickedFile {
                path: p.to_string_lossy().to_string(),
                name,
            })
        })
        .collect()
}

/// Open the OS folder picker, then scan the folder's children for import
/// candidates. Flat mode (`recursive=false`, the pre-existing behavior) scans
/// only immediate file children. Recursive mode (`recursive=true`) walks all
/// descendant directories, skipping:
///   - directories named in `folder_ignore.json` (dev/build dirs like
///     node_modules, dist, target — see `folder_ignore_set()`);
///   - hidden entries (leading dot, any platform) — macOS `.DS_Store` and
///     `.git`-style dirs never become candidates;
///   - symlinked directories (loop safety).
/// Each file candidate must pass BOTH filters (same as flat mode):
///   1. extension catalog — its lowercased dot-prefixed extension (".md",
///      ".py"…) must be present in `file_type_map()` (built from
///      `runtimes/rag/file_support.json`), so unknown/unsupported file kinds
///      are dropped by name before reading bytes;
///   2. content sniff — the first 8 KiB must look like text via
///      `is_likely_text` (same rule as upload validation), so a misnamed
///      binary file still gets filtered out.
/// This way the dialog only lists files the upload pipeline would actually
/// accept. Recursive scans stop at `SCAN_FOLDER_FILE_CAP` candidates and set
/// `truncated` (+ `skipped_files`) so the UI can warn. Returns an empty
/// groups list if the user cancels or no supported file remains.
pub fn pick_folder(app: &AppHandle, recursive: bool) -> RagFolderScan {
    use tauri_plugin_dialog::DialogExt;
    let b = app.dialog().file().set_title("Select a folder to import");
    let Some(fp) = b.blocking_pick_folder() else {
        return RagFolderScan::empty();
    };
    let Ok(folder) = fp.into_path() else {
        return RagFolderScan::empty();
    };
    scan_folder(&folder, recursive)
}

/// Hard cap on candidate files per recursive folder scan. Guards against
/// accidentally importing a home directory (which can hold hundreds of
/// thousands of files under node_modules etc.) — the scan stops early and the
/// UI tells the user to pick a smaller folder.
const SCAN_FOLDER_FILE_CAP: usize = 500;

/// Lazily-compiled ignore-list from `runtimes/rag/folder_ignore.json`
/// (directory names, exact match, any depth). Same include_str! pattern as
/// `file_type_map()`.
static FOLDER_IGNORE_SET: OnceLock<std::collections::HashSet<String>> = OnceLock::new();

fn folder_ignore_set() -> &'static std::collections::HashSet<String> {
    FOLDER_IGNORE_SET.get_or_init(|| {
        let raw = include_str!("../../runtimes/rag/folder_ignore.json");
        let mut set = std::collections::HashSet::new();
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
            if let Some(arr) = v.get("folders").and_then(|f| f.as_array()) {
                for name in arr.iter().filter_map(|x| x.as_str()) {
                    set.insert(name.to_lowercase());
                }
            }
        }
        set
    })
}

/// Scan `folder` for import candidates, grouped per sub-directory (recursive
/// mode) or as one flat group (the root, flat mode). Shared filtering rules
/// with `pick_files`' validation pipeline; see `pick_folder` for the list.
/// Group/file order is stable (path-sorted), so rescans look identical.
fn scan_folder(folder: &std::path::Path, recursive: bool) -> RagFolderScan {
    let ignore = folder_ignore_set();
    // rel-path -> group index in `out.groups`.
    let mut group_idx: std::collections::HashMap<std::path::PathBuf, usize> =
        std::collections::HashMap::new();
    let mut out = RagFolderScan {
        root: folder.to_string_lossy().to_string(),
        skipped_dirs: 0,
        skipped_files: 0,
        groups: Vec::new(),
        truncated: false,
    };
    let mut total = 0usize;

    // Depth-first walk of the directory tree. Directory read errors are
    // silently skipped — a folder we can't read contributes nothing to the
    // import list. The walk stops as soon as the candidate cap is reached;
    // remaining files are counted into `skipped_files` afterwards.
    fn walk(
        dir: &Path,
        rel: &Path,
        recursive: bool,
        ignore: &std::collections::HashSet<String>,
        out: &mut RagFolderScan,
        group_idx: &mut std::collections::HashMap<PathBuf, usize>,
        total: &mut usize,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            if *total >= SCAN_FOLDER_FILE_CAP {
                out.truncated = true;
                return;
            }
            let path = entry.path();
            // Skip symlinks entirely (files AND dirs) — an import candidate
            // must be a real file we can re-read later; a symlinked dir could
            // also loop the walk.
            let Ok(lm) = std::fs::symlink_metadata(&path) else { continue };
            if lm.is_symlink() {
                continue;
            }
            let Ok(md) = std::fs::metadata(&path) else { continue };
            if md.is_dir() {
                if !recursive {
                    continue;
                }
                let Some(name) = path.file_name() else { continue };
                let name = name.to_string_lossy().to_string();
                // Hidden dirs (any platform) + folder_ignore.json names are
                // pruned from the walk — nothing beneath them is scanned.
                if name.starts_with('.') || ignore.contains(&name.to_lowercase()) {
                    out.skipped_dirs += 1;
                    continue;
                }
                let child_rel = rel.join(&name);
                walk(&path, &child_rel, recursive, ignore, out, group_idx, total);
            } else if md.is_file() {
                let Some(name) = path.file_name() else { continue };
                let name = name.to_string_lossy().to_string();
                // Skip dotfiles (hidden on Unix; conventionally hidden on
                // Windows too) so macOS `.DS_Store` and similar noise never
                // become import candidates.
                if name.starts_with('.') {
                    continue;
                }
                // Extension-catalog filter: only files whose lowercased
                // dot-prefixed extension is listed in file_support.json are
                // import candidates — dropped by name before reading bytes.
                let lower = name.to_lowercase();
                let ext_ok = lower
                    .rfind('.')
                    .map(|dot| file_type_map().contains_key(&lower[dot..]))
                    .unwrap_or(false);
                if !ext_ok {
                    out.skipped_files += 1;
                    continue;
                }
                // Content sniff (first 8 KiB): a misnamed binary file (e.g. a
                // .txt that's actually a PDF) is still dropped here, so the
                // dialog only lists what the upload pipeline would accept.
                // Extractable formats (PDF/Office/image) skip the sniff —
                // their bytes are inherently binary (PDF headers carry NULs)
                // yet the extract pipeline parses them to Markdown.
                if !crate::rag::extract::can_extract(&name) {
                    let mut head = vec![0u8; 8192];
                    let n = std::fs::File::open(&path)
                        .and_then(|mut f| {
                            use std::io::Read;
                            f.read(&mut head)
                        })
                        .unwrap_or(0);
                    head.truncate(n);
                    if !is_likely_text(&head) {
                        out.skipped_files += 1;
                        continue;
                    }
                }
                // First file seen in this folder -> create its group.
                let idx = *group_idx.entry(rel.to_path_buf()).or_insert_with(|| {
                    out.groups.push(RagScanGroup {
                        rel_path: rel.to_string_lossy().replace('\\', "/"),
                        files: Vec::new(),
                    });
                    out.groups.len() - 1
                });
                out.groups[idx].files.push(RagScanFile {
                    path: path.to_string_lossy().to_string(),
                    name,
                    size: md.len(),
                });
                *total += 1;
            }
        }
    }
    walk(
        folder,
        Path::new(""),
        recursive,
        ignore,
        &mut out,
        &mut group_idx,
        &mut total,
    );

    // Stable, readable order: groups by path (root "" first), files by name.
    out.groups.sort_by(|a, b| {
        let a_root = a.rel_path.is_empty();
        let b_root = b.rel_path.is_empty();
        b_root.cmp(&a_root).then_with(|| a.rel_path.cmp(&b.rel_path))
    });
    for g in &mut out.groups {
        g.files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    }
    out
}

impl RagFolderScan {
    fn empty() -> Self {
        Self {
            root: String::new(),
            skipped_dirs: 0,
            skipped_files: 0,
            groups: Vec::new(),
            truncated: false,
        }
    }
}

/// Upload (read + decode + embed + index) a single file given by disk path.
/// Called per-file by the frontend so it can show per-file progress; the
/// frontend loops over the picked paths.
/// Core write+index pipeline shared by `upload_one_path_inner` (from disk,
/// with encoding detection) and `create_doc_from_content` (from the MCP tool,
/// already UTF-8). Writes `dir/{file_stem}` (content) + `dir/{file_stem}.meta`,
/// then indexes into lancedb via `reindex_doc`. Caller handles same-name
/// overwrite cleanup (`find_doc_ids_by_name` + delete) and tag-stat recompute.
///
/// `file_stem` is the on-disk filename without extension: uploads pass the
/// uuid, `rag_file_create` passes `{name}.{docType}` so the file is
/// human-readable. `display_name` is `meta.name` (what the UI shows). Returns
/// chunk_count.
///
/// Import-method: `method` ("symlink"|"copy"|None), `original_path` (the
/// imported source's absolute path, recorded for update detection + open-
/// location), `md5` (hex of the source bytes, for change detection). For
/// "symlink" docs `write_content` should be false (don't copy bytes into
/// rag/files — the source is read live from original_path); for "copy" +
/// legacy (None) it's true.
async fn write_doc_and_index(
    app: &AppHandle,
    dir: &Path,
    doc_id: &str,
    file_stem: &str,
    display_name: &str,
    content: &str,
    tags: Vec<String>,
    size: u64,
    file_type: Option<String>,
    version: u32,
    method: Option<String>,
    original_path: Option<String>,
    md5: Option<String>,
) -> Result<u32> {
    let meta_path = dir.join(format!("{}.meta", doc_id));
    let uploaded_at = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    // "symlink": don't write a content copy — the source is read live. "copy"
    // /legacy (None): write the copy as before.
    let write_content = method.as_deref() != Some("symlink");
    if write_content {
        let content_path = dir.join(file_stem);
        std::fs::write(&content_path, content)?;
    }
    let chunk_count = reindex_doc(app, doc_id, display_name, content, tags.clone()).await? as u32;
    let title = extract_title(content, display_name);
    let meta = DocMeta {
        id: doc_id.to_string(),
        name: display_name.to_string(),
        title: Some(title),
        tags,
        size,
        uploaded_at,
        chunk_count,
        version,
        file_type,
        method,
        original_path,
        md5,
    };
    write_meta_atomic(&meta_path, &meta)?;
    // 增量同步 SQL 镜像（rag_docs / rag_doc_tags / rag_tags）。镜像失败不阻断
    // 导入主流程（.meta 已落盘，事实源完整；启动对账会修复漂移），但记日志。
    if let Err(e) = upsert_doc_sql(&meta).await {
        rag_log("warn", format!("upsert_doc_sql for '{}' failed: {}", meta.name, e));
    }
    Ok(chunk_count)
}

pub async fn upload_one_path(
    app: &AppHandle,
    file_path: &str,
    tags: Vec<String>,
    method: Option<String>,
) -> Result<()> {
    // Derive the display name first so we can attribute any failure to it in
    // the log (the body below returns early on many `?`, and without this
    // wrapper those failures would never reach rag_log - the user would see
    // an error toast with no matching log entry).
    let name = Path::new(file_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| file_path.to_string());

    let result = upload_one_path_inner(app, file_path, tags, method).await;
    if let Err(ref e) = result {
        rag_log("error", format!("upload failed for '{}': {:#}", name, e));
    }
    result
}

/// Update an existing document in place: replace its on-disk content, meta
/// (name/size/uploaded_at/title/file_type), and vector chunks by picking a new
/// file. The doc_id is preserved (links/refs to the id stay valid). Tags are
/// preserved from the existing meta (the update replaces content, not the
/// user's tag organization). Requires RAG enabled (reindex_doc needs the
/// runtime to embed the new content).
///
/// Holds META_LOCK across the read-modify-write (lock order: meta -> runtime).
pub async fn update_doc_from_file(app: &AppHandle, id: &str, file_path: &str) -> Result<u32> {
    let _meta_guard = meta_lock().await.lock().await;
    let dir = files_dir(app)?;
    let path = Path::new(file_path);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| file_path.to_string());

    // Read + validate the new file (same rules as upload — extractable
    // formats go through the extract strategies, text via sniff+decode).
    let raw = std::fs::read(path).map_err(|e| anyhow!("read failed for {}: {}", name, e))?;
    if raw.len() > MAX_UPLOAD_BYTES {
        return Err(anyhow!(
            "file too large: {} ({} bytes, max {} bytes)",
            name,
            raw.len(),
            MAX_UPLOAD_BYTES
        ));
    }
    let (content, _encoding) = content_from_source(&name, &raw).await?;
    let size = raw.len() as u64;

    // Read the existing meta (for tags + to clean up the OLD on-disk file,
    // whose name may use a different extension than the new file).
    let meta_path = dir.join(format!("{}.meta", id));
    let old_meta_bytes = std::fs::read(&meta_path)
        .map_err(|e| anyhow!("update: read meta {} failed: {}", id, e))?;
    let old_meta: DocMeta = serde_json::from_slice(&old_meta_bytes)?;
    let tags = old_meta.tags.clone();

    // Remove the OLD on-disk content file (resolved by the old meta's name)
    // so a different extension doesn't leave an orphan. The new file is
    // written below by write_doc_and_index. (For symlink docs there's no copy
    // to remove, but content_path_for falls back to meta_name which won't exist
    // — the .exists() guard handles it.)
    let old_content = content_path_for(&dir, id, &old_meta.name);
    if old_content.exists() {
        if let Err(e) = std::fs::remove_file(&old_content) {
            rag_log("warn", format!("update: remove old {} failed: {}", old_content.display(), e));
        }
    }

    // New on-disk filename: `{id}{ext}` with the NEW file's extension for
    // text sources; `{id}.md` for extractable sources (content is derived
    // Markdown — see upload_one_path_inner).
    let extractable = crate::rag::extract::can_extract(&name);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e.to_ascii_lowercase()))
        .unwrap_or_default();
    let file_stem = if extractable {
        format!("{id}.md")
    } else {
        format!("{id}{ext}")
    };

    rag_log("info", format!("updating '{}' ({} bytes) -> id {}", name, raw.len(), id));
    // Bump the content version (1 on first upload; +1 each update). Legacy
    // metas without the field default to 1 (see `default_version`).
    let version = old_meta.version.saturating_add(1);
    // Manual-upload update path records the NEW file as the original source
    // (this is the legacy-compat path: old docs without original_path get one
    // now, so future update detection works). MD5 of the new source is stored
    // as the baseline. Method: extractable sources are always "copy" (the
    // content file is derived Markdown); otherwise keep the doc's existing
    // method — a legacy doc (method None) becomes "copy" since manual upload
    // now has a copy + a recorded source.
    let new_method = if extractable {
        Some("copy".to_string())
    } else {
        old_meta.method.clone().or_else(|| Some("copy".to_string()))
    };
    let new_original_path = Some(file_path.to_string());
    let new_md5 = Some(compute_md5(&raw));
    // write_doc_and_index writes the new disk file, reindexes (reindex_doc
    // deletes the old vectors by id then adds the new ones), and overwrites
    // the meta with the new name/size/uploaded_at/title/version — id preserved.
    let chunk_count = write_doc_and_index(
        app,
        &dir,
        id,
        &file_stem,
        &name,
        &content,
        tags,
        size,
        None,
        version,
        new_method,
        new_original_path,
        new_md5,
    )
    .await?;

    Ok(chunk_count)
}

/// Update a doc by re-reading its recorded `original_path` (the "from original"
/// path in the update dialog + batch update). Re-indexes the current content
/// of the source, refreshes the stored md5, and for "copy" docs rewrites the
/// copied file. id/tags preserved. Requires RAG enabled. Returns Err if the
/// doc has no original_path (legacy) or the source is missing/invalid.
///
/// Holds META_LOCK across the whole read-modify-write so a concurrent
/// delete/update of the same doc can't interleave (lock order: meta -> runtime).
pub async fn update_doc_from_original(app: &AppHandle, id: &str) -> Result<u32> {
    let _meta_guard = meta_lock().await.lock().await;
    let dir = files_dir(app)?;
    let meta_path = dir.join(format!("{}.meta", id));
    let old_meta_bytes = std::fs::read(&meta_path)
        .map_err(|e| anyhow!("update-from-original: read meta {} failed: {}", id, e))?;
    let old_meta: DocMeta = serde_json::from_slice(&old_meta_bytes)?;

    let original_path = old_meta
        .original_path
        .as_deref()
        .filter(|p| !p.is_empty())
        .ok_or_else(|| anyhow!("no original path recorded for this document"))?;
    let path = Path::new(original_path);
    // Keep the doc's DISPLAY name (old_meta.name) rather than re-deriving it
    // from the source file name: the display name may have been changed via
    // MCP rag_file_update, and re-deriving would silently revert the rename.
    // It also keeps charProgress.name (reindex_doc's doc_name) in sync with
    // batchProgress.name (meta.name) for the frontend progress sub-bar.
    let name = old_meta.name.clone();
    if !path.exists() {
        return Err(anyhow!("original file does not exist: {}", original_path));
    }

    // Read + validate the source (same rules as upload — extractable formats
    // are re-extracted to Markdown, text via sniff+decode).
    let raw = std::fs::read(path).map_err(|e| anyhow!("read failed for {}: {}", name, e))?;
    if raw.len() > MAX_UPLOAD_BYTES {
        return Err(anyhow!(
            "file too large: {} ({} bytes, max {} bytes)",
            name,
            raw.len(),
            MAX_UPLOAD_BYTES
        ));
    }
    // Dispatch by the ORIGINAL PATH (the true source), not the display name:
    // the display name is user-renameable via MCP rag_file_update and may no
    // longer carry the source extension — a renamed "report.pdf" must still
    // re-extract, not fall into the text sniff (which would reject the bytes).
    let (content, _encoding) = content_from_source(original_path, &raw).await?;
    let size = raw.len() as u64;
    let tags = old_meta.tags.clone();

    // Remove the OLD copied file for "copy" docs if its name/extension changed
    // (symlink docs have no copy). For "copy" docs the on-disk name is
    // `{id}{ext}`; recompute it from the OLD meta name.
    let is_symlink = old_meta.method.as_deref() == Some("symlink");
    if !is_symlink {
        let old_content = content_path_for(&dir, id, &old_meta.name);
        if old_content.exists() {
            if let Err(e) = std::fs::remove_file(&old_content) {
                rag_log("warn", format!("update-from-original: remove old {} failed: {}", old_content.display(), e));
            }
        }
    }

    // On-disk filename: `{id}{ext}` for text sources, `{id}.md` for
    // extractable sources (derived Markdown — see upload_one_path_inner).
    // Extractable decision follows the source file (original_path), not the
    // renameable display name.
    let extractable = crate::rag::extract::can_extract(original_path);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e.to_ascii_lowercase()))
        .unwrap_or_default();
    let file_stem = if extractable {
        format!("{id}.md")
    } else {
        format!("{id}{ext}")
    };

    rag_log("info", format!("updating '{}' from original ({} bytes) -> id {}", name, raw.len(), id));
    let version = old_meta.version.saturating_add(1);
    let new_md5 = Some(compute_md5(&raw));
    let chunk_count = write_doc_and_index(
        app,
        &dir,
        id,
        &file_stem,
        &name,
        &content,
        tags,
        size,
        old_meta.file_type.clone(),
        version,
        // Extractable sources are always "copy" (derived content file).
        if extractable { Some("copy".to_string()) } else { old_meta.method.clone() },
        Some(original_path.to_string()),
        new_md5,
    )
    .await?;

    Ok(chunk_count)
}

/// Extract text content from raw file bytes via the strategy registry
/// (`rag::extract`): PDF/Office/image sources are parsed to Markdown, plain
/// text keeps the historical sniff + decode path (which also preserves the
/// detected-encoding label for the per-file import summary).
///
/// Sentinel semantics unchanged — `UNSUPPORTED_FORMAT` (unknown binary via
/// the text path) / `EXTRACT_FAILED` (supported kind, useless result) /
/// `OCR_MISSING` (no OCR engine on this platform) propagate verbatim.
async fn content_from_source(name: &str, raw: &[u8]) -> Result<(String, &'static str)> {
    if crate::rag::extract::can_extract(name) {
        let content = crate::rag::extract::run(name, raw.to_vec()).await?;
        return Ok((content, "extracted-md"));
    }
    if !is_likely_text(raw) {
        return Err(anyhow!("UNSUPPORTED_FORMAT: {}", name));
    }
    let (content, encoding) = decode_text(raw, name);
    Ok((content, encoding))
}

async fn upload_one_path_inner(
    app: &AppHandle,
    file_path: &str,
    tags: Vec<String>,
    method: Option<String>,
) -> Result<()> {
    let started = std::time::Instant::now();
    let dir = files_dir(app)?;
    std::fs::create_dir_all(&dir)?;

    let path = Path::new(file_path);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| file_path.to_string());

    // Read raw bytes from disk first (content-based validation, not extension).
    let t_read = std::time::Instant::now();
    let raw = std::fs::read(path)
        .map_err(|e| anyhow!("read failed for {}: {}", name, e))?;
    let read_ms = t_read.elapsed().as_millis();
    if raw.len() > MAX_UPLOAD_BYTES {
        return Err(anyhow!(
            "file too large: {} ({} bytes, max {} bytes)",
            name,
            raw.len(),
            MAX_UPLOAD_BYTES
        ));
    }
    // Reject unknown binaries by CONTENT. Extractable formats (PDF/Office/
    // images) bypass the NUL sniff — their bytes are inherently binary and
    // are parsed to Markdown by the extract strategies instead. Returns a
    // sentinel the frontend maps to a localized message.
    let (content, encoding) = content_from_source(&name, &raw).await?;
    let size = raw.len() as u64;
    let char_count = content.chars().count() as u64;

    // Upload always creates a NEW document (fresh uuid). We do NOT overwrite
    // same-named docs anymore — the on-disk filename is `{uuid}.{ext}` (unique
    // by uuid), so two uploads of "report.txt" coexist as separate docs. The
    // per-file "update" button (update_doc) is the explicit overwrite path:
    // it replaces one doc's content + vectors + meta by id.
    let mut seen_lower: std::collections::HashSet<String> = std::collections::HashSet::new();
    let tags = tags
        .iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty() && seen_lower.insert(t.to_lowercase()))
        .collect::<Vec<_>>();
    let id = Uuid::new_v4().to_string();
    rag_log(
        "info",
        format!(
            "uploaded '{}' ({} bytes, {} chars, encoding={}), indexing...",
            name, raw.len(), char_count, encoding
        ),
    );
    // On-disk filename: `{id}{ext}` for text sources (uuid + original
    // extension — unique across same-named uploads, shows its type in the OS
    // file manager); `{id}.md` for extractable sources (PDF/Office/image),
    // because the stored content is the DERIVED Markdown, not the raw bytes.
    let extractable = crate::rag::extract::can_extract(&name);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e.to_ascii_lowercase()))
        .unwrap_or_default();
    let file_stem = if extractable {
        format!("{id}.md")
    } else {
        format!("{id}{ext}")
    };
    // MD5 of the source bytes — captured at import for update detection
    // (symlink: live source; copy: the original that was copied). For symlink
    // docs the content is read fresh from original_path at view/index time, so
    // the stored md5 is the baseline for "has it changed?".
    let md5_hex = Some(compute_md5(&raw));
    let original_path_str = Some(file_path.to_string());
    // reindex_doc returns the chunk count AND drives the per-batch char-progress
    // events for the UI's second progress bar.
    let chunk_count = write_doc_and_index(
        app,
        &dir,
        &id,
        &file_stem,
        &name,
        &content,
        tags.clone(),
        size,
        None,
        1,
        // Extractable sources ALWAYS import as "copy": the content file is
        // derived Markdown, so the symlink/copy distinction (about the raw
        // source bytes) doesn't apply — all read paths then treat it as a
        // plain copy document with zero special-casing.
        if extractable { Some("copy".to_string()) } else { method.clone() },
        original_path_str,
        md5_hex,
    )
    .await?;

    // Re-sync tag stats after this file's tags are written.

    // Per-file import summary — one structured line per file so the Logs page
    // (filter server=rag) gives an at-a-glance read of import cost for tuning
    // chunk_size / diagnosing slow imports. Sizes/encoding/timings/chunks all
    // in one place.
    rag_log(
        "info",
        format!(
            "indexed '{}' done: size={}B chars={} encoding={} chunks={} readMs={} totalMs={}",
            name,
            raw.len(),
            char_count,
            encoding,
            chunk_count,
            read_ms,
            started.elapsed().as_millis()
        ),
    );
    Ok(())
}

/// Find the ids of all docs whose stored display `name` equals `name` (for
/// overwrite-on-re-upload). Reads `.meta` files; returns an empty vec if none.
/// (meta files are named `{id}.meta`, so the id is the filename stem - but we
/// return the id from inside the meta to be robust.)
fn find_doc_ids_by_name(dir: &Path, name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("meta") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let Ok(meta) = serde_json::from_slice::<DocMeta>(&bytes) else { continue };
        if meta.name == name {
            out.push(meta.id);
        }
    }
    out
}

/// Resolve the content file path for a doc. Uploads name the content file by
/// the doc id (uuid); `rag_file_create` names it by `{docName}.{docType}`
/// (which equals `meta.name`). Try `dir/{id}` first, then `dir/{meta_name}` so
/// both naming schemes resolve without scanning.
/// Resolve the on-disk content path for a document. Tries, in order:
/// 1. `dir/{id}{ext}` — the current upload scheme (uuid + original extension,
///    so the file shows its type in the OS file manager). `ext` is taken from
///    `meta_name` (the display name, which carries the original extension for
///    uploads and is `{name}.{docType}` for rag_file_create).
/// 2. `dir/{id}.md` — extractable imports (PDF/Office/image): their stored
///    content is the DERIVED Markdown, named `{id}.md` regardless of the
///    source extension.
/// 3. `dir/{id}` — the legacy upload scheme (uuid, no extension) for docs
///    uploaded before the extension-preserving change. Kept for back-compat.
/// 4. `dir/{meta_name}` — rag_file_create docs (human-readable name).
/// Returns the first candidate that exists; if none exist, the last candidate
/// (`meta_name`) so callers get a sensible path to create/remove/error on.
fn content_path_for(dir: &Path, id: &str, meta_name: &str) -> std::path::PathBuf {
    // Extract the extension (with the dot, e.g. ".txt") from the display name.
    // For uploads meta_name = original filename; for rag_file_create it's
    // `{name}.{docType}`. Either way the extension matches what was written.
    let ext = std::path::Path::new(meta_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e.to_ascii_lowercase()));
    // Candidate 1: {id}{ext} (current scheme). Only when there IS an ext.
    if let Some(ref ext) = ext {
        let by_id_ext = dir.join(format!("{id}{ext}"));
        if by_id_ext.exists() {
            return by_id_ext;
        }
    }
    // Candidate 2: {id}.md (extracted-Markdown content of PDF/Office/image
    // imports).
    let by_id_md = dir.join(format!("{id}.md"));
    if by_id_md.exists() {
        return by_id_md;
    }
    // Candidate 3: {id} (legacy upload, no extension).
    let by_id = dir.join(id);
    if by_id.exists() {
        return by_id;
    }
    // Candidate 4: {meta_name} (rag_file_create).
    dir.join(meta_name)
}

/// Compute the MD5 hex digest of `bytes`. Used to fingerprint imported file
/// content for update detection (symlink/copy): the hash is stored in
/// `DocMeta.md5` and compared against a fresh hash of the source on update
/// check. Cheap to compute vs re-embedding, so it's the gate before any
/// expensive re-index.
fn compute_md5(bytes: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    // 16 bytes -> 32 hex chars
    digest.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Read a file's bytes and compute its MD5. Returns Ok(hash) or Err if the
/// file can't be read (caller decides what "unreadable" means — e.g. a missing
/// symlink source is "lost original", not a hard error).
fn md5_of_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(compute_md5(&bytes))
}

/// Classify a `DocMeta`'s original-source state for the single-doc update
/// dialog + batch preview. Centralizes the three legacy-compat rules:
///   - no `method`        -> treated as "copy"
///   - no `original_path` -> has_original_path=false (legacy, manual-upload only)
///   - no `md5`           -> has_md5=false (legacy -> original_changed=true if exists)
/// `lost_original` is symlink-method + original_path missing.
fn classify_original(meta: &DocMeta) -> RagUpdateCheck {
    let method = meta.method.clone().unwrap_or_else(|| "copy".to_string());
    let is_symlink = method == "symlink";
    let original_path = meta.original_path.as_deref();
    let has_original_path = original_path.map(|p| !p.is_empty()).unwrap_or(false);
    let original_exists = has_original_path && Path::new(original_path.unwrap()).exists();
    let has_md5 = meta.md5.as_deref().map(|m| !m.is_empty()).unwrap_or(false);
    // original_changed: source exists AND (no stored md5 -> legacy "has update",
    // OR stored md5 != fresh hash). If the source can't be read we treat it as
    // not-changed rather than erroring (the caller already knows original_exists).
    let original_changed = if original_exists && has_md5 {
        if let Ok(fresh) = md5_of_file(Path::new(original_path.unwrap())) {
            fresh != meta.md5.as_deref().unwrap_or("")
        } else {
            false
        }
    } else {
        original_exists && !has_md5
    };
    // lost_original: the recorded original_path is missing on disk, regardless
    // of method. Both symlink (no copy) AND copy (copy exists but source gone)
    // docs count as lost — the batch preview/run pass + the list UI all surface
    // them as "原始丢失" so the user sees the full count. (Earlier this was
    // symlink-only, which made copy docs whose source vanished show as "skipped"
    // in the batch preview — inconsistent with the list's lostOriginal badge.)
    let lost_original = has_original_path && !original_exists;
    let _ = is_symlink; // kept for clarity; not gating lost_original anymore.
    RagUpdateCheck {
        method,
        has_original_path,
        original_exists,
        has_md5,
        original_changed,
        lost_original,
    }
}

/// Resolve a file-type label from a docType extension (e.g. "md" -> "Markdown",
/// "java" -> "Java"). Used by `rag_file_create`/`rag_file_update` to persist an
/// explicit type even when the filename has no extension. Returns None if the
/// extension isn't in the catalog.
fn file_type_label_from_ext(doc_type: &str) -> Option<String> {
    let dt = doc_type.trim().trim_start_matches('.');
    if dt.is_empty() {
        return None;
    }
    let label = file_type_label(&format!(".{}", dt.to_lowercase()));
    if label.is_empty() {
        None
    } else {
        Some(label)
    }
}

/// Sanitize a caller-supplied docName into a safe single-segment filename:
/// replace path separators and other shell-unsafe chars with `-`, strip
/// leading dots (so the file isn't hidden / can't be `.` or `..`). Returns ""
/// if the result is empty.
fn sanitize_file_name(name: &str) -> String {
    let s: String = name
        .trim()
        .chars()
        .map(|c| match c {
            '/' | '\\' | '\0' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            _ => c,
        })
        .collect();
    let s = s.trim_start_matches('.').trim().to_string();
    if s.is_empty() || s == "." || s == ".." {
        String::new()
    } else {
        s
    }
}

/// `rag_file_create` MCP tool: create a document from UTF-8 content (no
/// encoding detection - the tool input is already a UTF-8 string). Writes the
/// file as `{docName}.{docType}` (human-readable) under files/, indexes it into
/// lancedb, and returns the new docId. Overwrites any existing doc with the
/// same resolved filename. `docType` is a bare extension like "md"/"java"/"py".
pub async fn create_doc_from_content(
    app: &AppHandle,
    doc_name: &str,
    doc_type: &str,
    content: &str,
    tags: Vec<String>,
) -> Result<String> {
    let dir = files_dir(app)?;
    std::fs::create_dir_all(&dir)?;

    let name_base = sanitize_file_name(doc_name);
    if name_base.is_empty() {
        return Err(anyhow!("docName must not be empty"));
    }
    let dt = doc_type.trim().trim_start_matches('.');
    if dt.is_empty() {
        return Err(anyhow!("docType must not be empty"));
    }
    // If docName already ends with .{docType}, keep it; else append.
    let file_name = if name_base.to_lowercase().ends_with(&format!(".{}", dt.to_lowercase())) {
        name_base.clone()
    } else {
        format!("{}.{}", name_base, dt)
    };

    let byte_len = content.len();
    if byte_len > MAX_UPLOAD_BYTES {
        return Err(anyhow!(
            "docContent too large: {} bytes, max {} bytes",
            byte_len, MAX_UPLOAD_BYTES
        ));
    }
    let file_type = file_type_label_from_ext(dt);
    // Sanitize tags (trim, drop empty, case-insensitive dedup keeping the
    // first spelling) - same rules as the upload path.
    let mut seen_lower: std::collections::HashSet<String> = std::collections::HashSet::new();
    let tags: Vec<String> = tags
        .into_iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty() && seen_lower.insert(t.to_lowercase()))
        .collect();

    // Overwrite same-named doc (same as upload path).
    let stale_ids = find_doc_ids_by_name(&dir, &file_name);
    if !stale_ids.is_empty() {
        for sid in &stale_ids {
            let _ = std::fs::remove_file(content_path_for(&dir, sid, &file_name));
            let _ = std::fs::remove_file(dir.join(format!("{}.meta", sid)));
        }
        rag_log(
            "info",
            format!(
                "rag_file_create overwriting '{}' ({} doc(s))",
                file_name,
                stale_ids.len()
            ),
        );
    }

    let id = Uuid::new_v4().to_string();
    rag_log(
        "info",
        format!("rag_file_create '{}' ({} bytes), indexing...", file_name, byte_len),
    );
    // rag_file_create has no on-disk source file (content is passed inline),
    // so it's a "copy" with no original_path + an md5 of the content for
    // consistency with the import-method feature.
    let _chunk_count = write_doc_and_index(
        app,
        &dir,
        &id,
        &file_name,
        &file_name,
        content,
        tags,
        byte_len as u64,
        file_type,
        1,
        Some("copy".to_string()),
        None,
        Some(compute_md5(content.as_bytes())),
    )
    .await?;

    // Remove overwritten docs' vector chunks + reclaim space.
    if !stale_ids.is_empty() {
        let guard = runtime().lock().await;
        if let Some(rt) = guard.as_ref() {
            for sid in &stale_ids {
                let _ = rt.db.delete_by_doc(sid).await;
            }
            // Reclaim the overwritten docs' freed space in the background
            // (deduped) instead of blocking this call on a slow inline Prune.
            // See `schedule_deferred_prune`.
            schedule_deferred_prune(app);
        }
    }
    // The stale docs' .meta files were removed above; drop their SQL mirror
    // rows too (their tags counted toward rag_tags). The new doc's mirror row
    // is upserted inside write_doc_and_index.
    for sid in &stale_ids {
        if let Err(e) = remove_doc_sql(sid).await {
            rag_log("warn", format!("remove_doc_sql for stale {} failed: {}", sid, e));
        }
    }
    Ok(id)
}

/// `rag_file_update` MCP tool: update a document's name/type/content. docId is
/// stable. When `docContent` is provided, `docContentAppend` decides append
/// (old + "\n" + new) vs replace; the old vector chunks are deleted and the
/// new content re-indexed (via `reindex_doc`, which delete_by_doc + add_chunks).
/// A name change on a `rag_file_create` doc renames the on-disk content file
/// (uploads name content by id, so they're untouched).
pub async fn update_doc(
    app: &AppHandle,
    doc_id: &str,
    name: Option<&str>,
    doc_type: Option<&str>,
    content: Option<&str>,
    append: bool,
    add_tags: Vec<String>,
    remove_tags: Vec<String>,
) -> Result<()> {
    let _meta_guard = meta_lock().await.lock().await;
    let dir = files_dir(app)?;
    let meta_path = dir.join(format!("{}.meta", doc_id));
    let Ok(meta_bytes) = std::fs::read(&meta_path) else {
        return Err(anyhow!("document not found: {}", doc_id));
    };
    let mut meta: DocMeta = serde_json::from_slice(&meta_bytes)?;
    let old_name = meta.name.clone();
    let content_path = content_path_for(&dir, doc_id, &old_name);

    // Update display name.
    if let Some(n) = name {
        let n = sanitize_file_name(n);
        if !n.is_empty() {
            meta.name = n;
        }
    }
    // Update persisted file-type label.
    if let Some(dt) = doc_type {
        meta.file_type = file_type_label_from_ext(dt);
    }
    // Update tags: add new ones, remove specified ones (case-insensitive dedup,
    // preserve order of existing + additions). Tags live on each chunk in
    // lancedb, so a tag change requires re-indexing to propagate.
    let tags_changed = !add_tags.is_empty() || !remove_tags.is_empty();
    if tags_changed {
        let remove_set: std::collections::HashSet<String> = remove_tags
            .iter()
            .map(|t| t.trim().to_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        let mut new_tags: Vec<String> = meta
            .tags
            .iter()
            .filter(|t| !remove_set.contains(&t.trim().to_lowercase()))
            .cloned()
            .collect();
        for t in &add_tags {
            let t = t.trim();
            if !t.is_empty()
                && !new_tags.iter().any(|x| x.trim().eq_ignore_ascii_case(t))
            {
                new_tags.push(t.to_string());
            }
        }
        meta.tags = new_tags;
    }
    // Re-index when content changed OR tags changed (tag edits must propagate
    // to the chunks' stored tags). Reads existing content from disk when only
    // tags changed. For symlink docs the content lives at original_path, not
    // the rag/files copy — read/write there instead.
    let mut content_text_for_md5: Option<String> = None;
    if content.is_some() || tags_changed {
        let is_symlink = meta.method.as_deref() == Some("symlink");
        // Symlink docs are read-only through the MCP rag_file_update tool:
        // writing content would overwrite the user's ORIGINAL file (there is
        // no copy). Reject with a clear message instead.
        if is_symlink && content.is_some() {
            return Err(anyhow!(
                "symlink documents are read-only via rag_file_update (writing would overwrite the original file); use the UI update dialog or re-import"
            ));
        }
        // Tag-only update on a symlink doc whose original is missing: the
        // content read below yields "" and reindex_doc would DELETE the old
        // vectors and insert 0 chunks (irreversible data loss). In that case
        // skip the re-index and just persist the meta tags below; the chunks
        // keep their existing tags/embeddings.
        let symlink_lost = is_symlink
            && meta
                .original_path
                .as_deref()
                .map(|p| !p.is_empty() && !Path::new(p).exists())
                .unwrap_or(false);
        if symlink_lost && content.is_none() {
            rag_log(
                "warn",
                format!("rag_file_update: '{}' original missing; tags saved without re-index (chunks keep old tags)", meta.name),
            );
        } else {
        let content_path = if is_symlink {
            meta.original_path
                .as_deref()
                .filter(|p| !p.is_empty())
                .map(Path::new)
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| content_path.clone())
        } else {
            content_path.clone()
        };
        let content_text = if let Some(c) = content {
            let new_content = if append {
                let old = std::fs::read_to_string(&content_path).unwrap_or_default();
                format!("{}\n{}", old, c)
            } else {
                c.to_string()
            };
            let byte_len = new_content.len();
            if byte_len > MAX_UPLOAD_BYTES {
                return Err(anyhow!(
                    "docContent too large: {} bytes, max {} bytes",
                    byte_len, MAX_UPLOAD_BYTES
                ));
            }
            std::fs::write(&content_path, &new_content)?;
            meta.size = byte_len as u64;
            new_content
        } else {
            std::fs::read_to_string(&content_path).unwrap_or_default()
        };
        let chunk_count =
            reindex_doc(app, doc_id, &meta.name, &content_text, meta.tags.clone()).await? as u32;
        meta.chunk_count = chunk_count;
        if content.is_some() {
            content_text_for_md5 = Some(content_text);
        }
        } // else (symlink_lost tag-only): skipped re-index above
    }

    // If the content file is named by old_name (rag_file_create doc, not uuid)
    // and the name changed, rename it so content_path_for still resolves.
    // (Symlink docs have no copied file, so this only applies to copy docs.)
    if meta.name != old_name {
        let by_id = dir.join(doc_id);
        if !by_id.exists() {
            let old_path = dir.join(&old_name);
            let new_path = dir.join(&meta.name);
            if old_path.exists() && old_path != new_path {
                let _ = std::fs::rename(&old_path, &new_path);
            }
        }
    }

    // If content changed, refresh the stored md5 so future update detection
    // compares against the new baseline.
    if let Some(ct) = content_text_for_md5 {
        meta.md5 = Some(compute_md5(ct.as_bytes()));
    }

    write_meta_atomic(&meta_path, &meta)?;
    if let Err(e) = upsert_doc_sql(&meta).await {
        rag_log("warn", format!("upsert_doc_sql for '{}' failed: {}", meta.name, e));
    }
    Ok(())
}

/// Hard cap to avoid indexing pathological files (embed cost is ~linear).
const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024; // 64 MiB

/// Decode raw bytes into a UTF-8 `String` and report the detected encoding.
/// Strategy (fast -> fallback):
/// 1. UTF-8 BOM / valid UTF-8 -> SIMD-validated by `std::str::from_utf8`, BOM
///    stripped. The common case; no detector runs.
/// 2. Otherwise `chardetng` detects (Mozilla statistical; honors BOMs incl.
///    UTF-16) and `encoding_rs` converts (SIMD). Covers GBK/GB18030, Big5,
///    Shift-JIS, EUC-*, ISO-8859-*, KOI8, Mac, IBM families.
/// Logs the detected encoding, byte count, and timing. Returns `(text, enc)`
/// where `enc` is the canonical encoding name (e.g. "UTF-8", "gb18030") so the
/// per-file import summary can include the encoding without re-detecting.
///
/// Shared with skill_service (SKILL.md parse) so non-UTF-8 frontmatter can be
/// read. Callers that COPY files (skill import, rag file copy) use byte-level
/// `fs::copy` and keep the original bytes — only parsing decodes.
pub fn decode_text(bytes: &[u8], source: &str) -> (String, &'static str) {
    let started = std::time::Instant::now();
    let n = bytes.len();

    // Always run the detector so the original encoding is reported (never a
    // hardcoded label). chardetng honors BOMs (UTF-8/16) + does statistical
    // disambiguation (GBK vs Big5 vs ...).
    let mut det = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Allow);
    det.feed(bytes, true);
    let enc = det.guess(None, chardetng::Utf8Detection::Allow);

    // Conversion: valid UTF-8 (SIMD-validated) -> no conversion, just strip a
    // leading BOM. Otherwise decode via the detected encoding (SIMD).
    let (out, convert, had_bom) = match std::str::from_utf8(bytes) {
        Ok(s) => {
            let had_bom = s.starts_with('\u{FEFF}');
            let s = s.strip_prefix('\u{FEFF}').unwrap_or(s);
            (s.to_string(), false, had_bom)
        }
        Err(_) => {
            let (cow, _enc_used, had_bom) = enc.decode(bytes);
            (cow.into_owned(), true, had_bom)
        }
    };

    rag_log(
        "info",
        format!(
            "file='{}' originalEncoding={} (bom={}) bytes={} convert={} {}ms",
            source,
            enc.name(),
            had_bom,
            n,
            convert,
            started.elapsed().as_millis()
        ),
    );
    (out, enc.name())
}

/// Chunk + embed + (re)write all chunks of a document. Used by upload and by
/// `set_doc_tags`. The runtime must be enabled. Holds the runtime lock for
/// the whole batch. If the doc already has chunks (tag edit), deletes them
/// first so the new chunks with updated tags replace them.
///
/// Drives the UI's per-document progress bar: chunks are embedded in
/// `EMBED_BATCH_SIZE`-sized sub-batches and after each sub-batch we emit a
/// `rag://upload-progress` event with the cumulative character count, so the
/// second (char-based) progress bar advances once per `session.run`.
async fn reindex_doc(
    app: &AppHandle,
    doc_id: &str,
    doc_name: &str,
    content: &str,
    tags: Vec<String>,
) -> Result<usize> {
    let settings = get_settings().await?;
    let n_chunks;
    {
        let mut guard = runtime().lock().await;
        let Some(rt) = guard.as_mut() else {
            return Err(anyhow!("RAG not enabled"));
        };
        // Resolve the effective chunk size / overlap. `0` in the user's global
        // setting means "auto" -> use the model's deploy.json-recommended value
        // (chunkSize/chunkOverlap); a positive value is an explicit override.
        // `chunk_size` is capped by the loaded model's context window so a chunk
        // can never exceed what the embedder accepts (the GGUF/ONNX backends
        // silently truncate at max_context, which would drop the chunk tail).
        // Fallbacks: 1024 / 100 when neither the user nor deploy.json specify.
        let max_ctx = rt.model.max_context().max(1) as u32;
        let chunk_size = match settings.chunk_size {
            0 => rt.deploy_chunk_size.unwrap_or(1024),
            v => v,
        }
        .min(max_ctx)
        .max(1) as usize;
        let chunk_overlap = match settings.chunk_overlap {
            0 => rt.deploy_chunk_overlap.unwrap_or(100),
            v => v,
        } as usize;
        // Chunk via the strategy pattern (text/markdown/code) — text-splitter
        // sizes each chunk in tokens via the loaded model's tokenizer, so
        // chunk_size maps directly to the model's context budget. Picked by
        // the file extension (CodeSplitter for source, MarkdownSplitter for
        // .md, TextSplitter otherwise). See `rag/chunker.rs`.
        let chunks = chunk_document(doc_name, content, &*rt.model, chunk_size as u32, chunk_overlap as u32);
        // Total chars (UTF-8 chars, not bytes) drives the per-file progress bar.
        let total_chars = content.chars().count() as u64;

        // Adaptive batch size: if the doc has few chunks (<=32), process them
        // ALL in one embed_batch call -> one big GEMM (max BLAS/AMX efficiency).
        // If many chunks, batch in 32 -> multiple progress ticks (the bar moves)
        // while keeping each GEMM large enough for efficient tiling. 32 is a
        // sweet spot: big enough for AMX/BLAS GEMM efficiency, small enough that
        // a 1000-chunk doc still gets ~30 progress ticks. This replaces the old
        // hardcoded 8 (too small for f32 BLAS efficiency).
        let batch_size = chunks.len().min(32);
        let mut embeddings: Vec<Vec<f32>> = Vec::with_capacity(chunks.len());
        let mut chars_done: u64 = 0;
        let t_embed = std::time::Instant::now();
        // Emit a 0% tick immediately so the UI's per-document bar shows the
        // real char total (and leaves "Preparing…") before the first - possibly
        // slow - forward finishes. Without this a large file shows no doc
        // progress for the duration of its first embedding batch.
        emit_upload_progress(app, doc_name, 0, total_chars);
        // Empty / whitespace-only docs produce no chunks (`chunk_document`
        // returns an empty vec after trim). Skip the batch loop in that case:
        // `Vec::chunks(0)` panics with "chunk size must be non-zero", so without
        // this guard an empty file (e.g. 0-byte upload or whitespace-only) would
        // panic during reindex — a latent bug surfaced by reindexing after a
        // model swap. The doc is still stored, just with zero searchable chunks.
        // Prepend the model's document prefix (deploy.json `importDocPrefix`) to
        // each chunk before embedding - asymmetric models (Qwen3, BGE) require a
        // distinct prefix on the document side. Cloned once (a few bytes) so the
        // mutable `rt.model.embed_batch` borrow below is unencumbered. The
        // stored chunk_text + the char-progress count use the ORIGINAL chunk
        // (no prefix) so retrieved snippets + the progress bar reflect the real
        // content, and the prefix doesn't inflate the char total.
        let import_doc_prefix = rt.import_doc_prefix.clone();
        // For an empty doc the loop below doesn't run (no chunks to embed), so
        // emit the 100% tick here to finish its progress bar.
        if chunks.is_empty() {
            emit_upload_progress(app, doc_name, total_chars, total_chars);
        }
        for sub in chunks.chunks(batch_size.max(1)) {
            let prefixed: Vec<String> = sub
                .iter()
                .map(|c| format!("{}{}", import_doc_prefix, c))
                .collect();
            let sub_refs: Vec<&str> = prefixed.iter().map(String::as_str).collect();
            let embs = if sub_refs.is_empty() {
                Vec::new()
            } else {
                rt.model
                    .embed_batch(&sub_refs)
                    .map_err(|e| anyhow!("embed_batch failed for '{}': {}", doc_name, e))?
            };
            let sub_chars: u64 = sub.iter().map(|c| c.chars().count() as u64).sum();
            chars_done = chars_done.saturating_add(sub_chars);
            embeddings.extend(embs);
            emit_upload_progress(app, doc_name, chars_done, total_chars);
        }
        let embed_ms = t_embed.elapsed().as_millis();

        rag_log(
            "info",
            format!(
                "indexed '{}' -> {} chunks (chunk_size={} overlap={}, embedMs={})",
                doc_name,
                chunks.len(),
                chunk_size,
                chunk_overlap,
                embed_ms
            ),
        );
        // Remove any existing chunks for this doc (tag re-edit / re-upload).
        let _ = rt.db.delete_by_doc(doc_id).await;
        let inputs: Vec<ChunkInput> = chunks
            .iter()
            .enumerate()
            .zip(embeddings.iter())
            .map(|((i, text), emb)| ChunkInput {
                chunk_id: Uuid::new_v4().to_string(),
                doc_id: doc_id.to_string(),
                doc_name: doc_name.to_string(),
                chunk_index: i as i64,
                chunk_text: text.clone(),
                embedding: emb.as_slice(),
                tags: tags.clone(),
            })
            .collect();
        rt.db
            .add_chunks(&inputs)
            .await
            .map_err(|e| anyhow!("add_chunks failed for '{}': {}", doc_name, e))?;
        n_chunks = chunks.len();
    }
    Ok(n_chunks)
}

/// Zero the on-disk `chunk_count` of every doc's `.meta` — called right after
/// the vector table is recreated on a model swap (dim mismatch). Until
/// `reindex_all` repopulates them, the list view shows 0 (honest: the table is
/// empty) instead of a stale pre-swap count.
fn zero_all_chunk_counts(app: &AppHandle) -> Result<()> {
    let dir = files_dir(app)?;
    if !dir.exists() {
        return Ok(());
    }
    let mut n = 0u32;
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("meta") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let Ok(mut meta) = serde_json::from_slice::<DocMeta>(&bytes) else { continue };
        if meta.chunk_count == 0 {
            continue;
        }
        meta.chunk_count = 0;
        if std::fs::write(&path, serde_json::to_vec(&meta)?).is_ok() {
            n += 1;
        }
    }
    if n > 0 {
        rag_log(
            "info",
            format!("zeroed chunk_count for {} doc(s) — model swapped, reindex pending", n),
        );
    }
    Ok(())
}

/// Re-embed every uploaded doc with the currently-loaded model, after a model
/// swap recreated the vector table (different embedding dim). Reads each doc's
/// content file + meta, re-chunks + re-embeds (reusing `reindex_doc`, which
/// also emits the per-doc char-progress bar), and rewrites the `.meta`
/// chunk_count. Emits `rag://reindex-progress` per doc so the frontend's
/// upload overlay (reused for reindex) shows the file-level bar. Clears
/// `NEEDS_REINDEX` when done.
///
/// The content files are never touched (only embeddings are regenerated), so
/// tags / titles / display names survive a model swap untouched.
pub async fn reindex_all(app: &AppHandle) -> Result<usize> {
    let dir = files_dir(app)?;
    if !dir.exists() {
        NEEDS_REINDEX.store(false, std::sync::atomic::Ordering::SeqCst);
        return Ok(0);
    }
    // Collect (meta_path, meta) so we own the data and can rewrite metas in place.
    let mut docs: Vec<(PathBuf, DocMeta)> = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("meta") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let Ok(meta) = serde_json::from_slice::<DocMeta>(&bytes) else { continue };
        docs.push((path, meta));
    }
    let total = docs.len() as u32;
    rag_log("info", format!("reindexing all docs ({} docs, model swapped)…", total));
    emit_reindex_progress(app, 0, total, "");
    let mut done = 0usize;
    for (i, (meta_path, mut meta)) in docs.into_iter().enumerate() {
        emit_reindex_progress(app, i as u32, total, &meta.name);
        // Read the doc content. MUST use `content_path_for` (not `dir.join(id)`)
        // — uploads are stored as `{id}.{ext}` (ext from the original filename),
        // so a bare `{id}` path doesn't exist and `read_to_string` would fail ->
        // empty content -> 0 chunks (looked like a silent reindex "success").
        // For "symlink" docs there's no copy — read live from original_path.
        let is_symlink = meta.method.as_deref() == Some("symlink");
        let content = if is_symlink {
            match meta
                .original_path
                .as_deref()
                .filter(|p| !p.is_empty())
                .map(Path::new)
            {
                Some(p) if p.exists() => std::fs::read_to_string(p).unwrap_or_default(),
                _ => {
                    rag_log(
                        "warn",
                        format!(
                            "reindex: symlink '{}' original missing ({}), skipped",
                            meta.name,
                            meta.original_path.as_deref().unwrap_or("")
                        ),
                    );
                    continue;
                }
            }
        } else {
            let content_path = content_path_for(&dir, &meta.id, &meta.name);
            match std::fs::read_to_string(&content_path) {
                Ok(c) => c,
                Err(e) => {
                    rag_log(
                        "warn",
                        format!(
                            "reindex: content read failed for '{}' ({}): {}",
                            meta.name,
                            content_path.display(),
                            e
                        ),
                    );
                    continue;
                }
            }
        };
        let tags = meta.tags.clone();
        match reindex_doc(app, &meta.id, &meta.name, &content, tags).await {
            Ok(cc) => {
                meta.chunk_count = cc as u32;
                if let Err(e) = write_meta_atomic(&meta_path, &meta) {
                    rag_log("warn", format!("reindex: rewrite meta for '{}' failed: {}", meta.name, e));
                }
                done += 1;
            }
            Err(e) => {
                rag_log(
                    "warn",
                    format!("reindex: re-embed failed for '{}': {:#}", meta.name, e),
                );
            }
        }
    }
    NEEDS_REINDEX.store(false, std::sync::atomic::Ordering::SeqCst);
    emit_reindex_progress(app, total, total, "");
    // Tags are unchanged by reindex (carried over), but a full mirror rebuild is
    // cheap and keeps the SQL tables consistent if any meta was skipped/corrupt.
    if let Err(e) = rebuild_rag_sql_index(app).await {
        rag_log("warn", format!("rebuild_rag_sql_index after reindex failed: {}", e));
    }
    rag_log("info", format!("reindexed all docs ({} of {} ok)", done, total));
    Ok(done)
}

/// Set the absolute tag list for a document: updates `.meta` and re-writes the
/// doc's chunks with the new tags WITHOUT re-running the embedding model
/// (reads existing chunks + embeddings, deletes them, re-inserts with new tags).
///
/// Requires RAG enabled. When the runtime is `None` (RAG off) this returns an
/// error rather than silently leaving chunks carrying the old tags - the UI
/// disables tag editing when RAG is off, this is the code-level guard for
/// other call paths. The runtime check happens before `.meta` is written, so a
/// failure leaves the document unchanged.
pub async fn set_doc_tags(app: &AppHandle, id: &str, tags: Vec<String>) -> Result<()> {
    let _meta_guard = meta_lock().await.lock().await;
    let dir = files_dir(app)?;
    let meta_path = dir.join(format!("{}.meta", id));
    let Ok(meta_bytes) = std::fs::read(&meta_path) else {
        return Err(anyhow!("document not found: {}", id));
    };
    let mut meta: DocMeta = serde_json::from_slice(&meta_bytes)?;
    // 规整 + 去重：trim、去空、同标签（大小写不敏感）只保留首个。后端兜底
    // （前端 TagEditor 已按精确匹配去重，这里覆盖批量路径/直接调命令的路径）。
    let mut seen_lower: std::collections::HashSet<String> = std::collections::HashSet::new();
    let tags: Vec<String> = tags
        .into_iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty() && seen_lower.insert(t.to_lowercase()))
        .collect();

    // Rewrite the chunks' tags in place (reusing existing embeddings, no
    // re-embed) and prune the replaced chunks. Requires RAG enabled - refuse
    // (rather than silently leaving stale-tag chunks) when the runtime is off.
    // Hold the runtime lock across the meta write + chunk rewrite so RAG can't
    // be toggled mid-op and leave meta/lancedb out of sync. META_LOCK (held
    // above) serializes against concurrent update/delete of the same doc.
    {
        let guard = runtime().lock().await;
        let rt = guard
            .as_ref()
            .ok_or_else(|| anyhow!("RAG is not enabled - turn on RAG before editing tags"))?;
        meta.tags = tags.clone();
        write_meta_atomic(&meta_path, &meta)?;
        rewrite_chunks_with_tags(&rt.db, id, &tags).await?;
    }
    rag_log("info", format!("updated tags for '{}' ({} tags)", meta.name, tags.len()));
    // The SQL mirror update (rag_tags / rag_doc_tags / rag_docs) is best-effort
    // relative to the .meta + chunk rewrites above, which already succeeded.
    // Retry on a busy lock: the pool is configured with a 5s busy_timeout, but
    // a write that lands just past the window should still heal the mirror
    // rather than silently leaving rag_tags empty (the original "tag dropdown
    // shows no list" bug). Bail after a few attempts so we never hang the UI.
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 1..=4 {
        match upsert_doc_sql(&meta).await {
            Ok(()) => {
                last_err = None;
                break;
            }
            Err(e) => {
                let msg = format!("{}", e);
                let busy = msg.contains("database is locked") || msg.contains("code: 5");
                if !busy || attempt == 4 {
                    last_err = Some(e);
                    break;
                }
                rag_log("warn", format!(
                    "upsert_doc_sql for '{}' busy (attempt {}/4), retrying: {}",
                    meta.name, attempt, msg
                ));
                tokio::time::sleep(std::time::Duration::from_millis(150 * attempt as u64)).await;
            }
        }
    }
    if let Some(e) = last_err {
        rag_log("warn", format!("upsert_doc_sql for '{}' failed: {}", meta.name, e));
    }
    Ok(())
}

/// Rewrite a document's chunks with `tags`, reusing the existing embeddings
/// (no re-embed). Deletes the old chunks first so the new ones replace them,
/// then prunes the freed space.
async fn rewrite_chunks_with_tags(db: &VectorDb, id: &str, tags: &[String]) -> Result<()> {
    let records = db.read_chunks_by_doc(id).await?;
    if records.is_empty() {
        return Ok(());
    }
    db.delete_by_doc(id).await?;
    let inputs: Vec<ChunkInput> = records
        .iter()
        .map(|r| ChunkInput {
            chunk_id: r.chunk_id.clone(),
            doc_id: r.doc_id.clone(),
            doc_name: r.doc_name.clone(),
            chunk_index: r.chunk_index,
            chunk_text: r.chunk_text.clone(),
            embedding: r.embedding.as_slice(),
            tags: tags.to_vec(),
        })
        .collect();
    db.add_chunks(&inputs).await?;
    db.optimize().await?;
    Ok(())
}

/// Delete a document: remove its files + all its chunks from the vector DB,
/// and reclaim the disk space those chunks occupied.
///
/// Requires RAG enabled. When the runtime is `None` (RAG off) this returns an
/// error rather than silently orphaning the chunks in lancedb - the UI
/// disables the delete button when RAG is off, this is the code-level guard
/// for other call paths (MCP, batch). lancedb cleanup runs before the files
/// are removed, so a failure leaves the document intact.
///
/// Holds META_LOCK for the whole delete (acquired BEFORE the runtime lock -
/// consistent ordering meta -> runtime with the update paths) so a concurrent
/// update of the same doc can't re-write the meta after we remove it (the
/// "deleted doc resurrected" race).
pub async fn delete_doc(app: &AppHandle, id: &str) -> Result<()> {
    let _meta_guard = meta_lock().await.lock().await;
    let dir = files_dir(app)?;

    // Remove the doc's chunks from lancedb + prune the freed space. Refuse if
    // RAG is off so we never delete the files while leaving orphan vectors.
    {
        let guard = runtime().lock().await;
        let rt = guard
            .as_ref()
            .ok_or_else(|| anyhow!("RAG is not enabled - turn on RAG before deleting documents"))?;
        rt.db.delete_by_doc(id).await?;
        // Reclaim the freed disk space in the background (deduped across a burst
        // of deletes) instead of blocking this IPC on a slow lancedb Prune —
        // see `schedule_deferred_prune`. The doc is already gone from the table
        // (delete_by_doc wrote a new version omitting it), so the list refresh
        // can return immediately while the prune runs moments later.
        schedule_deferred_prune(app);
    }

    // Content file is `dir/{id}` for uploads, `dir/{meta.name}` for
    // rag_file_create docs - read the meta to resolve the human-readable name.
    // Remove BOTH candidate paths (id + meta.name) so a content file is never
    // orphaned regardless of which naming scheme wrote it. Log any removal
    // failure (rather than `let _ =`) so a permissions/path bug surfaces in the
    // Logs page instead of silently leaving the file on disk.
    //
    // For "symlink" docs there is NO copied content file (the source lives at
    // original_path and is never owned by RAG) — only the `.meta` + vectors are
    // removed; the original file is left untouched.
    let meta_opt = std::fs::read(dir.join(format!("{}.meta", id)))
        .ok()
        .and_then(|b| serde_json::from_slice::<DocMeta>(&b).ok());
    let meta_name = meta_opt.as_ref().map(|m| m.name.clone());
    let is_symlink = meta_opt
        .as_ref()
        .map(|m| m.method.as_deref() == Some("symlink"))
        .unwrap_or(false);
    if !is_symlink {
        // Remove every candidate on-disk path (current `{id}{ext}` upload scheme,
        // legacy bare-`{id}` upload, and `{meta_name}` for rag_file_create) so no
        // content file is orphaned regardless of which scheme wrote it.
        let ext = meta_name
            .as_deref()
            .and_then(|n| std::path::Path::new(n).extension().and_then(|e| e.to_str()))
            .map(|e| format!(".{}", e.to_ascii_lowercase()));
        let mut candidates: Vec<std::path::PathBuf> = Vec::new();
        if let Some(ref ext) = ext {
            candidates.push(dir.join(format!("{id}{ext}")));
        }
        // Extracted-Markdown content of PDF/Office/image imports ({id}.md —
        // its name does NOT derive from meta_name, so it needs its own
        // candidate; deduped below for text .md docs where candidate 1 is
        // already this exact path).
        candidates.push(dir.join(format!("{id}.md")));
        candidates.push(dir.join(id));
        if let Some(ref name) = meta_name {
            let p = dir.join(name);
            if !candidates.contains(&p) {
                candidates.push(p);
            }
        }
        for p in &candidates {
            if p.exists() {
                if let Err(e) = std::fs::remove_file(p) {
                    rag_log("warn", format!("delete_doc: remove {} failed: {}", p.display(), e));
                }
            }
        }
    } else {
        rag_log("info", format!("delete_doc: symlink doc {} — original left untouched", id));
    }
    let meta_path = dir.join(format!("{}.meta", id));
    if meta_path.exists() {
        if let Err(e) = std::fs::remove_file(&meta_path) {
            rag_log("warn", format!("delete_doc: remove {} failed: {}", meta_path.display(), e));
        }
    }

    rag_log("info", format!("deleted document {}", id));
    if let Err(e) = remove_doc_sql(id).await {
        rag_log("warn", format!("remove_doc_sql for {} failed: {}", id, e));
    }
    Ok(())
}

/// Reveal a document's file location in the OS file manager.
///
/// For "symlink" docs this reveals the ORIGINAL file (original_path), not the
/// rag/files copy (there is none). For "copy"/legacy docs it reveals the copied
/// file under rag/files as before. Returns an error if the resolved target no
/// longer exists on disk (e.g. a symlink whose source was moved/deleted).
pub async fn open_file_location(app: &AppHandle, id: &str, target: Option<String>) -> Result<()> {
    let dir = files_dir(app)?;
    let meta = std::fs::read(dir.join(format!("{}.meta", id)))
        .ok()
        .and_then(|b| serde_json::from_slice::<DocMeta>(&b).ok())
        .ok_or_else(|| anyhow!("document not found: {}", id))?;
    let is_symlink = meta.method.as_deref() == Some("symlink");
    // target="source": explicitly reveal the ORIGINAL file (second-opinion
    // view for extractable imports). Default: symlink docs -> original,
    // copy docs -> the rag/files copy / extracted Markdown.
    let want_source = target.as_deref() == Some("source");
    let (resolved, is_source) = if want_source || is_symlink {
        let op = meta
            .original_path
            .as_deref()
            .filter(|p| !p.is_empty())
            .ok_or_else(|| anyhow!("no original path recorded for this document"))?;
        (PathBuf::from(op), true)
    } else {
        (content_path_for(&dir, id, &meta.name), false)
    };
    if !resolved.exists() {
        return Err(if is_source {
            anyhow!("original file does not exist: {}", resolved.display())
        } else {
            anyhow!("file not found: {}", resolved.display())
        });
    }
    reveal_in_file_manager(&resolved)?;
    Ok(())
}

/// Open a doc's ORIGINAL source file with the OS default application
/// ("view source file" button in the View dialog — PDF/Office/image sources
/// are stored as extracted Markdown, this re-opens the real file).
pub async fn open_doc_source(app: &AppHandle, id: &str) -> Result<()> {
    let dir = files_dir(app)?;
    let meta = std::fs::read(dir.join(format!("{}.meta", id)))
        .ok()
        .and_then(|b| serde_json::from_slice::<DocMeta>(&b).ok())
        .ok_or_else(|| anyhow!("document not found: {}", id))?;
    let op = meta
        .original_path
        .as_deref()
        .filter(|p| !p.is_empty())
        .ok_or_else(|| anyhow!("no original path recorded for this document"))?;
    let file = PathBuf::from(op);
    if !file.exists() {
        return Err(anyhow!("original file does not exist: {}", file.display()));
    }
    open_file_with_default_app(&file)?;
    Ok(())
}

/// Open `file` with the OS default application (view the file itself, not
/// its parent folder).
fn open_file_with_default_app(file: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(file).spawn()?.wait()?;
    }
    #[cfg(target_os = "windows")]
    {
        // explorer <file> opens it with the default associated application.
        std::process::Command::new("explorer").arg(file.as_os_str()).spawn()?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(file).spawn()?.wait()?;
    }
    Ok(())
}

// ── import-method update helpers ───────────────────────────────────────────

/// Single-doc update check: classify the recorded original's state so the
/// per-row UpdateDialog can render the right branch (lost / changed / no-
/// change / legacy-no-path). Reads `.meta` + stats the source file. Cheap
/// (one md5 of the source, no embedding).
pub async fn check_rag_update(app: &AppHandle, id: &str) -> Result<RagUpdateCheck> {
    let dir = files_dir(app)?;
    let meta_bytes = std::fs::read(dir.join(format!("{}.meta", id)))
        .map_err(|e| anyhow!("check: read meta {} failed: {}", id, e))?;
    let meta: DocMeta = serde_json::from_slice(&meta_bytes)?;
    Ok(classify_original(&meta))
}

/// Batch-update preview: classify every doc and return the aggregate counts
/// the confirm dialog shows before the expensive re-index pass runs. No
/// embedding, no progress events — just a fast filesystem scan.
pub async fn preview_batch_update(app: &AppHandle) -> Result<BatchPreview> {
    let dir = files_dir(app)?;
    if !dir.exists() {
        return Ok(BatchPreview {
            total: 0,
            to_update: 0,
            skipped: 0,
            lost: 0,
        });
    }
    let mut total = 0u32;
    let mut to_update = 0u32;
    let mut skipped = 0u32;
    let mut lost = 0u32;
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("meta") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let Ok(meta) = serde_json::from_slice::<DocMeta>(&bytes) else { continue };
        total += 1;
        let c = classify_original(&meta);
        if c.lost_original {
            lost += 1;
        } else if !c.has_original_path {
            // legacy doc with no recorded source — can't auto-update, left to
            // single-doc manual upload (which records a new original_path).
            skipped += 1;
        } else if c.original_changed {
            to_update += 1;
        } else {
            skipped += 1;
        }
    }
    Ok(BatchPreview {
        total,
        to_update,
        skipped,
        lost,
    })
}

/// Run the batch update in the background: re-index every doc whose source
/// changed (md5 differs, or legacy no-md5 treated as changed). Emits
/// `rag://batch-update-progress` per doc (checking/reindexing phases); the
/// char-level sub-bar comes from reindex_doc's existing `rag://upload-progress`
/// events. Lost/legacy docs are skipped. Guarded by `BATCH_UPDATE_RUNNING` so a
/// second trigger while one is in flight is a no-op (the frontend re-opens the
/// dialog instead). Returns immediately; the work runs on a spawned task.
pub fn batch_update_rag_docs(app: AppHandle) {
    // CAS guard: if already running, do nothing.
    if BATCH_UPDATE_RUNNING
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .is_err()
    {
        rag_log("info", "batch_update: already running, ignoring re-trigger");
        return;
    }
    tauri::async_runtime::spawn(async move {
        // RAII guard: whether run_batch_update returns Ok/Err OR panics, the
        // CAS flag is cleared on drop so a second batch can run later. Without
        // this, a panic mid-batch would leave BATCH_UPDATE_RUNNING=true forever
        // (the store(false) below ran in the happy path only), bricking batch
        // update until app restart.
        struct RunningGuard;
        impl Drop for RunningGuard {
            fn drop(&mut self) {
                BATCH_UPDATE_RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let _guard = RunningGuard;
        let result = run_batch_update(&app).await;
        if let Err(e) = result {
            // On early-? failure (e.g. files_dir / read_dir error) the done
            // event was NOT emitted inside run_batch_update, so the frontend's
            // batchUpdateRunning would stick on "查看进度" forever. Emit an
            // "error" phase (NOT "done" — that would show a green checkmark
            // "批量更新完成" for a failed run) so the frontend clears its
            // running flag AND shows the failure state.
            rag_log("error", format!("batch_update failed: {:#}", e));
            emit_batch_update_progress(&app, 0, 0, "", "error");
        }
        // _guard drops here -> BATCH_UPDATE_RUNNING = false.
    });
}

async fn run_batch_update(app: &AppHandle) -> Result<()> {
    let dir = files_dir(app)?;
    if !dir.exists() {
        emit_batch_update_progress(app, 0, 0, "", "done");
        return Ok(());
    }
    // Collect metas first so the total is known up front. Best-effort: skip
    // unreadable entries instead of failing the whole batch (a single corrupt
    // .meta shouldn't brick batch update for all other docs).
    let mut docs: Vec<DocMeta> = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("meta") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let Ok(meta) = serde_json::from_slice::<DocMeta>(&bytes) else { continue };
        docs.push(meta);
    }
    let total = docs.len() as u32;
    rag_log("info", format!("batch_update: scanning {} docs", total));
    for (i, meta) in docs.iter().enumerate() {
        let c = classify_original(meta);
        emit_batch_update_progress(app, i as u32, total, &meta.name, "checking");
        // Lost original or legacy-no-path -> skip (manual-upload-only).
        if c.lost_original || !c.has_original_path {
            continue;
        }
        if !c.original_changed {
            continue;
        }
        emit_batch_update_progress(app, i as u32, total, &meta.name, "reindexing");
        if let Err(e) = update_doc_from_original(app, &meta.id).await {
            rag_log("warn", format!("batch_update: '{}' failed: {:#}", meta.name, e));
        }
    }
    emit_batch_update_progress(app, total, total, "", "done");
    if let Err(e) = rebuild_rag_sql_index(app).await {
        rag_log("warn", format!("batch_update: rebuild_rag_sql_index failed: {}", e));
    }
    rag_log("info", format!("batch_update: done ({} docs)", total));
    Ok(())
}

// ── auto (timed) doc update ─────────────────────────────────────────────────

/// Auto-update timer generation. Bumped whenever the settings change (interval
/// or enable/disable) so a running timer loop wakes, notices the generation
/// moved, and restarts with the new cadence. This is how interval changes take
/// effect without an app restart.
static AUTO_UPDATE_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Re-arm the auto-update timer after settings changed: bump the generation so
/// any running loop exits, then (if enabled + RAG runtime available) spawn a
/// fresh loop with the new interval. Called from `save_settings` (user edited
/// the settings dialog) and at app startup / RAG enable.
///
/// The timer only fires while RAG is enabled (re-index needs the embedding
/// runtime); each tick re-checks `is_enabled()` so toggling RAG off mid-wait
/// simply skips ticks without restarting the loop.
pub fn restart_auto_update_timer(app: &AppHandle) {
    let gen = AUTO_UPDATE_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    // Clone the AppHandle (an Arc — cheap) so the spawned loop owns 'static
    // data.
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            // Read the CURRENT settings every iteration so an interval change
            // is picked up on the next wake even without a generation bump.
            let settings = get_settings().await.unwrap_or_default();
            if !settings.auto_update_enabled {
                // Disabled: stop this loop. save_settings bumps the generation
                // + re-calls restart_auto_update_timer on re-enable.
                rag_log("info", "auto-update: disabled, timer stopped");
                return;
            }
            // Sleep in short slices and watch for a generation change so a
            // settings edit (interval change / disable) takes effect promptly
            // instead of after the full old interval.
            let total = std::time::Duration::from_secs(settings.auto_update_interval_secs);
            let slice = std::time::Duration::from_millis(500);
            let mut waited = std::time::Duration::ZERO;
            let mut interrupted = false;
            while waited < total {
                tokio::time::sleep(slice.min(total - waited)).await;
                waited += slice.min(total - waited);
                let cur = AUTO_UPDATE_GEN.load(std::sync::atomic::Ordering::SeqCst);
                if cur != gen {
                    // A newer loop was armed (settings changed) — this one is
                    // obsolete. The new loop re-reads settings itself.
                    interrupted = true;
                    break;
                }
            }
            if interrupted {
                return;
            }
            // Tick: only run while RAG is enabled (re-index needs the runtime).
            // Skip silently otherwise — the doc list is still readable, and
            // the next enable re-arms the timer (start() calls this fn).
            if !is_enabled() {
                continue;
            }
            // Guard against overlap with a manual batch run + a still-running
            // previous tick (BATCH_UPDATE_RUNNING is the same CAS guard
            // batch_update_rag_docs uses, so auto/manual mutually exclude).
            if BATCH_UPDATE_RUNNING
                .compare_exchange(
                    false,
                    true,
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                )
                .is_err()
            {
                continue; // a manual (or previous auto) pass is in flight — skip this tick
            }
            struct RunningGuard;
            impl Drop for RunningGuard {
                fn drop(&mut self) {
                    BATCH_UPDATE_RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
                }
            }
            let _guard = RunningGuard;
            // Pre-scan (same md5 classification the confirm dialog shows). If
            // nothing changed, release the guard and DO NOT emit any progress
            // events — an idle tick must not flash the header button to
            // "查看进度" or pop anything in the UI.
            if let Ok(preview) = preview_batch_update(&app).await {
                if preview.to_update == 0 {
                    continue; // _guard drops -> BATCH_UPDATE_RUNNING = false
                }
                rag_log(
                    "info",
                    format!(
                        "auto-update: {} of {} doc(s) changed, re-indexing…",
                        preview.to_update, preview.total
                    ),
                );
            }
            // Real work: run the same pass as the manual button — same
            // progress events, same button state, same dialog.
            let result = run_batch_update(&app).await;
            if let Err(e) = result {
                rag_log("error", format!("auto-update pass failed: {:#}", e));
            }
            // _guard drops here -> BATCH_UPDATE_RUNNING = false.
        }
    });
}

// ── search ─────────────────────────────────────────────────────────────────

/// Hybrid search: vector nearest-neighbor + keyword (term) matching, merged
/// with the weights from settings (`vectorWeight`, `keywordWeight`). The
/// weights are read live from config on every call, so changing them in the
/// Search Settings dialog takes effect on the next search.
pub async fn search(query: String, tags: Vec<String>) -> Result<Vec<RagSearchResult>> {
    let started = std::time::Instant::now();
    let settings = get_settings().await?;
    let limit = settings.max_results.max(1) as usize;
    let vw = settings.vector_weight.max(0.0).min(1.0);
    let kw = settings.keyword_weight.max(0.0).min(1.0);

    let mut guard = runtime().lock().await;
    let Some(rt) = guard.as_mut() else {
        return Err(anyhow!("RAG not enabled"));
    };

    // When a tag filter is active, fetch more candidates so Rust-side filtering
    // (intersection with requested tags) still yields enough hits after pruning.
    let want_tags: Vec<String> = tags.into_iter().filter(|t| !t.is_empty()).collect();
    let fetch = if want_tags.is_empty() { (limit * 2).max(limit) } else { (limit * 4).max(limit) };

    // Vector channel.
    let vec_hits = if vw > 0.0 {
        // Prepend the model's query prefix (deploy.json `searchQueryPrefix`)
        // before embedding - asymmetric models (Qwen3, BGE) require a distinct
        // prefix on the query side. The keyword channel below uses the RAW
        // query (no prefix) since it's literal text matching.
        let q = format!("{}{}", &rt.search_query_prefix, query);
        let qvec = rt.model.embed(&q)?;
        match rt.db.search(&qvec, fetch).await {
            Ok(hits) => hits,
            Err(e) => {
                rag_log("warn", format!("vector search failed: {:#}", e));
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    // Keyword channel.
    let kw_hits = if kw > 0.0 {
        match rt.db.keyword_search(&query, fetch).await {
            Ok(hits) => hits,
            Err(e) => {
                rag_log("warn", format!("keyword search failed: {:#}", e));
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    let terms: Vec<String> = query
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect();
    let term_count = terms.len().max(1);

    // Merge by (doc_id, chunk_index): vec_score = cosine similarity (the search
    // uses cosine distance = 1 - cos, so 1 - distance = cos, clamped to [0,1] -
    // 0 = unrelated, 1 = identical); kw_score = matched_query_terms /
    // total_query_terms. Carry the doc's tags (all chunks of a doc share tags).
    use std::collections::HashMap;
    let mut merged: HashMap<(String, i64), (f32, f32, String, String, Vec<String>)> = HashMap::new();
    for h in vec_hits {
        let vs = (1.0 - h.distance).clamp(0.0, 1.0);
        let e = merged
            .entry((h.doc_id.clone(), h.chunk_index))
            .or_insert((0.0, 0.0, h.doc_name.clone(), h.chunk_text.clone(), h.tags.clone()));
        e.0 = vs;
    }
    for h in kw_hits {
        let lower = h.chunk_text.to_lowercase();
        let matched = terms.iter().filter(|t| lower.contains(t.as_str())).count();
        let ks = (matched as f32) / (term_count as f32);
        let e = merged
            .entry((h.doc_id.clone(), h.chunk_index))
            .or_insert((0.0, 0.0, h.doc_name.clone(), h.chunk_text.clone(), h.tags.clone()));
        e.1 = ks;
    }

    // Weighted final score, apply tag filter + score threshold, sort desc, take limit.
    let threshold = settings.score_threshold.max(0.0).min(1.0);
    let mut scored: Vec<(f32, String, String, String)> = merged
        .into_iter()
        .filter(|(_, (_, _, _, _, doc_tags))| {
            if want_tags.is_empty() {
                true
            } else {
                doc_tags.iter().any(|t| want_tags.iter().any(|w| w.eq_ignore_ascii_case(t)))
            }
        })
        .map(|((doc_id, _ci), (vs, ks, doc_name, chunk_text, _tags))| {
            (vw * vs + kw * ks, doc_id, doc_name, chunk_text)
        })
        .filter(|(score, _, _, _)| *score >= threshold)
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let top = scored.into_iter().take(limit).collect::<Vec<_>>();

    // Resolve a readable title per unique doc_id from the on-disk DocMeta
    // (falls back to the filename when meta is missing or has no title).
    let mut titles: HashMap<String, String> = HashMap::new();
    if let Some(app) = crate::mcp::progress::get_app_handle() {
        if let Ok(dir) = files_dir(app) {
            for (_, doc_id, doc_name, _) in &top {
                if titles.contains_key(doc_id) {
                    continue;
                }
                let title = std::fs::read(dir.join(format!("{}.meta", doc_id)))
                    .ok()
                    .and_then(|b| serde_json::from_slice::<DocMeta>(&b).ok())
                    .and_then(|m| m.title.filter(|t| !t.is_empty()))
                    .unwrap_or_else(|| doc_name.clone());
                titles.insert(doc_id.clone(), title);
            }
        }
    }

    let results = top
        .into_iter()
        .map(|(score, doc_id, doc_name, snippet)| RagSearchResult {
            title: titles.get(&doc_id).cloned().unwrap_or(doc_name.clone()),
            doc_id,
            doc_name,
            snippet,
            score,
        })
        .collect::<Vec<_>>();
    rag_log(
        "info",
        format!(
            "search query='{}' tags={} -> {} hits ({}ms)",
            query,
            want_tags.len(),
            results.len(),
            started.elapsed().as_millis()
        ),
    );
    Ok(results)
}

// ── settings ───────────────────────────────────────────────────────────────

pub async fn get_settings() -> Result<RagSettings> {
    let cfg = crate::services::config_service::get().await?;
    let rag = cfg.get("rag").cloned().unwrap_or_else(|| json!({}));
    // Defaults come from `RagSettings::default()` — the same struct the serde
    // `default = ...` fns use — so there's one source of truth for the weights
    // (0.9/0.1), score_threshold (0.65), and chunk_size/overlap (0 = auto). A
    // persisted config keeps its stored values; the defaults only fill keys
    // that aren't present.
    let d = RagSettings::default();
    Ok(RagSettings {
        vector_weight: rag
            .get("vectorWeight")
            .and_then(|v| v.as_f64())
            .map(|v| v as f32)
            .unwrap_or(d.vector_weight),
        keyword_weight: rag
            .get("keywordWeight")
            .and_then(|v| v.as_f64())
            .map(|v| v as f32)
            .unwrap_or(d.keyword_weight),
        max_results: rag
            .get("maxResults")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .unwrap_or(d.max_results),
        score_threshold: rag
            .get("scoreThreshold")
            .and_then(|v| v.as_f64())
            .map(|v| v as f32)
            .unwrap_or(d.score_threshold),
        // 0 = "auto": the effective chunk size is resolved per-loaded-model in
        // `reindex_doc` from the model's deploy.json (capped by max_context).
        chunk_size: rag
            .get("chunkSize")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .unwrap_or(d.chunk_size),
        chunk_overlap: rag
            .get("chunkOverlap")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .unwrap_or(d.chunk_overlap),
        auto_update_enabled: rag
            .get("autoUpdateEnabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(d.auto_update_enabled),
        // Clamp the interval so a bad persisted value can't busy-loop the
        // auto-update timer (min 1 minute, max 1 day).
        auto_update_interval_secs: rag
            .get("autoUpdateIntervalSecs")
            .and_then(|v| v.as_u64())
            .unwrap_or(d.auto_update_interval_secs)
            .clamp(60, 86_400),
        // Clamp the doc-detail page size (min 10 KiB, max 64 MiB = the upload
        // cap) so a bad value can't crash the View dialog or underflow to 0.
        doc_load_chunk_kb: rag
            .get("docLoadChunkKb")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .unwrap_or(d.doc_load_chunk_kb)
            .clamp(10, 65_536),
    })
}

pub async fn save_settings(settings: RagSettings) -> Result<()> {
    // Same clamps as get_settings so a round-trip normalizes bad values.
    let interval = settings.auto_update_interval_secs.clamp(60, 86_400);
    let chunk_kb = settings.doc_load_chunk_kb.clamp(10, 65_536);
    let patch = json!({
        "rag": {
            "vectorWeight": settings.vector_weight as f64,
            "keywordWeight": settings.keyword_weight as f64,
            "maxResults": settings.max_results,
            "scoreThreshold": settings.score_threshold as f64,
            "chunkSize": settings.chunk_size,
            "chunkOverlap": settings.chunk_overlap,
            "autoUpdateEnabled": settings.auto_update_enabled,
            "autoUpdateIntervalSecs": interval,
            "docLoadChunkKb": chunk_kb
        }
    });
    crate::services::config_service::update(&patch).await?;
    Ok(())
}

/// Save settings + re-arm the auto-update timer (called by the Tauri command
/// so an interval change / toggle takes effect immediately without a restart).
pub async fn save_settings_and_rearm(app: &AppHandle, settings: RagSettings) -> Result<()> {
    save_settings(settings).await?;
    restart_auto_update_timer(app);
    Ok(())
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// Sniff whether `bytes` look like plain text (vs binary). Content-based — we
/// don't trust the file extension (text extensions can't be exhaustively
/// enumerated). Heuristic (same idea as the `file` utility):
/// - A NUL byte (0x00) in the first 8 KiB -> binary (PDF/Word/Excel/ZIP/EXE
///   all contain NULs). Text files never do.
/// - Otherwise, if >30% of the sample is non-text control bytes -> binary.
/// ASCII/UTF-8/legacy-CJK text passes (printable ASCII, tab/LF/CR, or high
/// bytes for multibyte/extended chars are all "text").
/// `pub(crate)`: also used by the `text` fallback strategy in
/// `rag::extract::text`.
pub(crate) fn is_likely_text(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(8192)];
    if sample.is_empty() {
        return true; // empty file -> treat as text
    }
    let mut non_text = 0usize;
    for &b in sample {
        if b == 0 {
            return false; // NUL -> binary
        }
        // text bytes: TAB(9) LF(10) CR(13), printable ASCII (32..=126),
        // or high byte (>=128, valid in UTF-8 multibyte / legacy CJK).
        if !(b == 9 || b == 10 || b == 13 || (32..=126).contains(&b) || b >= 128) {
            non_text += 1;
        }
    }
    (non_text as f64) / (sample.len() as f64) < 0.30
}

/// Display-label catalog compiled in from `runtimes/rag/file_support.json`
/// (extension -> human-readable name, e.g. ".md" -> "Markdown"). Display-only —
/// does NOT gate upload (validation is content-based via `is_likely_text`).
static FILE_TYPE_MAP: OnceLock<std::collections::HashMap<String, String>> = OnceLock::new();

fn file_type_map() -> &'static std::collections::HashMap<String, String> {
    FILE_TYPE_MAP.get_or_init(|| {
        let raw = include_str!("../../runtimes/rag/file_support.json");
        let mut map = std::collections::HashMap::new();
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
            if let Some(obj) = v.as_object() {
                for (ext, val) in obj {
                    if let Some(name) = val.get("name").and_then(|n| n.as_str()) {
                        map.insert(ext.to_lowercase(), name.to_string());
                    }
                }
            }
        }
        map
    })
}

/// Look up a display label for `filename` by its extension. Returns "" if the
/// extension isn't in the catalog (查不到返回空).
fn file_type_label(filename: &str) -> String {
    let lower = filename.to_lowercase();
    if let Some(dot) = lower.rfind('.') {
        let ext = &lower[dot..]; // includes the dot, e.g. ".md"
        if let Some(name) = file_type_map().get(ext) {
            return name.clone();
        }
    }
    String::new()
}

/// Extract a human-readable title from document content: the first markdown/// H1 (`# ...`), else the first non-empty line, else the filename without
/// extension.
fn extract_title(content: &str, filename: &str) -> String {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(stripped) = trimmed.strip_prefix("# ") {
            return stripped.trim().to_string();
        }
        // first non-empty, non-heading line
        return trimmed.trim_end_matches('#').trim().to_string();
    }
    // fall back to filename without extension
    std::path::Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| filename.to_string())
}

/// Reveal `file` in the OS file manager, highlighting/selecting it so the user
/// can identify it even when the on-disk name differs from the display name
/// (uploads are stored as `dir/{uuid}`, so the file the user uploaded isn't
/// findable by its original name). On macOS/Windows the file is selected; on
/// Linux (no portable "select file" API) we open the containing directory.
#[cfg(target_os = "macos")]
fn reveal_in_file_manager(file: &Path) -> std::io::Result<()> {
    // `open -R` opens the file's parent in Finder with the file selected.
    std::process::Command::new("open")
        .arg("-R")
        .arg(file)
        .spawn()?
        .wait()?;
    Ok(())
}
#[cfg(target_os = "windows")]
fn reveal_in_file_manager(file: &Path) -> std::io::Result<()> {
    // `explorer /select,<path>` opens Explorer with the file selected.
    std::process::Command::new("explorer")
        .arg(format!("/select,{}", file.display()))
        .spawn()?;
    Ok(())
}
#[cfg(all(unix, not(target_os = "macos")))]
fn reveal_in_file_manager(file: &Path) -> std::io::Result<()> {
    // No portable "select file" on Linux; open the containing directory. The
    // file is at least visible there.
    let dir = file.parent().unwrap_or(file);
    std::process::Command::new("xdg-open").arg(dir).spawn()?;
    Ok(())
}
