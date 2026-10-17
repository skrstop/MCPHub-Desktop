//! mv::models — model inventory, selection persistence, download, and path
//! resolution. Moved verbatim from `rag/service.rs` in the Phase 2 shared
//! runtime extraction (paths and behavior unchanged; `mv.model` is the new
//! selection key with `rag.model` as the read fallback for existing installs).

use super::{embedder::{detect_format, read_max_context}, data_dir};
use crate::rag::service::rag_log;
use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager};

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
/// The bundled resource dir is read-only in a signed app, so models fetched
/// via deploy.json `downloadUrls` land here. Mirrors the bundled layout.
fn download_root(app: &AppHandle) -> Result<PathBuf> {
    Ok(data_dir(app)?.join("models"))
}

/// The out-of-box default ready size: the size whose `deploy.json` has
/// `"default": true` (and is ready), else the first ready size. Also the
/// fallback when the persisted selection is gone (a model deleted in a later
/// version). Sync (scans the model dirs).
pub(crate) fn default_size(app: &AppHandle) -> Option<String> {
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
    let cfg = crate::mv::embedder::read_deploy_config(&d);
    (max_ctx, cfg.chunk_size, cfg.chunk_overlap)
}

/// Sum the on-disk size of the model file in a ready size dir: exactly
/// `model.gguf` (the canonical published name — other `*.gguf` names are not
/// recognized as a downloaded model). 0 when absent.
fn model_file_size(dir: &Path) -> u64 {
    dir.join("model.gguf")
        .metadata()
        .map(|m| m.len())
        .unwrap_or(0)
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
    /// Download source names (deploy.json `downloadUrls` keys, default first).
    /// Empty for ready/unavailable sizes. The frontend shows a source picker
    /// when there is more than one.
    #[serde(default)]
    pub download_sources: Vec<String>,
}

/// Scan `model/<family>/<size>/` and return one entry per size. A size is
/// "ready" iff its size dir (downloaded copy preferred, else the bundled dir)
/// contains exactly `model.gguf` — any other `*.gguf` name (e.g. a
/// user-renamed `model.gguf`) does NOT count as downloaded. "downloadable"
/// when the size's `deploy.json` has a non-empty `downloadUrls` object but no
/// `model.gguf` yet.
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
            let (ready, _format, file_size) = match &ready_dir {
                Some(d) => (true, detect_format(d).to_string(), model_file_size(d)),
                None => (false, String::new(), 0u64),
            };
            let deploy_cfg = crate::mv::embedder::read_deploy_config(&sz_path);
            let downloadable = !deploy_cfg.download_urls.is_empty() && !ready;
            // GGUF is the only backend; downloadable sizes will be gguf once
            // fetched, so badge them immediately.
            let format = if ready || downloadable { "gguf".to_string() } else { String::new() };
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
                download_sources: deploy_cfg
                    .download_urls
                    .iter()
                    .map(|(n, _)| n.clone())
                    .collect(),
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
pub(crate) fn resolve_model_paths(app: &AppHandle, size: &str) -> Result<Option<PathBuf>> {
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

/// The persisted selected model size. Read order: `mv.model` (new key) →
/// `rag.model` (legacy fallback, existing installs) → None.
pub async fn current_model() -> Option<String> {
    let c = crate::services::config_service::get().await.ok()?;
    c.get("mv")
        .and_then(|m| m.get("model"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            c.get("rag")
                .and_then(|r| r.get("model"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
}

/// Persist `mv.model = <size>` (deep-merge). The legacy `rag.model` key is
/// left untouched as a read fallback for downgrades / existing installs.
pub(crate) async fn set_current_model(size: &str) {
    let patch = json!({ "mv": { "model": size } });
    if let Err(e) = crate::services::config_service::update(&patch).await {
        rag_log("warn", format!("failed to persist mv.model: {}", e));
    }
}

/// Validate that `size` resolves to a ready model dir.
pub fn ensure_model_ready(app: &AppHandle, size: &str) -> Result<()> {
    if resolve_model_paths(app, size)?.is_none() {
        return Err(anyhow!(
            "model '{}' is not ready - download it first",
            size
        ));
    }
    Ok(())
}

/// Persist the selection (mv.model) and log it. Reload orchestration (restart
/// running consumers) belongs to the calling consumer lifecycle, not here.
pub async fn persist_selection(size: &str) {
    set_current_model(size).await;
    rag_log("info", format!("selected model size '{}'", size));
}

/// The model's default retrieval params from its bundled size dir's
/// `deploy.json`: `(score_threshold, vector_weight, keyword_weight)`.
/// Falls back to `(0.0, 0.5, 0.5)` when the size dir / fields are missing.
pub fn model_defaults(app: &AppHandle, size: &str) -> (f32, f32, f32) {
    let dir = resolve_model_paths(app, size).ok().flatten();
    match dir {
        Some(d) => {
            let cfg = crate::mv::embedder::read_deploy_config(&d);
            (cfg.default_score, cfg.default_weight_vector, cfg.default_weight_keyword)
        }
        None => (0.0, 0.5, 0.5),
    }
}


// ── model download ──────────────────────────────────────────────────────────

/// Progress for a model download, emitted on `rag://model-download` so the
/// dropdown's download item can show a rich progress bar. `phase` is
/// "downloading" | "done" | "error" | "cancelled". The bar shows total %,
/// speed (B/s), ETA (seconds), and the current file name.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RagModelDownloadProgress {
    pub size: String,
    phase: String,
    /// Bytes downloaded so far.
    downloaded: u64,
    /// Total bytes (0 if unknown — server sent no content-length).
    total: u64,
    /// 0..100 (best-effort; 0 when total unknown).
    percent: u8,
    /// Download speed in bytes/sec (sliding-window estimate; 0 at the start).
    speed: u64,
    /// Estimated seconds remaining (0 if unknown / just started).
    eta: u64,
    /// Human message (current source name / "done").
    message: Option<String>,
}

const MODEL_DOWNLOAD_EVENT: &str = "rag://model-download";
/// Canonical published model file name. A size is "ready" ONLY when this
/// exact file exists; anything else (e.g. a user-renamed `model.gguf`) is
/// treated as not downloaded.
const MODEL_FILE: &str = "model.gguf";
/// In-flight temp name: the download streams into `<MODEL_FILE>.downloadtemp`
/// and renames to `model.gguf` ONLY on success, so an interrupted download
/// can never leave a truncated file that would be mistaken for a ready model.
const MODEL_TEMP_SUFFIX: &str = ".downloadtemp";

fn emit_model_download(app: &AppHandle, p: &RagModelDownloadProgress) {
    if let Err(e) = app.emit(MODEL_DOWNLOAD_EVENT, p) {
        log::warn!("[RAG] emit model-download failed: {e}");
    }
}

/// In-memory download state: last progress snapshot per size (survives the
/// settings page being unmounted/remounted — the frontend re-queries it via
/// `rag_model_download_status`) + a cancel flag the streaming loop polls.
static DOWNLOAD_STATE: std::sync::Mutex<Option<DownloadState>> = std::sync::Mutex::new(None);

struct DownloadState {
    progress: Option<RagModelDownloadProgress>,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// When the download reached phase "done" (for the 10s queryable window).
    done_at: Option<std::time::Instant>,
    /// The background task's join handle — aborted on cancel so a download
    /// stalled on a dead connection (no chunks flowing) still stops at once.
    task: Option<tauri::async_runtime::JoinHandle<()>>,
    /// The in-flight temp file path — removed on cancel (an aborted task
    /// cannot run its own cleanup).
    temp_path: Option<PathBuf>,
    /// AppHandle clone used to emit the final "cancelled" event when the task
    /// was aborted (the task itself can no longer emit).
    emitter: Option<AppHandle>,
}

fn state_lock() -> std::sync::MutexGuard<'static, Option<DownloadState>> {
    match DOWNLOAD_STATE.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// Locate the bundled size dir for `size` and read its `downloadUrls`.
/// Returns `(family, size_dir, ordered sources)`.
fn find_download_source(
    app: &AppHandle,
    size: &str,
) -> Result<(String, PathBuf, Vec<(String, String)>)> {
    let root = model_root(app)?;
    for fam in std::fs::read_dir(&root)? {
        let fam_path = fam?.path();
        if !fam_path.is_dir() {
            continue;
        }
        let size_dir = fam_path.join(size);
        if !size_dir.is_dir() {
            continue;
        }
        let family = fam_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let cfg = crate::mv::embedder::read_deploy_config(&size_dir);
        if cfg.download_urls.is_empty() {
            return Err(anyhow!(
                "model '{}' has no downloadUrls in deploy.json",
                size
            ));
        }
        return Ok((family, size_dir, cfg.download_urls));
    }
    Err(anyhow!("model size '{}' not found", size))
}

/// Start a BACKGROUND download of `size`'s model file from the FIRST source
/// in its deploy.json `downloadUrls` (remaining entries are fallbacks tried
/// in order on network failure). Streams into
/// `<app_data>/rag/models/<family>/<size>/model.gguf.downloadtemp` and
/// renames to `model.gguf` only on success. Returns immediately; progress is
/// emitted on `rag://model-download` and queryable via `download_status`.
/// Re-downloading an already-ready size is allowed (修复损坏文件的重试路径).
pub fn download_model(app: &AppHandle, size: &str, source: Option<&str>) -> Result<()> {
    let (family, _size_dir, mut sources) = find_download_source(app, size)?;
    // Optional explicit source selection (frontend source picker); unknown
    // names fall back to the default (first) source.
    if let Some(want) = source {
        if let Some(idx) = sources.iter().position(|(n, _)| n == want) {
            if idx > 0 {
                let picked = sources.remove(idx);
                sources.insert(0, picked);
            }
        }
    }
    {
        let mut st = state_lock();
        if let Some(DownloadState {
            progress: Some(p), ..
        }) = st.as_ref()
        {
            if p.size == size && p.phase == "downloading" {
                return Err(anyhow!("model '{}' is already downloading", size));
            }
        }
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let temp_path = download_root(app)?
            .join(&family)
            .join(size)
            .join(format!("{MODEL_FILE}{MODEL_TEMP_SUFFIX}"));
        let mut state = DownloadState {
            done_at: None,
            temp_path: Some(temp_path),
            emitter: Some(app.clone()),
            task: None,
            cancel: Some(cancel.clone()),
            progress: Some(RagModelDownloadProgress {
                size: size.to_string(),
                phase: "downloading".into(),
                downloaded: 0,
                total: 0,
                percent: 0,
                speed: 0,
                eta: 0,
                message: sources.first().map(|(n, _)| n.clone()),
            }),
        };
        let app2 = app.clone();
        let size2 = size.to_string();
        state.task = Some(tauri::async_runtime::spawn(async move {
            if let Err(e) = run_download(app2, size2, family, sources, cancel).await {
                log::warn!("[RAG] model download ended: {e}");
            }
        }));
        *st = Some(state);
    }
    rag_log("info", format!("model '{}' download started (background)", size));
    Ok(())
}

/// The background download body. Emits progress on
/// `rag://model-download` throughout and leaves the final phase
/// ("done" | "error" | "cancelled") in the in-memory state.
async fn run_download(
    app: AppHandle,
    size: String,
    family: String,
    sources: Vec<(String, String)>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    use futures_util::StreamExt;
    use std::sync::atomic::Ordering;
    use tokio::io::AsyncWriteExt;

    let target_dir = download_root(&app)?.join(&family).join(&size);
    tokio::fs::create_dir_all(&target_dir)
        .await
        .map_err(|e| anyhow!("create {}: {}", target_dir.display(), e))?;
    let temp_path = target_dir.join(format!("{MODEL_FILE}{MODEL_TEMP_SUFFIX}"));
    let final_path = target_dir.join(MODEL_FILE);

    let set_phase = |phase: &str, msg: Option<String>| {
        let mut st = state_lock();
        if let Some(DownloadState { progress: Some(p), .. }) = st.as_mut() {
            if p.size == size {
                p.phase = phase.to_string();
                p.message = msg;
            }
        }
    };
    let emit_current = |app: &AppHandle| {
        let snapshot = {
            let st = state_lock();
            match st.as_ref() {
                Some(DownloadState {
                    progress: Some(p), ..
                }) if p.size == size => Some(p.clone()),
                _ => None,
            }
        };
        if let Some(p) = snapshot {
            emit_model_download(app, &p);
        }
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3600))
        .build()
        .map_err(|e| anyhow!("http client: {}", e))?;

    let mut last_err: Option<anyhow::Error> = None;
    'sources: for (source_name, url) in &sources {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        {
            let mut st = state_lock();
            if let Some(DownloadState { progress: Some(p), .. }) = st.as_mut() {
                if p.size == size {
                    p.message = Some(source_name.clone());
                    p.downloaded = 0;
                    p.total = 0;
                    p.percent = 0;
                    p.speed = 0;
                    p.eta = 0;
                }
            }
        }
        emit_current(&app);

        // HEAD first to learn the total size (for % + ETA); tolerated to fail.
        let total = client
            .head(url.as_str())
            .send()
            .await
            .ok()
            .and_then(|r| r.error_for_status().ok())
            .and_then(|r| r.content_length())
            .unwrap_or(0);
        {
            let mut st = state_lock();
            if let Some(DownloadState { progress: Some(p), .. }) = st.as_mut() {
                if p.size == size {
                    p.total = total;
                }
            }
        }

        let resp = match client.get(url.as_str()).send().await {
            Ok(r) => match r.error_for_status() {
                Ok(r) => r,
                Err(e) => {
                    last_err = Some(anyhow!("{} status: {}", source_name, e));
                    continue 'sources;
                }
            },
            Err(e) => {
                last_err = Some(anyhow!("{} request: {}", source_name, e));
                continue 'sources;
            }
        };

        let mut file = match tokio::fs::File::create(&temp_path).await {
            Ok(f) => f,
            Err(e) => {
                last_err = Some(anyhow!("create {}: {}", temp_path.display(), e));
                break 'sources;
            }
        };
        let mut stream = resp.bytes_stream();
        let mut downloaded: u64 = 0;
        let mut last_emit = std::time::Instant::now();
        let mut since_emit: u64 = 0;
        let result: Result<()> = async {
            while let Some(chunk) = stream.next().await {
                if cancel.load(Ordering::Relaxed) {
                    return Err(anyhow!("cancelled"));
                }
                let chunk = chunk.map_err(|e| anyhow!("{} stream: {}", source_name, e))?;
                file.write_all(&chunk)
                    .await
                    .map_err(|e| anyhow!("write {}: {}", MODEL_FILE, e))?;
                let n = chunk.len() as u64;
                downloaded = downloaded.saturating_add(n);
                since_emit = since_emit.saturating_add(n);
                if last_emit.elapsed() >= std::time::Duration::from_millis(200) {
                    let elapsed_secs = last_emit.elapsed().as_secs_f64().max(0.001);
                    let speed = (since_emit as f64 / elapsed_secs) as u64;
                    let eta = if speed > 0 && total > downloaded {
                        (total - downloaded) / speed
                    } else {
                        0
                    };
                    {
                        let mut st = state_lock();
                        if let Some(DownloadState { progress: Some(p), .. }) = st.as_mut() {
                            if p.size == size {
                                p.downloaded = downloaded;
                                p.speed = speed;
                                p.eta = eta;
                                p.percent = pct(downloaded, total);
                            }
                        }
                    }
                    emit_current(&app);
                    last_emit = std::time::Instant::now();
                    since_emit = 0;
                }
            }
            file.flush()
                .await
                .map_err(|e| anyhow!("flush {}: {}", MODEL_FILE, e))?;
            Ok(())
        }
        .await;
        match result {
            Ok(()) => {
                // Publish atomically: rename temp -> model.gguf ONLY after the
                // full body landed, so an interrupted download never leaves a
                // truncated file that detect_format would call ready.
                let _ = std::fs::remove_file(&final_path);
                match tokio::fs::rename(&temp_path, &final_path).await {
                    Ok(()) => {
                        set_phase("done", Some("done".into()));
                        {
                            let mut st = state_lock();
                            if let Some(ds) = st.as_mut() {
                                ds.done_at = Some(std::time::Instant::now());
                            }
                        }
                        {
                            let mut st = state_lock();
                            if let Some(DownloadState { progress: Some(p), .. }) = st.as_mut() {
                                if p.size == size {
                                    p.downloaded = downloaded;
                                    p.total = total.max(downloaded);
                                    p.percent = 100;
                                    p.speed = 0;
                                    p.eta = 0;
                                }
                            }
                        }
                        emit_current(&app);
                        rag_log(
                            "info",
                            format!(
                                "model '{}' downloaded from '{}' ({} bytes)",
                                size, source_name, downloaded
                            ),
                        );
                        return Ok(());
                    }
                    Err(e) => {
                        last_err = Some(anyhow!("publish {}: {}", MODEL_FILE, e));
                        break 'sources;
                    }
                }
            }
            Err(e) => {
                if cancel.load(Ordering::Relaxed) {
                    break 'sources;
                }
                last_err = Some(e);
                // fall through to the next source
            }
        }
    }

    // Failure / cancellation: clean the temp file so no partial model lingers.
    let _ = tokio::fs::remove_file(&temp_path).await;
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        set_phase("cancelled", None);
        emit_current(&app);
        rag_log("info", format!("model '{}' download cancelled", size));
        return Ok(());
    }
    let err = last_err.unwrap_or_else(|| anyhow!("download failed"));
    set_phase("error", Some(err.to_string()));
    emit_current(&app);
    rag_log("error", format!("model '{}' download failed: {}", size, err));
    Err(err)
}

/// The in-memory progress snapshot for `size`'s download (None when no
/// download has run this session). Lets the settings page restore the progress
/// bar after being unmounted/remounted. A terminal "done" snapshot stays
/// queryable for 10s (covering slow remounts), then reports None so a page
/// revisited much later doesn't show a phantom finished bar.
pub fn download_status(size: &str) -> Option<RagModelDownloadProgress> {
    let st = state_lock();
    match st.as_ref() {
        Some(DownloadState {
            progress,
            done_at,
            ..
        }) => {
            let p = progress.clone().filter(|p| p.size == size)?;
            if p.phase == "done"
                && done_at
                    .map(|t| t.elapsed() > std::time::Duration::from_secs(10))
                    .unwrap_or(true)
            {
                return None;
            }
            Some(p)
        }
        _ => None,
    }
}

/// Request cancellation of a running background download for `size`. Sets the
/// cooperative flag (the streaming loop checks it between chunks) AND aborts
/// the task outright — a download stalled on a dead connection (no chunks
/// flowing) must still stop immediately. The temp file is removed here too,
/// since an aborted task cannot run its own cleanup, and the final
/// "cancelled" event is emitted from this thread.
pub fn cancel_download(size: &str) -> bool {
    let mut st = state_lock();
    if let Some(DownloadState {
        progress: Some(p),
        cancel,
        task,
        temp_path,
        emitter,
        ..
    }) = st.as_mut()
    {
        if p.size == size && p.phase == "downloading" {
            if let Some(flag) = cancel {
                use std::sync::atomic::Ordering;
                flag.store(true, Ordering::Relaxed);
            }
            if let Some(h) = task.take() {
                h.abort();
            }
            if let Some(tp) = temp_path.take() {
                let _ = std::fs::remove_file(tp);
            }
            p.phase = "cancelled".into();
            p.message = None;
            p.speed = 0;
            p.eta = 0;
            if let Some(app) = emitter {
                emit_model_download(app, p);
            }
            return true;
        }
    }
    false
}

/// 0..100 percent of `done / total`; 0 when total is unknown.
fn pct(done: u64, total: u64) -> u8 {
    if total == 0 {
        0
    } else {
        ((done as f64 / total as f64) * 100.0).min(100.0) as u8
    }
}
