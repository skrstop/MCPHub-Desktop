//! Embedder strategy: a uniform trait over the ONNX (ort) and GGUF (candle)
//! backends so the rest of the RAG service is format-agnostic.
//!
//! `load_embedder(size_dir)` probes the size directory and instantiates the
//! right backend:
//!   - `model.onnx` present -> `OrtEmbedder` (ort + tokenizers, see `embedding.rs`)
//!   - `model.gguf` present -> `GgufEmbedder` (candle, see `gguf.rs`)
//!
//! Both backends implement `Embedder`: text -> `embed_dim`-long f32 vector
//! (L2-normalized), with GPU-first + CPU-fallback execution providers (ort:
//! CoreML/DirectML/CUDA; candle: Metal/CUDA). The trait keeps the service free
//! of any backend-specific types - `Runtime` holds a `Box<dyn Embedder>`, and
//! swapping in a new format is a new impl + a branch in `load_embedder`.
//!
//! Shared, backend-agnostic helpers (memory probes, `read_max_context` from
//! `config.json`) live here so both backends and the service (which needs the
//! memory gate + the UI context bound even with RAG off) can use them without
//! pulling in ort or candle.

use std::path::Path;

use anyhow::{anyhow, Result};

use crate::rag::embedding::OrtEmbedder;
use crate::rag::gguf::GgufEmbedder;

/// A loaded embedding model, abstracted over the on-disk format (ONNX / GGUF).
/// All backends produce an `embed_dim`-long L2-normalized f32 vector per input
/// text, expose their context window (for the chunk-size UI cap), and report
/// the execution-provider config that took effect (for the enable-time log).
///
/// `embed` / `embed_batch` take `&mut self` because the ort `Session::run` is
/// Per-model deployment platform, read from a size dir's `deploy.json`
/// (`{"platform": "AUTO"|"GPU"|"CPU"}`). Drives device selection for both
/// backends: GGUF (candle Metal/CUDA/CPU) and ONNX (ort CoreML/DirectML/CUDA
/// EP vs CPU). Missing file / field → `Auto` (GPU-first, CPU fallback).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Platform {
    /// GPU-first, CPU fallback (the default; format-aware for ort - quantized
    /// ONNX models stay CPU because GPU EPs can't map the contrib int4 ops).
    Auto,
    /// Force GPU (Metal/CUDA/CoreML). Errors if no GPU is available (no silent
    /// CPU fallback) - the user explicitly asked for GPU.
    Gpu,
    /// Force CPU.
    Cpu,
}

impl Platform {
    /// Parse a platform string (case-insensitive); unknown/AUTO → Auto.
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
    /// `None` if unset → the service falls back to a built-in default (1024).
    /// Applied by `reindex_doc` ONLY when the user's global `chunk_size` setting
    /// is `0` ("auto"); a positive user setting overrides it. Surfaced so the
    /// frontend can show the recommended value next to the Auto toggle.
    pub chunk_size: Option<u32>,
    /// Model-author-recommended chunk overlap in tokens (deploy.json
    /// `chunkOverlap`). `None` if unset → falls back to a built-in default
    /// (100). Used the same auto-vs-override way as `chunk_size`.
    pub chunk_overlap: Option<u32>,
}

/// Read `size_dir/deploy.json`
/// (`{"platform":"AUTO"|"GPU"|"CPU", "description":"...", "default":true}`).
/// Missing file / invalid JSON / missing fields → `Auto`, empty description,
/// `is_default=false`. One file read; both `resolve_platform` (at load) and
/// `list_models` (for the dropdown + default selection) use this.
pub fn read_deploy_config(size_dir: &Path) -> DeployConfig {
    let Ok(text) = std::fs::read_to_string(size_dir.join("deploy.json")) else {
        return DeployConfig {
            platform: Platform::Auto,
            description: String::new(),
            is_default: false,
            sort: 0,
            search_query_prefix: String::new(),
            import_doc_prefix: String::new(),
            chunk_size: None,
            chunk_overlap: None,
        };
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        log::warn!("[RAG] deploy.json in {} is not valid JSON - using AUTO", size_dir.display());
        return DeployConfig {
            platform: Platform::Auto,
            description: String::new(),
            is_default: false,
            sort: 0,
            search_query_prefix: String::new(),
            import_doc_prefix: String::new(),
            chunk_size: None,
            chunk_overlap: None,
        };
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
    let is_default = v.get("default").and_then(|d| d.as_bool()).unwrap_or(false);
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
    DeployConfig {
        platform,
        description,
        is_default,
        sort,
        search_query_prefix,
        import_doc_prefix,
        chunk_size,
        chunk_overlap,
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
/// `deploy.json` > Auto. Both backends call this at load with their size dir.
pub fn resolve_platform(size_dir: &Path) -> Platform {
    if let Some(p) = env_platform_override() {
        return p;
    }
    read_deploy_platform(size_dir)
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
    /// "CoreML+CPU", "Metal+CPU", "CUDA+CPU", "CPU". Surfaced in the
    /// enable-time log so slow imports can be diagnosed (GPU engaged or not).
    /// Backend-specific diagnostics (e.g. ort's "CPU forced: contrib quantized")
    /// are folded into this string.
    fn ep_label(&self) -> &str;

    /// The backend identifier: "onnx" or "gguf". Surfaced in the enable log +
    /// the model dropdown so users can tell which engine a size uses.
    fn backend(&self) -> &str;
}

/// Load the right `Embedder` for `size_dir` by probing the model file present.
/// `size_dir` is self-contained (per the stage-18 dir layout): it holds the
/// model file + `tokenizer.json` + `config.json`. ONNX = a `model.onnx` file;
/// GGUF = any `*.gguf` file (bundled names like `model.gguf` and the
/// downloaded `model.gguf` are both accepted). Returns an error if neither is
/// present (the size isn't ready - the caller should have checked via
/// `list_models`).
pub fn load_embedder(size_dir: &Path) -> Result<Box<dyn Embedder>> {
    if size_dir.join("model.onnx").exists() {
        OrtEmbedder::load(size_dir).map(|m| Box::new(m) as Box<dyn Embedder>)
    } else if let Some(gguf) = find_gguf(size_dir) {
        GgufEmbedder::load(&gguf).map(|m| Box::new(m) as Box<dyn Embedder>)
    } else {
        Err(anyhow!(
            "no model file in '{}' (need model.onnx or *.gguf) - download it first",
            size_dir.display()
        ))
    }
}

/// Detect the format of a ready size dir by file presence: "onnx" | "gguf" |
/// "" (not ready). Used by `list_models` to populate `RagModelInfo.format`
/// without loading the model. GGUF is any `*.gguf` file (so bundled
/// `model.gguf` and downloaded `model.gguf` both register).
pub fn detect_format(size_dir: &Path) -> &'static str {
    if size_dir.join("model.onnx").exists() {
        "onnx"
    } else if find_gguf(size_dir).is_some() {
        "gguf"
    } else {
        ""
    }
}

/// Find the (first) `*.gguf` file in a dir, or None. Used by `load_embedder` /
/// `detect_format` so bundled GGUF files keep their descriptive names (e.g.
/// `model.gguf`) instead of being forced to `model.gguf`.
fn find_gguf(dir: &Path) -> Option<std::path::PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()).map(|s| s.eq_ignore_ascii_case("gguf")).unwrap_or(false) {
            return Some(p);
        }
    }
    None
}

/// Read the model's max context window (tokens). Resolution order:
/// 1. `config.json`'s `max_position_embeddings` (ONNX + GGUF dirs that ship
///    one, e.g. Gemma3).
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
    if let Some(ctx) = crate::rag::gguf::read_gguf_context_length(model_dir) {
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
