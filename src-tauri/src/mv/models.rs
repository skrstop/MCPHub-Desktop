//! mv::models — model inventory, selection persistence, download, and path
//! resolution. Moved verbatim from `rag/service.rs` in the Phase 2 shared
//! runtime extraction (paths and behavior unchanged; `mv.model` is the new
//! selection key with `rag.model` as the read fallback for existing installs).

use super::{embedder::{detect_format, read_max_context}, data_dir};
use crate::rag::service::rag_log;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
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
/// The bundled resource dir is read-only in a signed app, so models fetched via
/// `download.url` land here. Mirrors the bundled layout.
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
            let deploy_cfg = crate::mv::embedder::read_deploy_config(&sz_path);
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
        // Download to "<name>.part" and rename on success: an interrupted
        // download (network drop / process kill) must not leave a truncated
        // *.gguf that detect_format() would classify as ready.
        let out_path = target_dir.join(format!("{}.part", out_name));

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

    // Verify the model file landed, then publish all .part files atomically.
    let model_file = "model.gguf";
    if !target_dir.join(format!("{}.part", model_file)).exists() {
        return Err(anyhow!("{} download failed for '{}'", model_file, size));
    }
    for url in &urls {
        let name = if url.as_str() == urls[0] {
            "model.gguf".to_string()
        } else {
            url_basename(url)
        };
        let part = target_dir.join(format!("{}.part", name));
        let finalp = target_dir.join(&name);
        let _ = std::fs::remove_file(&finalp);
        std::fs::rename(&part, &finalp).map_err(|e| {
            anyhow!("publish {} for '{}': {}", name, size, e)
        })?;
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

