//! Embedder strategy: the GGUF (candle) backend, wrapped so the rest of the
//! RAG service is backend-agnostic.
//!
//! `load_embedder(size_dir)` probes the size directory for a `*.gguf` file and
//! instantiates `GgufEmbedder` (candle, see `gguf.rs`).
//!
//! The backend implements `Embedder`: text -> `embed_dim`-long f32 vector
//! (L2-normalized), with GPU-first + CPU-fallback (candle: Metal on macOS). The
//! trait keeps the service free of backend-specific types - `Runtime` holds a
//! `Box<dyn Embedder>`.
//!
//! Shared, backend-agnostic helpers (memory probes, `read_max_context` from
//! `config.json`) live here so the backend and the service (which needs the
//! memory gate + the UI context bound even with RAG off) can use them without
//! pulling in candle.

use std::path::Path;

use anyhow::{anyhow, Result};

use crate::mv::gguf::GgufEmbedder;

/// A loaded embedding model (GGUF). Produces an `embed_dim`-long L2-normalized
/// f32 vector per input text, exposes its context window (for the chunk-size
/// UI cap), and reports the execution-provider config that took effect (for
/// the enable-time log).
///
/// `embed` / `embed_batch` take `&mut self` (the trait is unified on `&mut` so
/// callers don't branch on backend).
///
/// Per-model deployment platform, read from a size dir's `deploy.json`
/// (`{"platform": "AUTO"|"GPU"|"CPU"}`). Drives device selection: GGUF (candle
/// Metal/CUDA/CPU — though only Metal is built in practice (CUDA is disabled,
/// see Cargo.toml). Missing file / field -> `Auto` (GPU-first, CPU fallback).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Platform {
    /// GPU-first, CPU fallback (the default).
    Auto,
    /// Force GPU (Metal/CUDA). Errors if no GPU is available (no silent CPU
    /// fallback) - the user explicitly asked for GPU.
    Gpu,
    /// Force CPU.
    Cpu,
}

impl Platform {
    /// Parse a platform string (case-insensitive); unknown/AUTO -> Auto.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_uppercase().as_str() {
            "GPU" => Platform::Gpu,
            "CPU" => Platform::Cpu,
            _ => Platform::Auto,
        }
    }
}

/// Parsed `deploy.json` config for a size dir: the platform (device strategy),
/// an optional human description (shown in the model dropdown), an optional
/// `default` flag (marks this size as the out-of-box model + the fallback when
/// the persisted selection is gone), and the asymmetric embedding prefixes
/// (`searchQueryPrefix` / `importDocPrefix`).
pub struct DeployConfig {
    pub platform: Platform,
    pub description: String,
    pub is_default: bool,
    /// Sort order in the dropdown (lower = higher in list; default 0).
    pub sort: i32,
    /// Prefix prepended to a search QUERY before embedding (deploy.json
    /// `searchQueryPrefix`). Asymmetric embedding models (Qwen3-Embedding, BGE)
    /// require a distinct instruction prefix on the query side; "" for symmetric
    /// models (Gemma). Applied by the service in `search`. Missing field / JSON
    /// null / non-string -> "".
    pub search_query_prefix: String,
    /// Prefix prepended to each imported DOCUMENT chunk before embedding
    /// (deploy.json `importDocPrefix`). The document-side counterpart of
    /// `search_query_prefix`; "" for symmetric models. Applied by the service
    /// in `reindex_doc`. Missing field / JSON null / non-string -> "".
    pub import_doc_prefix: String,
    /// Model-author-recommended chunk size in tokens (deploy.json `chunkSize`).
    /// `None` if unset -> the service falls back to a built-in default (1024).
    /// Applied by `reindex_doc` ONLY when the user's global `chunk_size` setting
    /// is `0` ("auto"); a positive user setting overrides it. Surfaced so the
    /// frontend can show the recommended value next to the Auto toggle.
    pub chunk_size: Option<u32>,
    /// Model-author-recommended chunk overlap in tokens (deploy.json
    /// `chunkOverlap`). `None` if unset -> falls back to a built-in default
    /// (100). Used the same auto-vs-override way as `chunk_size`.
    pub chunk_overlap: Option<u32>,
    /// Title field delimiter (deploy.json `docTitleDelimiter`, optional).
    /// When the document template embeds a real title (`{title}`), the model's
    /// field separator character (e.g. `|` for granite's `Title: {title} | ...`
    /// format) must not appear INSIDE the title or a parser would split at the
    /// wrong boundary. When set, occurrences of this string in the title are
    /// replaced with "_" at render time. Per-model convention (granite-specific
    /// `|`), NOT a global rule — models whose template has no field separator
    /// simply leave this unset. None/missing -> no replacement.
    pub doc_title_delimiter: Option<String>,
    /// True when this size's model files ship inside the installer
    /// (deploy.json `bundle: true`; legacy `default: true` honored as
    /// fallback). Non-bundled sizes ship only their deploy.json and are
    /// downloaded on demand via `download_urls`.
    pub bundle: bool,
    /// Default relevance score threshold (deploy.json `defaultScore`, default
    /// 0). Applied on first load when the user has never set a value, and
    /// restored on model switch (user's per-model tuning is dropped).
    pub default_score: f32,
    /// Default semantic (vector) weight (deploy.json `defaultWeightVector`,
    /// default 0.5). Same first-load / model-switch semantics as
    /// `default_score`.
    pub default_weight_vector: f32,
    /// Default keyword weight (deploy.json `defaultWeightKeyword`, default
    /// 0.5). Same first-load / model-switch semantics as `default_score`.
    pub default_weight_keyword: f32,
    /// Ordered download sources (deploy.json `downloadUrls`, an object of
    /// `{"source name": "url"}`). Empty = not downloadable. The FIRST entry is
    /// the default source; the rest are fallbacks.
    pub download_urls: Vec<(String, String)>,
}

/// Render the embedding input for one document chunk from the deploy.json
/// `importDocPrefix` TEMPLATE. Placeholder syntax (model-agnostic):
/// - `{title}`   -> the doc's title (sanitized per `doc_title_delimiter`)
/// - `{content}` -> the raw chunk text
/// Back-compat: a template WITHOUT `{content}` is treated as a plain prefix
/// (Qwen3/BGE style, e.g. "passage: ") — rendered (with `{title}` substituted
/// if present) and concatenated before the content, exactly the pre-template
/// behavior. Templates that DO contain `{content}` control the full layout
/// (granite style: `"Title: {title} | {content}"`).
pub fn render_doc_embed_text(
    template: &str,
    title: &str,
    content: &str,
    title_delimiter: Option<&str>,
) -> String {
    let safe_title = match title_delimiter {
        Some(d) if !d.is_empty() && title.contains(d) => title.replace(d, "_"),
        _ => title.to_string(),
    };
    if template.contains("{content}") {
        template.replace("{title}", &safe_title).replace("{content}", content)
    } else {
        format!("{}{}", template.replace("{title}", &safe_title), content)
    }
}

/// Render the embedding input for a search query from the deploy.json
/// `searchQueryPrefix` TEMPLATE. `{query}` -> the raw query; a template
/// without the placeholder keeps the plain-prefix behavior
/// (`template + query`).
pub fn render_query_embed_text(template: &str, query: &str) -> String {
    if template.contains("{query}") {
        template.replace("{query}", query)
    } else {
        format!("{}{}", template, query)
    }
}

/// Read `size_dir/deploy.json`
/// (`{"platform":"AUTO"|"GPU"|"CPU", "description":"...", "default":true}`).
/// Missing file / invalid JSON / missing fields -> `Auto`, empty description,
/// `is_default=false`. One file read; both `resolve_platform` (at load) and
/// `list_models` (for the dropdown + default selection) use this.
pub fn read_deploy_config(size_dir: &Path) -> DeployConfig {
    let empty = || DeployConfig {
        platform: Platform::Auto,
        description: String::new(),
        is_default: false,
        sort: 0,
        search_query_prefix: String::new(),
        import_doc_prefix: String::new(),
        chunk_size: None,
        chunk_overlap: None,
        doc_title_delimiter: None,
        bundle: false,
        default_score: 0.0,
        default_weight_vector: 0.5,
        default_weight_keyword: 0.5,
        download_urls: Vec::new(),
    };
    let Ok(text) = std::fs::read_to_string(size_dir.join("deploy.json")) else {
        return empty();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        log::warn!("[RAG] deploy.json in {} is not valid JSON - using AUTO", size_dir.display());
        return empty();
    };
    let platform = match v.get("platform").and_then(|p| p.as_str()) {
        Some(s) => Platform::parse(s),
        None => Platform::Auto,
    };
    let description = v
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_string();
    // Bundle flag: `bundle: true` ships the model files in the installer;
    // legacy `default: true` is honored as a fallback (older deploy.json
    // files). Either marks the size as the out-of-box model.
    let is_default = v
        .get("bundle")
        .and_then(|d| d.as_bool())
        .or_else(|| v.get("default").and_then(|d| d.as_bool()))
        .unwrap_or(false);
    let sort = v.get("sort").and_then(|s| s.as_i64()).unwrap_or(0) as i32;
    // Asymmetric embedding prefixes (deploy.json `searchQueryPrefix` /
    // `importDocPrefix`). Missing field / JSON null / non-string -> "" (symmetric
    // models like Gemma need no prefix). Applied by the service on the query vs
    // document side respectively. `as_str()` returns None on JSON null, so an
    // explicit null also falls back to "".
    let search_query_prefix = v
        .get("searchQueryPrefix")
        .and_then(|p| p.as_str())
        .unwrap_or("")
        .to_string();
    let import_doc_prefix = v
        .get("importDocPrefix")
        .and_then(|p| p.as_str())
        .unwrap_or("")
        .to_string();
    // Model-author-recommended chunk size / overlap (deploy.json `chunkSize` /
    // `chunkOverlap`, in tokens). Optional — `None` if absent / null / negative
    // / non-int. Used by `reindex_doc` when the user's global setting is `0`
    // ("auto"); a positive user setting overrides them.
    let chunk_size = v
        .get("chunkSize")
        .and_then(|n| n.as_u64())
        .filter(|&n| n > 0 && n <= u32::MAX as u64)
        .map(|n| n as u32);
    let chunk_overlap = v
        .get("chunkOverlap")
        .and_then(|n| n.as_u64())
        .filter(|&n| n <= u32::MAX as u64)
        .map(|n| n as u32);
    // Title field delimiter for `{title}` templates (deploy.json
    // `docTitleDelimiter`). Optional; missing/null/non-string -> None.
    let doc_title_delimiter = v
        .get("docTitleDelimiter")
        .and_then(|d| d.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    // Model-default retrieval params (deploy.json `defaultScore` /
    // `defaultWeightVector` / `defaultWeightKeyword`). Applied on first model
    // load when the user has never tuned them, and restored on model switch
    // (per-model tuning is dropped — thresholds/weights are model-specific).
    let num = |key: &str, fallback: f32| -> f32 {
        v.get(key)
            .and_then(|n| n.as_f64())
            .filter(|n| n.is_finite())
            .map(|n| n as f32)
            .unwrap_or(fallback)
    };
    let default_score = num("defaultScore", 0.0);
    let default_weight_vector = num("defaultWeightVector", 0.5);
    let default_weight_keyword = num("defaultWeightKeyword", 0.5);
    // Ordered download sources (deploy.json `downloadUrls`, an object of
    // `{"source name": "url"}`). serde_json's Map preserves insertion order
    // only with the `preserve_order` feature, so sort is NOT guaranteed —
    // keep the object's natural iteration order and let the first entry be
    // the default source (documented behavior).
    let download_urls: Vec<(String, String)> = v
        .get("downloadUrls")
        .and_then(|d| d.as_object())
        .map(|m| {
            m.iter()
                .filter_map(|(k, val)| {
                    val.as_str()
                        .filter(|s| !s.is_empty())
                        .map(|s| (k.clone(), s.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    DeployConfig {
        platform,
        description,
        is_default,
        sort,
        search_query_prefix,
        import_doc_prefix,
        chunk_size,
        chunk_overlap,
        doc_title_delimiter,
        bundle: is_default,
        default_score,
        default_weight_vector,
        default_weight_keyword,
        download_urls,
    }
}

/// Read the per-size deployment platform (convenience over `read_deploy_config`).
pub fn read_deploy_platform(size_dir: &Path) -> Platform {
    read_deploy_config(size_dir).platform
}

/// Read the per-size description (for the model dropdown). Empty if absent.
pub fn read_deploy_description(size_dir: &Path) -> String {
    read_deploy_config(size_dir).description
}

/// Resolve the effective platform: `RAG_GGUF_DEVICE` env override (debug) >
/// the user's `mv.device` setting (Phase 4: the UI selector actually drives
/// the engine) > `deploy.json` > Auto. Callers pass the user override read
/// from config (None = "auto" / unset).
pub fn resolve_platform_with_user(size_dir: &Path, user: Option<Platform>) -> Platform {
    if let Some(p) = env_platform_override() {
        return p;
    }
    if let Some(p) = user {
        return p;
    }
    read_deploy_platform(size_dir)
}

/// Legacy entry (deploy.json only) — kept for any caller without a user
/// override; mv's load path uses `resolve_platform_with_user`.
pub fn resolve_platform(size_dir: &Path) -> Platform {
    resolve_platform_with_user(size_dir, None)
}

/// `load_embedder` env override: if `RAG_GGUF_DEVICE` is set it overrides the
/// model's deploy.json (debug/dev convenience - try both devices without
/// editing the file). `cpu` / `metal|cuda|gpu` map to Cpu/Gpu.
fn env_platform_override() -> Option<Platform> {
    match std::env::var("RAG_GGUF_DEVICE").as_deref().ok() {
        Some("cpu") => Some(Platform::Cpu),
        Some("metal") | Some("cuda") | Some("gpu") => Some(Platform::Gpu),
        _ => None,
    }
}

/// `&mut self` (candle's `Device` is `&self`, but the trait is unified on
/// `&mut` so callers don't branch on backend).
pub trait Embedder: Send + Sync {
    /// Embed a single text into an `embed_dim`-long f32 vector.
    fn embed(&mut self, text: &str) -> Result<Vec<f32>>;

    /// Embed a batch of texts in one forward pass (padded). Returns one vector
    /// per input, in order. The main import-throughput lever.
    fn embed_batch(&mut self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;

    /// The embedding (output) dimension, read from the model at load. Drives
    /// the lancedb `FixedSizeList<f32, embed_dim>` schema + row slicing.
    fn embed_dim(&self) -> usize;

    /// The model's context window in tokens (from `config.json`
    /// `max_position_embeddings` / GGUF metadata). Caps the chunk-size UI.
    fn max_context(&self) -> u32;

    /// Tokenize `text` once, return each token's `(start, end)` byte offsets.
    /// The chunker sizes chunks by token in O(n) (single tokenize pass).
    fn tokenize_offsets(&self, text: &str) -> Vec<(usize, usize)>;

    /// Human-readable execution-provider config that took effect, e.g.
    /// "Metal+CPU", "CPU". Surfaced in the enable-time log so slow
    /// imports can be diagnosed (GPU engaged or not). Backend-specific
    /// diagnostics are folded into this string.
    fn ep_label(&self) -> &str;

    /// The backend identifier: "gguf". Surfaced in the enable log + the model
    /// dropdown.
    fn backend(&self) -> &str;
}

/// Load the `Embedder` for `size_dir` by probing for the `model.gguf` model
/// file. `size_dir` is self-contained (per the stage-18 dir layout): it holds
/// the model file + `tokenizer.json` + `config.json`. STRICT: only
/// `model.gguf` is accepted (matches `detect_format`'s ready rule — a
/// user-renamed `model.gguf` is not a downloaded model). Returns an error if
/// it is absent (the size isn't ready - the caller should have checked via
/// `list_models`).
pub fn load_embedder(size_dir: &Path) -> Result<Box<dyn Embedder>> {
    load_embedder_with_user_platform(size_dir, None)
}

/// `load_embedder` + the user's `mv.device` selection ("auto" → None).
/// Precedence: RAG_GGUF_DEVICE env (debug) > user setting > deploy.json.
pub fn load_embedder_with_user_platform(
    size_dir: &Path,
    user: Option<Platform>,
) -> Result<Box<dyn Embedder>> {
    let gguf = size_dir.join("model.gguf");
    if gguf.is_file() {
        GgufEmbedder::load_with_user_platform(&gguf, user).map(|m| Box::new(m) as Box<dyn Embedder>)
    } else {
        Err(anyhow!(
            "no model.gguf in '{}' - download the model first",
            size_dir.display()
        ))
    }
}

/// Detect the format of a ready size dir by file presence: "gguf" | "" (not
/// ready). Used by `list_models` to populate `RagModelInfo.format` without
/// loading the model. STRICT: only a `model.gguf` file counts as downloaded —
/// any other `*.gguf` name (e.g. a user-renamed `model.gguf`) does NOT make
/// the size ready (the download flow publishes exactly `model.gguf`).
pub fn detect_format(size_dir: &Path) -> &'static str {
    if size_dir.join("model.gguf").is_file() {
        "gguf"
    } else {
        ""
    }
}

/// Read the model's max context window (tokens). Resolution order:
/// 1. `config.json`'s `max_position_embeddings` (GGUF dirs that ship one,
///    e.g. Gemma3).
/// 2. GGUF metadata's `{arch}.context_length` (parsed from the `.gguf` header,
///    no model load - for GGUF-only dirs like Qwen3 that have no config.json).
/// 3. Fallback 2048 (a safe common default) only when both are missing.
/// Standalone (no runtime / no backend crate needed) so the UI can show the
/// chunk_size upper bound even when RAG is off.
pub fn read_max_context(model_dir: &Path) -> u32 {
    if let Ok(s) = std::fs::read_to_string(model_dir.join("config.json")) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
            if let Some(m) = v.get("max_position_embeddings").and_then(|m| m.as_u64()) {
                return m as u32;
            }
        }
    }
    if let Some(ctx) = crate::mv::gguf::read_gguf_context_length(model_dir) {
        return ctx;
    }
    2048
}

/// Check whether the device has enough free memory to run the embedding model
/// (the model + runtime + lancedb needs a sizable chunk). Returns Ok(()) if
/// sufficient, Err with a human message otherwise. Threshold: 2 GiB free.
pub fn check_memory_sufficient() -> Result<()> {
    const MIN_FREE_BYTES: u64 = 2 * 1024 * 1024 * 1024; // 2 GiB
    let free = free_memory_bytes().unwrap_or(0);
    if free > 0 && free < MIN_FREE_BYTES {
        return Err(anyhow!(
            "insufficient memory: {:.1} GiB free (need ≥ {:.0} GiB to enable RAG)",
            free as f64 / (1024.0 * 1024.0 * 1024.0),
            MIN_FREE_BYTES as f64 / (1024.0 * 1024.0 * 1024.0),
        ));
    }
    Ok(())
}

/// Best-effort free-memory probe, cross-platform, no extra deps. Used by
/// `check_memory_sufficient` to gate enabling RAG.
pub fn free_memory_bytes() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        // Use the real page size (16384 on Apple Silicon, 4096 on Intel) and
        // count free + speculative (reclaimable) pages from vm_stat.
        let out = Command::new("sh").args([
            "-c",
            "ps=$(/usr/sbin/sysctl -n hw.pagesize); /usr/bin/vm_stat | awk -v ps=$ps \"/^Pages free/ {f=$3} /^Pages speculative/ {s=$3} END {gsub(/[^0-9]/,\"\",f); gsub(/[^0-9]/,\"\",s); print (f+s)*ps}\"",
        ]).output().ok()?;
        let s = String::from_utf8_lossy(&out.stdout);
        s.trim().parse::<u64>().ok().filter(|&v| v > 0)
    }
    #[cfg(target_os = "linux")]
    {
        use std::process::Command;
        let out = Command::new("sh")
            .args(["-c", "awk '/MemAvailable/ {print $2*1024}' /proc/meminfo"])
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout);
        s.trim().parse::<u64>().ok()
    }
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        let out = Command::new("powershell")
            .args(["-NoProfile", "-Command", "(Get-Ciminstance Win32_OperatingSystem).FreePhysicalMemory*1MB"])
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout);
        s.trim().parse::<u64>().ok()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

/// Best-effort probe of THIS process's RSS (resident set size) in MiB. Used to
/// confirm RAG's memory is actually released on disable.
pub fn process_rss_mib() -> Option<u64> {
    let pid = std::process::id();
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::process::Command;
        let out = Command::new("ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout);
        // ps reports RSS in KiB -> convert to MiB.
        s.trim().parse::<u64>().ok().map(|kib| kib / 1024)
    }
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        let out = Command::new("powershell")
            .args(["-NoProfile", "-Command", &format!("Get-Process -Id {pid} | Select-Object -ExpandProperty WS")])
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout);
        s.trim().parse::<u64>().ok().map(|b| b / (1024 * 1024))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

#[cfg(test)]
mod template_tests {
    use super::{render_doc_embed_text, render_query_embed_text};

    #[test]
    fn granite_template_with_title_and_content() {
        let out = render_doc_embed_text(
            "Title: {title} | {content}",
            "消息配置与待办事项 - 迭代评审文档",
            "本周待办事项：1. 完成版本发布",
            Some("|"),
        );
        assert_eq!(
            out,
            "Title: 消息配置与待办事项 - 迭代评审文档 | 本周待办事项：1. 完成版本发布"
        );
    }

    #[test]
    fn title_delimiter_replaced() {
        // 标题里含分隔符 | → 替换成 "；"，防止被模型按字段边界错误切分
        let out = render_doc_embed_text(
            "Title: {title} | {content}",
            "A|B 评审",
            "正文",
            Some("|"),
        );
        assert_eq!(out, "Title: A_B 评审 | 正文");
    }

    #[test]
    fn no_delimiter_configured_no_replacement() {
        let out = render_doc_embed_text("Title: {title} | {content}", "A|B", "c", None);
        assert_eq!(out, "Title: A|B | c");
    }

    #[test]
    fn plain_prefix_template_back_compat() {
        // 无 {content} 占位符 = 旧「前缀」语义（Qwen3/BGE 风格）
        assert_eq!(
            render_doc_embed_text("passage: ", "任意标题", "正文", None),
            "passage: 正文"
        );
        assert_eq!(
            render_doc_embed_text("passage: {title}\n", "T", "C", None),
            "passage: T\nC"
        );
    }

    #[test]
    fn query_template() {
        assert_eq!(render_query_embed_text("Query: {query}", "待办"), "Query: 待办");
        // 无占位符 = 旧前缀拼接
        assert_eq!(render_query_embed_text("query: ", "待办"), "query: 待办");
    }
}
