//! GGUF embedding backend (candle), wrapped as an `Embedder`.
//!
//! Loads a GGUF embedding model from a self-contained size dir (`*.gguf` +
//! optional `tokenizer.json` + `config.json` + `deploy.json`) and embeds text
//! into an `embed_dim`-long L2-normalized f32 vector. The model forward +
//! pooling is delegated to a `GgufArch` (strategy) dispatched by the GGUF's
//! `general.architecture`:
//!   - `gemma`/`gemma2`/`gemma3`/`gemma-embedding` -> `Gemma3EmbedArch`
//!   - `qwen2`/`qwen3` -> `Qwen3EmbedArch`
//!   - `nomic-bert-moe`/`nomic-bert` -> `NomicBertMoeArch`
//!   - `lfm2` -> `Lfm2Arch`
//!   - `modern-bert` -> `ModernBertArch`
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
//! Device strategy: GPU-first (Metal on macOS), CPU fallback, driven by
//! `deploy.json`'s `platform` (AUTO/GPU/CPU). Linux/Windows are CPU-only.

use std::path::Path;

use anyhow::{anyhow, Result};
use candle_core::quantized::gguf_file::{Content, Value};
use candle_core::Device;
use tokenizers::models::{bpe::BpeBuilder, unigram::Unigram};
use tokenizers::normalizers::Precompiled;
use tokenizers::pre_tokenizers::{
    byte_level::ByteLevel,
    metaspace::{Metaspace, PrependScheme},
    sequence::Sequence,
    whitespace::WhitespaceSplit,
};
use tokenizers::tokenizer::Tokenizer;

use crate::mv::embedder::{Embedder, Platform};
use crate::mv::gguf_gemma::{Gemma3EmbedArch, GgufArch};
use crate::mv::gguf_lfm2::Lfm2Arch;
use crate::mv::gguf_modernbert::ModernBertArch;
use crate::mv::gguf_nomic::NomicBertMoeArch;
use crate::mv::gguf_qwen3::Qwen3EmbedArch;

/// Read the model's max context length from a GGUF file's metadata, WITHOUT
/// loading the model - parses only the GGUF header (metadata key-values, no
/// tensors are read into memory). Used by `read_max_context` for GGUF-only
/// size dirs that don't ship a `config.json` (e.g. Qwen3-Embedding). Returns
/// None if no `.gguf` file in `model_dir` or the `{arch}.context_length` key
/// is absent.
pub fn read_gguf_context_length(model_dir: &Path) -> Option<u32> {
    let gguf_file = std::fs::read_dir(model_dir)
        .ok()?
        .flatten()
        // Case-insensitive to match find_gguf (MODEL.GGUF otherwise gets a
        // default context length, review round 8).
        .find(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| x.eq_ignore_ascii_case("gguf"))
                .unwrap_or(false)
        })
        .map(|e| e.path())?;
    let mut file = std::fs::File::open(&gguf_file).ok()?;
    let content = Content::read(&mut file).ok()?;
    let md = &content.metadata;
    let arch = md
        .get("general.architecture")
        .and_then(|v| v.to_string().ok())?;
    let key = format!("{}.context_length", arch);
    md.get(&key).and_then(|v| v.to_u32().ok())
}

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
    pub fn load_with_user_platform(gguf_path: &Path, user: Option<Platform>) -> Result<Self> {
        if !gguf_path.exists() {
            return Err(anyhow!("GGUF model not found: {}", gguf_path.display()));
        }
        let size_dir = gguf_path
            .parent()
            .ok_or_else(|| anyhow!("gguf path has no parent dir"))?;
        let platform = crate::mv::embedder::resolve_platform_with_user(size_dir, user);
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
        // Honor the GGUF's explicit flag; default FALSE to match llama.cpp
        // (its add_special never auto-appends EOS). The previous `true`
        // default appended an unrequested EOS — for last-token pooling that
        // pools the EOS position and shifts every vector.
        let add_eos = meta_bool(md, "tokenizer.ggml.add_eos_token").unwrap_or(false);
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
            // nomic-embed-text-v2-moe (BERT-MoE encoder, mean-pool). "nomic-bert"
            // (v1, dense) shares the arch - moe_every_n_layers=0 -> all dense.
            "nomic-bert-moe" | "nomic-bert" => {
                Box::new(NomicBertMoeArch::from_content(&mut file, &content, &device)?)
            }
            // LFM2.5-Embedding (hybrid ShortConv + attention encoder, CLS-pool).
            "lfm2" => {
                Box::new(Lfm2Arch::from_content(&mut file, &content, &device)?)
            }
            // modern-bert (ModernBert encoder, CLS-pool + L2, pre-norm, full-
            // head_dim RoPE, sliding-window pattern). For Granite Embedding 97M
            // Multilingual R2 and any other modern-bert GGUF embedding.
            "modern-bert" => {
                Box::new(ModernBertArch::from_content(&mut file, &content, &device)?)
            }
            other => {
                return Err(anyhow!(
                    "unsupported gguf architecture '{}' (supported: gemma*, qwen2/qwen3, nomic-bert-moe, lfm2, modern-bert)",
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

    /// Origin-parity entry: no user override (deploy.json decides, env still
    /// wins). mv's load path uses `load_with_user_platform`.
    pub fn load(gguf_path: &Path) -> Result<Self> {
        Self::load_with_user_platform(gguf_path, None)
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
        let mut ids = self.encode_ids(text)?;
        // Cap to max_context (parity with embed_batch's forward_sub_batch):
        // an over-context input (e.g. a pathological single query) would build
        // a [1, seq≫context] tensor — huge attention alloc, Metal buffer
        // overflow / OOM instead of a clean truncation. encode_ids already
        // appended bos/eos; truncate the BODY (drop the tail eos first, then
        // re-push it) so the eos token survives for pooling layers —
        // truncating after append silently drops it for long inputs and
        // degrades long-query retrieval quality.
        let cap = self.max_context.max(1) as usize;
        let overhead = self.add_bos as usize + self.add_eos as usize;
        if ids.len() > cap {
            let eos = if self.add_eos { ids.pop() } else { None };
            ids.truncate(cap.saturating_sub(overhead).max(1));
            if let Some(e) = eos {
                ids.push(e);
            }
        }
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
        // Length-bucketed batching: forward cost is [batch × max_len_in_batch]
        // (right-padding to the longest row), so one long row in a batch of
        // short rows makes EVERY row pay the long row's seq length. Chunk
        // lengths vary wildly (statement-boundary chunking of minified JS:
        // 300..2048 tokens), which measured ~6x wasted GEMM on a real 3.3MB
        // import (embedMs=437s of a 522s total). Sorting rows by length and
        // forwarding same-length runs together shrinks the padding waste to
        // ~1.1x with ZERO semantic change (each row's embedding is identical;
        // only the compute order differs). Rows are re-scattered to the
        // caller's order below.
        let n = all_ids.len();
        // Empty-text chunks (tokenize -> 0 ids) would forward as an all-zero
        // attention-mask row — masked softmax over a fully-masked row produces
        // NaN embeddings that poison vector search. Short-circuit them with a
        // zero vector before batching.
        let mut zero_rows: Vec<usize> = Vec::new();
        let mut live_ids: Vec<Vec<u32>> = Vec::with_capacity(n);
        let mut live_orig: Vec<usize> = Vec::with_capacity(n);
        for (i, ids) in all_ids.iter().enumerate() {
            if ids.is_empty() {
                zero_rows.push(i);
                continue;
            }
            // eos-preserving truncation (parity with embed()): encode_ids
            // appended bos/eos; truncate the BODY so the tail eos survives
            // for pooling layers — document chunks are the main long-input
            // path and silently dropping eos degrades retrieval quality.
            let mut ids = ids.clone();
            let overhead = self.add_bos as usize + self.add_eos as usize;
            if ids.len() > cap {
                let eos = if self.add_eos { ids.pop() } else { None };
                ids.truncate(cap.saturating_sub(overhead).max(1));
                if let Some(e) = eos {
                    ids.push(e);
                }
            }
            live_ids.push(ids);
            live_orig.push(i);
        }
        let mut order: Vec<usize> = (0..live_ids.len()).collect();
        order.sort_by_key(|&i| live_ids[i].len().min(cap));
        let mut rows_sorted: Vec<Vec<f32>> = Vec::with_capacity(live_ids.len());
        let mut bucket: Vec<usize> = Vec::new();
        let t0 = std::time::Instant::now();
        for &i in &order {
            let len = live_ids[i].len().min(cap);
            if let Some(&first) = bucket.first() {
                let first_len = live_ids[first].len().min(cap);
                // Close the bucket when the new row's length exceeds the
                // bucket's first row by >12.5% (or >64 tokens) — beyond that
                // the padding waste outweighs the bigger GEMM.
                if len * 8 > first_len * 9 + 64 {
                    let sub: Vec<Vec<u32>> = bucket.iter().map(|&j| live_ids[j].clone()).collect();
                    let rows = forward_sub_batch(&*self.arch, &self.device, cap, &sub)?;
                    rows_sorted.extend(rows);
                    bucket.clear();
                }
            }
            bucket.push(i);
        }
        if !bucket.is_empty() {
            let sub: Vec<Vec<u32>> = bucket.iter().map(|&j| live_ids[j].clone()).collect();
            let rows = forward_sub_batch(&*self.arch, &self.device, cap, &sub)?;
            rows_sorted.extend(rows);
        }
        // Scatter back to the caller's order (rows_sorted is in sorted order).
        let mut result: Vec<Vec<f32>> = vec![Vec::new(); n];
        for (row, &orig_idx) in rows_sorted.into_iter().zip(order.iter()) {
            result[live_orig[orig_idx]] = row;
        }
        for i in zero_rows {
            result[i] = vec![0.0; self.embed_dim];
        }
        log::info!(
            "[RAG] embed_batch: {} chunks on {:?} in {}ms (length-bucketed)",
            n,
            self.device,
            t0.elapsed().as_millis()
        );
        Ok(result)
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
        // T5 / XLM-RoBERTa-style SentencePiece Unigram (nomic-embed-text-v2-moe).
        "t5" => build_t5_tokenizer(md),
        // Gemma SentencePiece BPE (`gemma`/`gemma2`/`gemma3`/`gemma4`): vocab +
        // merges + a `▁` Metaspace pre-tokenizer + byte-fallback (`<0xNN>` for
        // any unknown byte). Token content uses the `▁` metaspace prefix and the
        // vocab ships 256 `<0x00>`..`<0xFF>` byte tokens (token_type=BYTE), so
        // `byte_fallback` reconstructs the exact Gemma tokenization from the
        // GGUF metadata alone (no tokenizer.json needed). Granite Embedding
        // 311M Multilingual R2 ships `gemma4`; the 97M ships `gpt2`.
        "gemma" | "gemma2" | "gemma3" | "gemma4" => build_gemma_tokenizer(md),
        _ => Err(anyhow!(
            "no tokenizer.json and GGUF tokenizer model '{}' is not reconstructable (supported: gpt2/qwen2/llama3/llama/t5/gemma) - ship a tokenizer.json",
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

/// Gemma SentencePiece BPE (`gemma`/`gemma2`/`gemma3`/`gemma4`): a BPE model
/// (vocab + merges, same as the gpt2 builder) BUT with a `▁` Metaspace
/// pre-tokenizer and byte-fallback enabled. Gemma's vocab stores tokens with
/// the `▁` metaspace prefix and ships 256 `<0x00>`..`<0xFF>` byte tokens, so
/// `byte_fallback` reproduces Gemma's SentencePiece-with-byte-fallback behavior:
/// any byte not covered by a vocab token is emitted as its `<0xNN>` token
/// instead of `<unk>`. bos/eos are still added manually by the embedder per the
/// GGUF's `add_bos_token`/`add_eos_token`.
///
/// Verified against the `granite-embedding-311M-multilingual-r2` GGUF
/// (`tokenizer.ggml.model = gemma4`): produces correct in-range ids for
/// EN/CJK/AR samples (e.g. `今天天气真好` -> `▁今天 天气 真 好`).
fn build_gemma_tokenizer(md: &std::collections::HashMap<String, Value>) -> Result<Tokenizer> {
    let tokens = meta_string_array(md, "tokenizer.ggml.tokens")?;
    let merges_raw = meta_string_array(md, "tokenizer.ggml.merges")?;
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
        // Gemma maps unknown bytes to their <0xNN> token (the vocab ships all
        // 256) rather than emitting <unk>.
        .byte_fallback(true)
        .build()
        .map_err(|e| anyhow!("build Gemma BPE from gguf: {}", e))?;
    let mut tok = Tokenizer::new(bpe);
    // ▁ metaspace, always prepend (Gemma convention).
    tok.with_pre_tokenizer(Some(Metaspace::new('▁', PrependScheme::Always, true)));
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

/// T5 / XLM-RoBERTa-style tokenizer (GGUF `tokenizer.ggml.model == "t5"`):
/// a SentencePiece Unigram model (same as `llama` SPM) BUT with a
/// `Precompiled` normalizer built from the GGUF's `precompiled_charsmap`
/// (NFKC-style normalization baked in) and a `Sequence[WhitespaceSplit,
/// Metaspace]` pre-tokenizer. Without the Precompiled normalizer, multilingual
/// text tokenizes wrong. Used by nomic-embed-text-v2-moe.
///
/// Verified byte-identical to the official `tokenizer.json` on the nomic v2 MoE
/// vocab (250048 tokens incl. 46 `score=-10000` padding tokens) across
/// EN/CJK/ES/EL/RU/JA/AR samples + the `search_query:`/`search_document:`
/// prefixes - so the GGUF is self-contained (no tokenizer.json needed).
fn build_t5_tokenizer(md: &std::collections::HashMap<String, Value>) -> Result<Tokenizer> {
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
        .map_err(|e| anyhow!("build Unigram (t5) from gguf: {}", e))?;
    let mut tok = Tokenizer::new(unigram);

    // Precompiled SentencePiece charsmap normalizer. The GGUF stores it as an
    // array of U8 bytes (e.g. 237539 for nomic); `spm_precompiled::Precompiled`
    // parses the blob. If absent, skip (rare - multilingual normalization would
    // be off, but Unigram + Metaspace still tokenizes).
    if let Some(Value::Array(items)) = md.get("tokenizer.ggml.precompiled_charsmap") {
        let bytes: Vec<u8> = items
            .iter()
            .map(|v| match v {
                Value::U8(b) => Ok(*b),
                _ => Err(anyhow!("precompiled_charsmap element is not U8")),
            })
            .collect::<Result<Vec<_>>>()?;
        if !bytes.is_empty() {
            let precompiled = Precompiled::from(&bytes)
                .map_err(|e| anyhow!("build Precompiled charsmap from gguf: {}", e))?;
            // with_normalizer returns Result (it refreshes added-token
            // normalization); propagate a failure rather than silently dropping
            // the normalizer (multilingual text would tokenize wrong without it).
            tok.with_normalizer(Some(precompiled))
                .map_err(|e| anyhow!("set t5 Precompiled normalizer: {}", e))?;
        }
    }

    // Pre-tokenizer: WhitespaceSplit then Metaspace (▁, always prepend) -
    // matches the official tokenizer.json's Sequence pre-tokenizer.
    let seq = Sequence::new(vec![
        WhitespaceSplit.into(),
        Metaspace::new('▁', PrependScheme::Always, true).into(),
    ]);
    tok.with_pre_tokenizer(Some(seq));
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
    // Linux & Windows: CPU-only — candle has no usable GPU backend here (no
    // DirectML on Windows; Linux CUDA is disabled, see Cargo.toml). Falls
    // through to None -> CPU.
    None
}

#[cfg(test)]
mod semantic_probe_tests {
    use super::*;
    use crate::mv::embedder::Embedder;

    /// 真实加载捆绑的 granite-97m-r2 模型，测量「需要办的事情 / 等待办的事情 /
    /// 待办事项」对含「待办事项」chunk 的余弦相似度（用户实测检索失败场景，
    /// 2026-10-12）。`#[ignore]`（需要真实 GGUF + 模型加载 ~10s）：
    /// `cargo test --lib semantic_probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn granite_97m_semantic_similarity_probe() {
        let gguf = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("runtimes/rag/model/granite/97m-r2/model.gguf");
        if !gguf.exists() {
            eprintln!("skipping: {} not present", gguf.display());
            return;
        }
        let mut emb = GgufEmbedder::load_with_user_platform(&gguf, Some(crate::mv::embedder::Platform::Cpu))
            .expect("load granite 97m-r2");

        let chunk = "本周待办事项：1. 完成版本发布 2. 更新文档 3. 修复登录问题。";
        let queries = [
            "待办事项",       // 字面全命中（词法上限）
            "等待办的事情",   // bigram 部分重叠
            "需要办的事情",   // 零字面重叠，纯语义
            "需要处理的工作", // 同义改写
            "今天天气很好",   // 无关对照组
        ];
        let cv = emb.embed(chunk).expect("embed chunk");
        println!("\nchunk = {chunk:?}");
        println!("dim = {}", emb.embed_dim());
        for q in queries {
            let qv = emb.embed(q).expect("embed query");
            let dot: f32 = qv.iter().zip(&cv).map(|(a, b)| a * b).sum();
            println!("cos({q:>10?}, chunk) = {dot:.4}");
        }

        // 模拟真实语料排名：chunk 混入 8 个干扰 chunk，看「需要办的事情」
        // 能否排进前 5（用户场景）。
        let distractors = [
            "项目周会纪要：评审了数据库迁移方案，下周开始压测。",
            "Python 数据处理脚本：读取 CSV，清洗缺失值，输出统计图表。",
            "服务器运维手册：Nginx 反向代理配置与证书续期步骤。",
            "会议记录：市场部 Q4 预算讨论，广告投放渠道优化方案。",
            "前端组件文档：Table 分页、排序与虚拟滚动的使用说明。",
            " Reiseplan: Flug nach Tokio, Hotel buchen, Visa beantragen.",
            "机器学习笔记：交叉验证、过拟合与正则化超参数调优。",
            "git 工作流：feature branch 命名规范与 code review 流程。",
        ];
        let mut texts: Vec<&str> = vec![chunk];
        texts.extend(distractors.iter());
        let vecs = emb.embed_batch(&texts).expect("embed batch");
        println!("\n=== ranking probe (query embedding vs 9 chunks) ===");
        for q in ["需要办的事情", "等待办的事情", "Query: 需要办的事情"] {
            let qv = emb.embed(q).expect("embed q");
            let mut scored: Vec<(f32, &str)> = texts
                .iter()
                .zip(&vecs)
                .map(|(t, v)| (qv.iter().zip(v).map(|(a, b)| a * b).sum::<f32>(), *t))
                .collect();
            scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
            println!("\nquery = {q:?}");
            for (i, (s, t)) in scored.iter().take(4).enumerate() {
                let short: String = t.chars().take(24).collect();
                println!("  #{i} {s:.4}  {short}");
            }
        }
    }
    /// 打开用户真实 lancedb 索引 + 真实模型，输出「等待办的事情」的完整
    /// 排名（doc_name + cos + chunk 片段），并标注目标文档
    /// 181db8ce（迭代评审-消息配置与待办事项.md）的真实排位。
    /// `#[ignore]`：需要真实索引与模型；应用运行中并发只读一般安全。
    /// `cargo test --lib real_index_probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_index_ranking_probe() {
        use crate::mv::embedder::Embedder;
        use lancedb::connect;
        use lancedb::query::{ExecutableQuery, QueryBase};

        let gguf = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("runtimes/rag/model/granite/97m-r2/model.gguf");
        if !gguf.exists() {
            eprintln!("skipping: model not present");
            return;
        }
        let db_dir = std::env::var("PROBE_DB").unwrap_or_else(|_| {
            format!(
                "{}/Library/Application Support/app.mcphub.desktop/rag/lancedb",
                std::env::var("HOME").unwrap()
            )
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let mut emb = GgufEmbedder::load_with_user_platform(
                &gguf,
                Some(crate::mv::embedder::Platform::Cpu),
            )
            .expect("load model");
            let db = connect(&db_dir).execute().await.expect("open lancedb");
            let table = db.open_table("rag_chunk").execute().await.expect("open table");
            println!("total rows: {}", table.count_rows(None).await.unwrap());

            // ============ 前后对比 benchmark（真实数据，真实标题） ============
            // 8 篇真实文档（含目标），各取前 3 个 700 字块，按两种格式嵌入：
            //   OLD: 裸 chunk（当前索引的格式）
            //   NEW: "Title: {真实标题} | {chunk}"（新模板）+ "Query: {q}"
            // 排名 = 同格式内部对比（旧查询 vs 旧文档 / 新查询 vs 新文档），
            // 完全等价于重索引前后的真实差异（mermaid 垃圾块除外——它们
            // 无标题概念，NEW 格式下按 "Title: mermaid.min.js | ..." 处理）。
            let docs: &[(&str, &str, &str)] = &[
                ("181db8ce-206a-4296-a452-09612986f8ea", "迭代评审-消息配置与待办事项.md", "消息配置与待办事项 - 迭代评审文档"),
                ("38c94be0-a5c2-4ac8-9f5b-eb1e88c133f3", "镁信入组状态.md", "镁信入组状态 — 业务理解"),
                ("7c13b5c9-3572-4f90-8951-6ee95befdeb2", "hislite-centre-bff-architecture-review.md", "Hislite 与 Centre-BFF 整体架构"),
                ("21482dc7-b503-4431-952c-afd7576db9c0", "直连门诊_多机构多门诊多厂商_架构改造设计.md", "直连门诊多机构多门诊多厂商架构改造设计"),
                ("95e0e268-bedc-4674-aa12-a43fbddf3875", "A流程.md", "A流程"),
                ("2c671ee4-fe92-4440-9981-5608d24c6240", "rws架构设计说明.md", "rws 架构设计说明"),
                ("e5ee555d-b023-4b0c-b351-00ea511be57d", "门诊排班V2-技术评审文档.md", "门诊排班V2 - 技术评审文档"),
                ("daf1be10-1d6f-48e1-ae09-ff33da9c4340", "YNW易诺卫用户中心整体架构设计.md", "YNW易诺卫用户中心整体架构设计"),
                ("c6e915e5-6a3b-4379-913f-80baf3fb0d71", "mermaid.min.js", "mermaid.min.js"),
            ];
            let files_dir = format!(
                "{}/Library/Application Support/app.mcphub.desktop/rag/files",
                std::env::var("HOME").unwrap()
            );
            let query = "等待做的事情，这个功能的设计";

            // 收集所有 (doc_idx, chunk_text)；每篇取前 3 块（700 字步进 600）
            let mut all_chunks: Vec<(usize, String)> = Vec::new();
            for (di, (id, _, _)) in docs.iter().enumerate() {
                let base = format!("{files_dir}/{id}");
                let path = [".md", ".js", ".txt", ".log"]
                    .iter()
                    .map(|e| format!("{base}{e}"))
                    .find(|p| std::path::Path::new(p).exists())
                    .expect("doc content file");
                let Ok(content) = std::fs::read_to_string(&path) else {
                    println!("!! cannot read {}", path);
                    continue;
                };
                let chars: Vec<char> = content.chars().collect();
                let mut i = 0;
                let mut taken = 0;
                while i < chars.len() && taken < 2 {
                    let end = (i + 700).min(chars.len());
                    let piece: String = chars[i..end].iter().collect();
                    if piece.trim().len() > 20 {
                        all_chunks.push((di, piece));
                        taken += 1;
                    }
                    i += 600;
                }
            }
            println!("benchmark chunks: {}", all_chunks.len());

            // OLD 格式：query 裸文本，chunk 裸文本
            let qv_old = emb.embed(query).expect("embed q old");
            // NEW 格式：Query: {q}，Title: {title} | {content}
            let q_new = format!("Query: {query}");
            let qv_new = emb.embed(&q_new).expect("embed q new");

            let mut old_scored: Vec<(f32, usize, usize)> = Vec::new();
            let mut new_scored: Vec<(f32, usize, usize)> = Vec::new();
            for (ci, (di, text)) in all_chunks.iter().enumerate() {
                let v_old = emb.embed(text).expect("embed old");
                let cos_old: f32 = qv_old.iter().zip(&v_old).map(|(a, b)| a * b).sum();
                old_scored.push((cos_old, *di, ci));
                let title = docs[*di].2;
                let rendered =
                    crate::mv::embedder::render_doc_embed_text("Title: {title} | {content}", title, text, Some("|"));
                let v_new = emb.embed(&rendered).expect("embed new");
                let cos_new: f32 = qv_new.iter().zip(&v_new).map(|(a, b)| a * b).sum();
                new_scored.push((cos_new, *di, ci));
            }
            let name = |di: usize| docs[di].1;
            for (label, mut scored) in [("OLD (无前缀, 当前索引格式)", old_scored), ("NEW (Title: {title} | {content} + Query: 前缀)", new_scored)] {
                scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
                println!("\n===== {label} =====");
                let mut target_rank = None;
                for (r, (s, di, ci)) in scored.iter().enumerate() {
                    if *di == 0 && target_rank.is_none() {
                        target_rank = Some(r + 1);
                    }
                    if r < 20 {
                        let short: String = all_chunks[*ci].1.chars().take(30).collect();
                        println!("#{:<2} cos={s:.4} [{:<28}] {} …", r + 1, name(*di), short);
                    }
                }
                let best_target = scored
                    .iter()
                    .filter(|(_, di, _)| *di == 0)
                    .map(|(s, _, _)| *s)
                    .fold(f32::MIN, f32::max);
                println!("TARGET 最佳 chunk cos={best_target:.4}, 排名 = {target_rank:?}");
            // ===== 生产管线优化验证：真实索引 top-100 + 惩罚公式 =====
            let qv_cur = emb.embed("等待办的事情").expect("embed cur");
            let mut stream = table
                .query()
                .nearest_to(qv_cur.clone())
                .expect("nearest")
                .limit(100)
                .execute()
                .await
                .expect("search100");
            use futures_util::TryStreamExt;
            use lancedb::arrow::arrow::array::{Float32Array as FA, Int64Array as IA};
            fn gcol(batch: &lancedb::arrow::arrow_array::RecordBatch, col: &str, i: usize) -> String {
                lancedb::arrow::arrow::array::AsArray::as_string::<i32>(
                    batch.column_by_name(col).unwrap(),
                )
                .value(i)
                .to_string()
            }
            let mut rows: Vec<(f32, String, i64, String, String)> = Vec::new();
            while let Some(batch) = stream.try_next().await.unwrap() {
                let n = batch.num_rows();
                for i in 0..n {
                    let d = batch
                        .column_by_name("_distance")
                        .unwrap()
                        .as_any()
                        .downcast_ref::<FA>()
                        .unwrap()
                        .value(i);
                    let ci = batch
                        .column_by_name("chunk_index")
                        .unwrap()
                        .as_any()
                        .downcast_ref::<IA>()
                        .unwrap()
                        .value(i);
                    rows.push((1.0 - d, gcol(&batch, "doc_id", i), ci, gcol(&batch, "doc_name", i), gcol(&batch, "chunk_text", i)));
                }
            }
            let mut raw = rows.clone();
            raw.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
            println!("\n===== REALIDX-RAW top-8 =====");
            for (r, (s, _, _, nm, tx)) in raw.iter().take(8).enumerate() {
                let short: String = tx.chars().take(28).collect();
                println!("#{:<2} cos={s:.4} {:<26} {}", r + 1, nm, short);
            }
            let penalty = |t: &str| -> f32 {
                let total = t.chars().count().max(1);
                let cjk = t.chars().filter(|c| crate::services::fts_service::is_cjk_pub(*c)).count();
                let ratio = cjk as f32 / total as f32;
                if ratio >= 0.1 { 1.0 } else { 0.55 + 0.45 * (ratio / 0.1) }
            };
            let mut pen = rows.clone();
            for row in pen.iter_mut() {
                row.0 *= penalty(&row.4);
            }
            pen.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
            println!("\n===== REALIDX-PENALIZED top-8 =====");
            for (r, (s, _, _, nm, tx)) in pen.iter().take(8).enumerate() {
                let short: String = tx.chars().take(28).collect();
                println!("#{:<2} cos={s:.4} {:<26} {}", r + 1, nm, short);
            }
            let traw = raw.iter().position(|(_, id, _, _, _)| id.starts_with("181db8ce")).map(|p| p + 1);
            let tpen = pen.iter().position(|(_, id, _, _, _)| id.starts_with("181db8ce")).map(|p| p + 1);
            println!("TARGET-rank raw={traw:?} penalized={tpen:?}");
            }
        });
    }

    /// 重索引后存储向量格式验证：目标块向量应与 NEW(Title 模板)渲染一致。
    /// `cargo test --lib stored_vec_format -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn stored_vec_format_probe() {
        use crate::mv::embedder::Embedder;
        use futures_util::TryStreamExt;
        use lancedb::connect;
        use lancedb::query::{ExecutableQuery, QueryBase};
        let gguf = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("runtimes/rag/model/granite/97m-r2/model.gguf");
        if !gguf.exists() {
            eprintln!("skipping: model not present");
            return;
        }
        let db_dir = format!(
            "{}/Library/Application Support/app.mcphub.desktop/rag/lancedb",
            std::env::var("HOME").unwrap()
        );
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let doc_id_prefix = std::env::var("PROBE_DOC").unwrap_or_else(|_| "181db8ce".into());
            let mut m = GgufEmbedder::load_with_user_platform(
                &gguf,
                Some(crate::mv::embedder::Platform::Cpu),
            )
            .expect("load model");
            let db = connect(&db_dir).execute().await.expect("open lancedb");
            let table = db.open_table("rag_chunk").execute().await.expect("table");
            let mut stream = table.query().limit(30000).execute().await.expect("scan");
            use lancedb::arrow::arrow::array::{AsArray, FixedSizeListArray, Float32Array as FA};
            let mut stored: Option<(String, Vec<f32>)> = None;
            while let Some(b) = stream.try_next().await.unwrap() {
                let idc = b
                    .column_by_name("doc_id")
                    .unwrap()
                    .as_string::<i32>();
                let txc = b
                    .column_by_name("chunk_text")
                    .unwrap()
                    .as_string::<i32>();
                let emb = b
                    .column_by_name("embedding")
                    .unwrap()
                    .as_any()
                    .downcast_ref::<FixedSizeListArray>()
                    .unwrap();
                for i in 0..b.num_rows() {
                    if idc.value(i).starts_with(&doc_id_prefix) {
                        let vals = emb.value(i);
                        let fa = vals.as_any().downcast_ref::<FA>().unwrap();
                        let v: Vec<f32> = (0..fa.len()).map(|j| fa.value(j)).collect();
                        stored = Some((txc.value(i).to_string(), v));
                        break;
                    }
                }
                if stored.is_some() {
                    break;
                }
            }
            let (text, sv) = stored.expect("no target chunk");
            let title = if doc_id_prefix == "181db8ce" {
                "消息配置与待办事项 - 迭代评审文档".to_string()
            } else {
                // README's real .meta title (read from the full-id meta file)
                let meta_path = std::fs::read_dir(format!(
                    "{}/Library/Application Support/app.mcphub.desktop/rag/files",
                    std::env::var("HOME").unwrap()
                ))
                .ok()
                .and_then(|rd| {
                    rd.flatten()
                        .map(|e| e.path())
                        .find(|p| {
                            p.extension().and_then(|e| e.to_str()) == Some("meta")
                                && p.file_name()
                                    .and_then(|n| n.to_str())
                                    .map(|n| n.starts_with(&doc_id_prefix))
                                    .unwrap_or(false)
                        })
                });
                meta_path
                    .and_then(|p| std::fs::read(p).ok())
                    .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                    .and_then(|v| v.get("title").and_then(|t| t.as_str()).map(String::from))
                    .filter(|t| !t.trim().is_empty())
                    .unwrap_or_else(|| "README".to_string())
            };
            let new = crate::mv::embedder::render_doc_embed_text(
                "Title: {title} | {content}",
                &title,
                &text,
                Some("|"),
            );
            let vo = m.embed(&text).expect("embed old");
            let vn = m.embed(&new).expect("embed new");
            let cos = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
            println!("cos(stored, OLD bare) = {:.4}", cos(&sv, &vo));
            println!("cos(stored, NEW title) = {:.4}", cos(&sv, &vn));
            println!("cos(OLD, NEW)          = {:.4}", cos(&vo, &vn));
            // Same-backend (Metal/Auto) reference to remove CPU-vs-GPU numeric noise
            if let Ok(mut mg) = GgufEmbedder::load_with_user_platform(
                &gguf,
                Some(crate::mv::embedder::Platform::Gpu),
            ) {
                let go = mg.embed(&text).expect("gpu old");
                let gn = mg.embed(&new).expect("gpu new");
                println!("cos(stored, GPU OLD) = {:.4}", cos(&sv, &go));
                println!("cos(stored, GPU NEW) = {:.4}", cos(&sv, &gn));
                let tnew = crate::mv::embedder::render_doc_embed_text("Title: {title} | {content}", &title, &text, Some("|"));
                let gt = mg.embed(&tnew).expect("gpu realtitle");
                println!("cos(stored, GPU REALTITLE) = {:.4}", cos(&sv, &gt));
                println!("REALTITLE text head: {:?}", tnew.chars().take(60).collect::<String>());
            }
        });
    }

    /// 查询 2/3 回归定位：生产一致格式（Query: 前缀）下，真实索引纯向量
    /// 通道中目标文档 181db8ce 的排名。`cargo test --lib query23_probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_index_query23_probe() {
        use crate::mv::embedder::Embedder;
        use futures_util::TryStreamExt;
        use lancedb::connect;
        use lancedb::query::{ExecutableQuery, QueryBase};

        let gguf = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("runtimes/rag/model/granite/97m-r2/model.gguf");
        if !gguf.exists() {
            eprintln!("skipping: model not present");
            return;
        }
        let db_dir = std::env::var("PROBE_DB").unwrap_or_else(|_| {
            format!(
                "{}/Library/Application Support/app.mcphub.desktop/rag/lancedb",
                std::env::var("HOME").unwrap()
            )
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let mut emb = GgufEmbedder::load_with_user_platform(
                &gguf,
                Some(crate::mv::embedder::Platform::Cpu),
            )
            .expect("load model");
            let db = connect(&db_dir).execute().await.expect("open lancedb");
            let table = db.open_table("rag_chunk").execute().await.expect("open table");
            use lancedb::arrow::arrow::array::{Float32Array as FA, Int64Array as IA};
            fn gstr(b: &lancedb::arrow::arrow_array::RecordBatch, col: &str, i: usize) -> String {
                lancedb::arrow::arrow::array::AsArray::as_string::<i32>(
                    b.column_by_name(col).unwrap(),
                )
                .value(i)
                .to_string()
            }
            for q in ["需要办的事情", "等待做的事情，这个功能的设计"] {
                let qtext = crate::mv::embedder::render_query_embed_text("Query: {query}", q);
                let qv = emb.embed(&qtext).expect("embed q");
                let mut stream = table
                    .query()
                    .nearest_to(qv.clone())
                    .expect("nearest")
                    .limit(300)
                    .execute()
                    .await
                    .expect("search");
                let mut rows: Vec<(f32, String, String, String)> = Vec::new();
                while let Some(batch) = stream.try_next().await.unwrap() {
                    let n = batch.num_rows();
                    for i in 0..n {
                        let d = batch
                            .column_by_name("_distance")
                            .unwrap()
                            .as_any()
                            .downcast_ref::<FA>()
                            .unwrap()
                            .value(i);
                        rows.push((
                            1.0 - d,
                            gstr(&batch, "doc_id", i),
                            gstr(&batch, "doc_name", i),
                            gstr(&batch, "chunk_text", i),
                        ));
                    }
                }
                rows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
                println!("\n===== q={q:?} (Query: 前缀) =====");
                for (r, (s, id, nm, tx)) in rows.iter().take(6).enumerate() {
                    let short: String = tx.chars().take(26).collect();
                    println!("#{:<2} cos={s:.4} {:<26} {}", r + 1, nm, short);
                }
                let target_first = rows.iter().position(|(_, id, _, _)| id.starts_with("181db8ce"));
                match target_first {
                    Some(p) => {
                        let (s, _, _, tx) = &rows[p];
                        let short: String = tx.chars().take(26).collect();
                        println!("TARGET rank={} cos={s:.4}  {}", p + 1, short);
                    }
                    None => println!("TARGET 不在前 300！"),
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Load the real Granite Embedding 311M GGUF (whose GGUF tags
    /// `tokenizer.ggml.model = "gemma4"`), reconstruct the tokenizer from its
    /// metadata via `build_gguf_tokenizer`, and verify it tokenizes correctly.
    /// Guards against the regression where switching to the 311m model errored
    /// `gemma4 is not reconstructable`. `#[ignore]` because it needs the bundled
    /// GGUF on disk; run with
    /// `cargo test --lib rag::gguf::tests::granite_311m_gemma4_tokenizer -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn granite_311m_gemma4_tokenizer() {
        let gguf = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("runtimes/rag/model/granite/311m/granite-embedding-311M-multilingual-r2-Q8_0.gguf");
        if !gguf.exists() {
            eprintln!("skipping: {} not present", gguf.display());
            return;
        }
        let mut file = std::fs::File::open(&gguf).expect("open gguf");
        let content = Content::read(&mut file).expect("read gguf");
        let md = &content.metadata;

        // The dispatch used to error here: "gemma4 is not reconstructable".
        assert_eq!(meta_string(md, "general.architecture").as_deref(), Some("modern-bert"));
        assert_eq!(meta_string(md, "tokenizer.ggml.model").as_deref(), Some("gemma4"));
        let tokenizer = build_gguf_tokenizer(md).expect("build gemma4 tokenizer");

        let vocab_size = meta_string_array(md, "tokenizer.ggml.tokens").unwrap().len();

        // Each sample encodes to ids that are all in-range, and the known word
        // "machine" / "learning" round-trip as single metaspace-prefixed tokens.
        // (These ids were cross-checked against the same reconstruction in Python
        // `tokenizers` — they are the ids the real GGUF tokenizer produces.)
        let cases: &[(&str, &[&str])] = &[
            ("machine learning model", &["▁machine", "▁learning", "▁model"]),
            ("Rust lifetime error", &["▁Rust", "▁lifetime", "▁error"]),
        ];
        for &(text, expected_tokens) in cases {
            let enc = tokenizer.encode(text, false).expect("encode");
            let ids: Vec<u32> = enc.get_ids().to_vec();
            assert!(
                ids.iter().all(|&id| (id as usize) < vocab_size),
                "{text:?}: id out of range ({:?}; vocab={vocab_size})",
                ids
            );
            let toks: Vec<&str> = enc.get_tokens().iter().map(|s| s.as_str()).collect();
            assert_eq!(
                toks, expected_tokens,
                "{text:?}: tokens {toks:?} != expected {expected_tokens:?}"
            );
        }

        // Multilingual byte-fallback path: CJK + Arabic must all encode to
        // in-range ids (no out-of-vocab <unk>, byte-fallback covers every byte).
        for text in ["今天天气真好", "你好,世界!", "مرحبا بالعالم"] {
            let enc = tokenizer.encode(text, false).expect("encode");
            let ids: Vec<u32> = enc.get_ids().to_vec();
            assert!(
                ids.iter().all(|&id| (id as usize) < vocab_size),
                "{text:?}: id out of range ({:?})",
                ids
            );
            assert!(!ids.is_empty(), "{text:?}: produced no tokens");
        }
    }
}
