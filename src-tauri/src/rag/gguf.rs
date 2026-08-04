//! GGUF embedding backend (candle), wrapped as an `Embedder`.
//!
//! Loads a GGUF embedding model from a self-contained size dir (`*.gguf` +
//! optional `tokenizer.json` + `config.json` + `deploy.json`) and embeds text
//! into an `embed_dim`-long L2-normalized f32 vector. The model forward +
//! pooling is delegated to a `GgufArch` (strategy) dispatched by the GGUF's
//! `general.architecture`:
//!   - `gemma`/`gemma2`/`gemma3`/`gemma-embedding` -> `Gemma3EmbedArch`
//!   - `qwen2`/`qwen3` -> `Qwen3EmbedArch`
//! Adding a new arch = a new `GgufArch` impl + a dispatch arm here.
//!
//! Tokenizer: if `tokenizer.json` ships in the size dir, use it (fast path).
//! Otherwise reconstruct a HuggingFace `Tokenizer` from the GGUF's embedded
//! `tokenizer.ggml.*` metadata (BPE for gpt2/qwen2/llama3-style) - so GGUF is
//! self-contained, no external tokenizer file required.
//!
//! Special tokens: the GGUF's `add_bos_token`/`add_eos_token` + bos/eos ids
//! drive manual prepend/append (encode without special tokens, then add bos/eos
//! per the GGUF's convention) - so each arch gets the input it was trained on.
//!
//! Device strategy mirrors ort's: GPU-first (Metal/CUDA), CPU fallback, driven
//! by `deploy.json`'s `platform` (AUTO/GPU/CPU).

use std::path::Path;

use anyhow::{anyhow, Result};
use candle_core::quantized::gguf_file::{Content, Value};
use candle_core::Device;
use tokenizers::tokenizer::Tokenizer;
use tokenizers::{
    models::{bpe::BpeBuilder, unigram::Unigram},
    pre_tokenizers::{
        byte_level::ByteLevel,
        metaspace::{Metaspace, PrependScheme},
    },
};

use crate::rag::embedder::{Embedder, Platform};
use crate::rag::gguf_gemma::{Gemma3EmbedArch, GgufArch};
use crate::rag::gguf_qwen3::Qwen3EmbedArch;

/// A loaded GGUF embedding model: tokenizer + architecture-specific forward
/// (held as a `Box<dyn GgufArch>`). `embed`/`embed_batch` tokenize (with manual
/// bos/eos), pad, run the arch forward (returns pooled + L2-normalized
/// [batch, hidden]), and slice rows.
pub struct GgufEmbedder {
    arch: Box<dyn GgufArch>,
    tokenizer: Tokenizer,
    device: Device,
    embed_dim: usize,
    max_context: u32,
    ep_label: String,
    /// Whether to prepend `bos_id` to each encoded sequence (from the GGUF's
    /// `tokenizer.ggml.add_bos_token`).
    add_bos: bool,
    /// Whether to append `eos_id` to each encoded sequence.
    add_eos: bool,
    bos_id: u32,
    eos_id: u32,
}

impl GgufEmbedder {
    /// Load the GGUF model. `gguf_path` is the `*.gguf` FILE (the size dir -
    /// holding optional `tokenizer.json` + `config.json` + `deploy.json` - is
    /// its parent). Reads the GGUF `Content` ONCE and shares it with the
    /// tokenizer builder + the arch (avoids a double-read). Device from
    /// `deploy.json` (AUTO/GPU/CPU; `RAG_GGUF_DEVICE` env overrides).
    pub fn load(gguf_path: &Path) -> Result<Self> {
        if !gguf_path.exists() {
            return Err(anyhow!("GGUF model not found: {}", gguf_path.display()));
        }
        let size_dir = gguf_path
            .parent()
            .ok_or_else(|| anyhow!("gguf path has no parent dir"))?;
        let platform = crate::rag::embedder::resolve_platform(size_dir);
        let (device, ep_label) = pick_device(platform)?;

        // Read the GGUF Content ONCE - shared by the tokenizer builder (reads
        // metadata) + the arch (reads tensors via content.tensor(file, ...)).
        let mut file = std::fs::File::open(gguf_path)
            .map_err(|e| anyhow!("open gguf {}: {}", gguf_path.display(), e))?;
        let content = Content::read(&mut file)
            .map_err(|e| anyhow!("read gguf {}: {}", gguf_path.display(), e))?;
        let md = &content.metadata;

        // Tokenizer: prefer a shipped tokenizer.json (fast, exact); else
        // reconstruct from the GGUF's embedded tokenizer.ggml.* metadata (BPE).
        let tokenizer_path = size_dir.join("tokenizer.json");
        let tokenizer = if tokenizer_path.exists() {
            Tokenizer::from_file(&tokenizer_path)
                .map_err(|e| anyhow!("load tokenizer {}: {}", tokenizer_path.display(), e))?
        } else {
            build_gguf_tokenizer(md)?
        };

        // Special-token convention from the GGUF (manual prepend/append).
        let add_bos = meta_bool(md, "tokenizer.ggml.add_bos_token").unwrap_or(false);
        let add_eos = meta_bool(md, "tokenizer.ggml.add_eos_token").unwrap_or(true);
        let bos_id = meta_u32(md, "tokenizer.ggml.bos_token_id").unwrap_or(0);
        let eos_id = meta_u32(md, "tokenizer.ggml.eos_token_id").unwrap_or(0);

        // Dispatch to the architecture impl by general.architecture.
        let arch_name = meta_string(md, "general.architecture")
            .ok_or_else(|| anyhow!("missing gguf key general.architecture"))?;
        let arch: Box<dyn GgufArch> = match arch_name.as_str() {
            "gemma" | "gemma2" | "gemma3" | "gemma-embedding" => {
                Box::new(Gemma3EmbedArch::from_content(&mut file, &content, &device)?)
            }
            "qwen2" | "qwen3" => {
                Box::new(Qwen3EmbedArch::from_content(&mut file, &content, &device)?)
            }
            other => {
                return Err(anyhow!(
                    "unsupported gguf architecture '{}' (supported: gemma*, qwen2/qwen3)",
                    other
                ))
            }
        };
        let embed_dim = arch.hidden_dim();
        let max_context = arch.max_context();
        Ok(Self {
            arch,
            tokenizer,
            device,
            embed_dim,
            max_context,
            ep_label: ep_label.to_string(),
            add_bos,
            add_eos,
            bos_id,
            eos_id,
        })
    }

    /// Tokenize `text` WITHOUT auto special tokens, then manually prepend bos
    /// (if add_bos) / append eos (if add_eos) per the GGUF's convention.
    fn encode_ids(&self, text: &str) -> Result<Vec<u32>> {
        let enc = self
            .tokenizer
            .encode(text, false)
            .map_err(|e| anyhow!("tokenize text: {}", e))?;
        let mut ids: Vec<u32> = enc.get_ids().to_vec();
        if ids.is_empty() {
            return Ok(ids);
        }
        if self.add_bos {
            ids.insert(0, self.bos_id);
        }
        if self.add_eos {
            ids.push(self.eos_id);
        }
        Ok(ids)
    }
}

impl Embedder for GgufEmbedder {
    fn embed(&mut self, text: &str) -> Result<Vec<f32>> {
        let ids = self.encode_ids(text)?;
        if ids.is_empty() {
            return Ok(vec![0.0; self.embed_dim]);
        }
        let seq = ids.len() as u32;
        let input_ids = tensor_ids(&ids, 1, seq, &self.device)?;
        let mask_vals = vec![1u32; ids.len()];
        let attn = tensor_mask(&mask_vals, 1, seq, &self.device)?;
        let emb = self.arch.forward_embed(&input_ids, &attn)?;
        // emb is [1, hidden] (rank 2) -> take the single row.
        let mut rows = emb.to_vec2::<f32>().map_err(|e| anyhow!("flatten embed: {}", e))?;
        Ok(rows.pop().unwrap_or_default())
    }

    fn embed_batch(&mut self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let cap = self.max_context.max(1) as usize;
        // Tokenize all texts (single-thread; cheap relative to embed).
        let all_ids: Vec<Vec<u32>> = texts
            .iter()
            .map(|t| self.encode_ids(t))
            .collect::<Result<Vec<_>>>()?;
        if all_ids.is_empty() {
            return Ok(Vec::new());
        }
        // Single batched forward for BOTH CPU and GPU. The matmul library
        // handles multi-threading internally:
        //   - CPU + accelerate (macOS): AMX BLAS sgemm (internally multi-threaded).
        //   - CPU + mkl (Linux x86_64): MKL BLAS sgemm (internally multi-threaded).
        //   - CPU + gemm (other): pure-Rust gemm crate (rayon multi-threaded).
        //   - GPU (Metal/CUDA): quantized int4 kernels (inherently parallel).
        // No external batch splitting needed — the BLAS/gemm handles it, and a
        // bigger [batch*seq, hidden] GEMM is more efficient than N small ones.
        let n = all_ids.len();
        let t0 = std::time::Instant::now();
        let rows = forward_sub_batch(&*self.arch, &self.device, cap, &all_ids)?;
        log::info!(
            "[RAG] embed_batch: {} chunks on {:?} in {}ms",
            n,
            self.device,
            t0.elapsed().as_millis()
        );
        Ok(rows)
    }

    fn embed_dim(&self) -> usize {
        self.embed_dim
    }

    fn max_context(&self) -> u32 {
        self.max_context
    }

    fn tokenize_offsets(&self, text: &str) -> Vec<(usize, usize)> {
        match self.tokenizer.encode(text, false) {
            Ok(enc) => enc.get_offsets().to_vec(),
            Err(_) => Vec::new(),
        }
    }

    fn ep_label(&self) -> &str {
        &self.ep_label
    }

    fn backend(&self) -> &str {
        "gguf"
    }
}

/// Build a HuggingFace `Tokenizer` from the GGUF's embedded `tokenizer.ggml.*`
/// metadata. Used when no `tokenizer.json` ships alongside the GGUF (so GGUF is
/// self-contained). Supports:
///   - BPE (`gpt2`/`qwen2`/`llama3`/`bert`): vocab + merges + ByteLevel.
///   - SentencePiece (`llama`): tokens + scores + Metaspace + byte-fallback.
fn build_gguf_tokenizer(md: &std::collections::HashMap<String, Value>) -> Result<Tokenizer> {
    let model = meta_string(md, "tokenizer.ggml.model").unwrap_or_default();
    match model.as_str() {
        "gpt2" | "qwen2" | "llama3" | "bert" => build_bpe_tokenizer(md),
        "llama" => build_spm_tokenizer(md),
        _ => Err(anyhow!(
            "no tokenizer.json and GGUF tokenizer model '{}' is not reconstructable (supported: gpt2/qwen2/llama3/llama) - ship a tokenizer.json",
            model
        )),
    }
}

/// BPE tokenizer (gpt2/qwen2/llama3-style): vocab + merges + ByteLevel.
fn build_bpe_tokenizer(md: &std::collections::HashMap<String, Value>) -> Result<Tokenizer> {
    let tokens = meta_string_array(md, "tokenizer.ggml.tokens")?;
    let merges_raw = meta_string_array(md, "tokenizer.ggml.merges").unwrap_or_default();
    let vocab: ahash::AHashMap<String, u32> = tokens
        .iter()
        .enumerate()
        .map(|(i, t)| (t.clone(), i as u32))
        .collect();
    let merges: Vec<(String, String)> = merges_raw
        .iter()
        .filter_map(|m| {
            let mut it = m.splitn(2, ' ');
            Some((it.next()?.to_string(), it.next()?.to_string()))
        })
        .collect();
    let unk = meta_u32(md, "tokenizer.ggml.unknown_token_id")
        .and_then(|id| tokens.get(id as usize).cloned())
        .unwrap_or_else(|| "<unk>".to_string());
    let bpe = BpeBuilder::new()
        .vocab_and_merges(vocab, merges)
        .unk_token(unk)
        .build()
        .map_err(|e| anyhow!("build BPE from gguf: {}", e))?;
    let mut tok = Tokenizer::new(bpe);
    tok.with_pre_tokenizer(Some(ByteLevel::new(false, true, true)));
    Ok(tok)
}

/// SentencePiece (llama) tokenizer: Unigram model (tokens + scores) + Metaspace
/// pre-tokenizer (▁ replacement, always prepend). byte_fallback=true (llama SPM
/// typically ships the 256 byte tokens). bos/eos added manually by the embedder
/// per `add_bos_token`/`add_eos_token`.
fn build_spm_tokenizer(md: &std::collections::HashMap<String, Value>) -> Result<Tokenizer> {
    let tokens = meta_string_array(md, "tokenizer.ggml.tokens")?;
    let scores = meta_f32_array(md, "tokenizer.ggml.scores").unwrap_or_else(|_| {
        // No scores -> uniform default (unigram needs a score per token).
        vec![-10.0_f32; tokens.len()]
    });
    let vocab: Vec<(String, f64)> = tokens
        .iter()
        .enumerate()
        .map(|(i, t)| (t.clone(), scores.get(i).copied().unwrap_or(-10.0) as f64))
        .collect();
    let unk_id = meta_u32(md, "tokenizer.ggml.unknown_token_id").map(|id| id as usize);
    let unigram = Unigram::from(vocab, unk_id, true)
        .map_err(|e| anyhow!("build SPM Unigram from gguf: {}", e))?;
    let mut tok = Tokenizer::new(unigram);
    // Metaspace: ▁ (U+2581) replaces spaces, always prepend (llama convention).
    tok.with_pre_tokenizer(Some(Metaspace::new('▁', PrependScheme::Always, true)));
    Ok(tok)
}

// ── GGUF metadata helpers (Value extraction) ────────────────────────────────

fn meta_string(md: &std::collections::HashMap<String, Value>, key: &str) -> Option<String> {
    match md.get(key)? {
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn meta_u32(md: &std::collections::HashMap<String, Value>, key: &str) -> Option<u32> {
    match md.get(key)? {
        Value::U32(v) => Some(*v),
        _ => None,
    }
}

fn meta_bool(md: &std::collections::HashMap<String, Value>, key: &str) -> Option<bool> {
    match md.get(key)? {
        Value::Bool(b) => Some(*b),
        _ => None,
    }
}

fn meta_string_array(
    md: &std::collections::HashMap<String, Value>,
    key: &str,
) -> Result<Vec<String>> {
    let arr = md
        .get(key)
        .ok_or_else(|| anyhow!("missing gguf key {}", key))?;
    let Value::Array(items) = arr else {
        return Err(anyhow!("gguf key {} is not an array", key));
    };
    items
        .iter()
        .map(|v| match v {
            Value::String(s) => Ok(s.clone()),
            _ => Err(anyhow!("array element of {} is not a string", key)),
        })
        .collect()
}

fn meta_f32_array(
    md: &std::collections::HashMap<String, Value>,
    key: &str,
) -> Result<Vec<f32>> {
    let arr = md
        .get(key)
        .ok_or_else(|| anyhow!("missing gguf key {}", key))?;
    let Value::Array(items) = arr else {
        return Err(anyhow!("gguf key {} is not an array", key));
    };
    items
        .iter()
        .map(|v| match v {
            Value::F32(x) => Ok(*x),
            Value::F64(x) => Ok(*x as f32),
            _ => Err(anyhow!("array element of {} is not f32/f64", key)),
        })
        .collect()
}

/// Build [batch, max_len] tensors for a sub-batch of token-id rows, run
/// `forward_embed`, return one `[hidden]` row per input (in order). Used both
/// for the single GPU forward and (one call per rayon worker) for the
/// CPU multi-threaded int4 path.
fn forward_sub_batch(
    arch: &dyn GgufArch,
    device: &Device,
    cap: usize,
    sub_ids: &[Vec<u32>],
) -> Result<Vec<Vec<f32>>> {
    let batch = sub_ids.len();
    let mut max_len = 0usize;
    let mut lens: Vec<usize> = Vec::with_capacity(batch);
    for ids in sub_ids {
        let n = ids.len().min(cap);
        max_len = max_len.max(n);
        lens.push(n);
    }
    max_len = max_len.max(1);
    // Flatten ids + mask with right-padding (mask=0 on pad), [batch * max_len].
    let mut ids: Vec<u32> = Vec::with_capacity(batch * max_len);
    let mut mask: Vec<u32> = Vec::with_capacity(batch * max_len);
    for (row_ids, &n) in sub_ids.iter().zip(lens.iter()) {
        ids.extend_from_slice(&row_ids[..n]);
        mask.extend(std::iter::repeat(1u32).take(n));
        for _ in n..max_len {
            ids.push(0);
            mask.push(0);
        }
    }
    let input_ids = tensor_ids(&ids, batch as u32, max_len as u32, device)?;
    let attn = tensor_mask(&mask, batch as u32, max_len as u32, device)?;
    let emb = arch.forward_embed(&input_ids, &attn)?;
    emb.to_vec2::<f32>().map_err(|e| anyhow!("flatten sub-batch: {}", e))
}

/// Build a [batch, seq] u32 token-id tensor on `device` from a flattened row.
fn tensor_ids(flat: &[u32], batch: u32, seq: u32, device: &Device) -> Result<candle_core::Tensor> {
    candle_core::Tensor::new(flat, device)?
        .reshape((batch as usize, seq as usize))
        .map_err(|e| anyhow!("reshape input_ids: {}", e))
}

/// Build a [batch, seq] u32 attention-mask tensor on `device`.
fn tensor_mask(flat: &[u32], batch: u32, seq: u32, device: &Device) -> Result<candle_core::Tensor> {
    candle_core::Tensor::new(flat, device)?
        .reshape((batch as usize, seq as usize))
        .map_err(|e| anyhow!("reshape attention_mask: {}", e))
}

/// Pick the candle device from the model's `Platform` (from `deploy.json`):
/// - `Cpu` → CPU.
/// - `Gpu` → force GPU; errors if no GPU available (no silent fallback).
/// - `Auto` → GPU-first, CPU fallback.
fn pick_device(platform: Platform) -> Result<(Device, &'static str)> {
    // For Cpu, skip GPU init entirely; otherwise try the platform GPU.
    let gpu = if platform == Platform::Cpu {
        None
    } else {
        pick_gpu()
    };
    match (gpu, platform) {
        (Some(d), _) => Ok(d),
        (_, Platform::Cpu) => Ok((Device::Cpu, "CPU")),
        (None, Platform::Auto) => {
            log::warn!("[RAG] no candle GPU device - using CPU (deploy AUTO)");
            Ok((Device::Cpu, "CPU"))
        }
        (None, Platform::Gpu) => Err(anyhow!(
            "deploy.json forces GPU but no GPU device is available (set \"platform\":\"AUTO\" to allow CPU fallback)"
        )),
    }
}

/// Try to init the platform GPU device. Returns `Some((device, label))` on
/// success, `None` if unavailable (no GPU / EP not in build / init failed).
fn pick_gpu() -> Option<(Device, &'static str)> {
    #[cfg(target_os = "macos")]
    {
        match Device::new_metal(0) {
            Ok(d) => return Some((d, "Metal")),
            Err(e) => log::warn!("[RAG] candle Metal device init failed: {e}"),
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        match Device::new_cuda(0) {
            Ok(d) => return Some((d, "CUDA")),
            Err(e) => log::warn!("[RAG] candle CUDA device init failed: {e}"),
        }
    }
    #[cfg(not(any(target_os = "macos", all(unix, not(target_os = "macos")))))]
    {
        // No GPU backend on this platform (e.g. Windows - candle has no DirectML).
    }
    None
}
