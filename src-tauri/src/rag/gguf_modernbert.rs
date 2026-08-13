//! GGUF architecture strategy for `modern-bert` (IBM Granite Embedding 97M
//! Multilingual R2, and any other `modern-bert` GGUF embedding).
//!
//! ## Architecture (verified against the GGUF metadata + tensor layout AND the
//! HF reference: `ibm-granite/granite-embedding-97m-multilingual-r2`'
//! `config.json` / `1_Pooling/config.json` / `modules.json` + transformers'
//! `modeling_modern_bert.py`)
//! 12-layer BERT-style encoder, **pre-norm** RMSNorm (standard, NO +1 weight
//! shift — the GGUF has weight-only norm tensors and metadata key
//! `attention.layer_norm_rms_epsilon`; HF `norm_bias=false`), SiLU GLU FFN
//! (`Wo(silu(Wi(x)_input) * Wi(x)_gate)` — first half = `input` activated,
//! second = `gate` passthrough), full RoPE over the whole `head_dim` (NeoX
//! rotate-half, identical to Gemma3's `apply_rope`), bidirectional (no causal
//! mask). Layer 0's `attn_norm` is `Identity` (the GGUF has no
//! `blk.0.attn_norm.weight` tensor, matching HF's `nn.Identity()`).
//!
//! Sliding-window attention: every `sliding_window_pattern`-th layer (pattern
//! = 3 → layers `i%3==0`: 0,3,6,9) is FULL attention; the rest attend within
//! a bidirectional `±sliding_window` (128) band. Sliding layers use RoPE base
//! `rope.freq_base_swa` (160000); full layers use `rope.freq_base` (150000) —
//! the HF `local_rope_theta` / `global_rope_theta` split.
//!
//! Pooling = **CLS** (first token = bos, since `add_bos_token=true`). The GGUF
//! `modern-bert.pooling_type=2` = CLS, and HF `1_Pooling/config.json` confirms
//! `pooling_mode_cls_token=true`, `pooling_mode_mean_tokens=false`. `modules.json`
//! ends with `2_Normalize` → L2-normalize after pooling (the app always L2-
//! normalizes anyway since vectordb uses cosine distance).
//!
//! ## Tensor layout
//! candle returns logical shapes (GGUF stores dims reversed on disk): 2D
//! linears `[out, in]`, `token_embd` `[vocab, hidden]` (same convention as
//! Gemma/Qwen/nomic/lfm2). Linears are transposed to `[in, out]` at load so
//! the forward does plain `x @ W` via `gguf_gemma::linear`. No bias tensors
//! anywhere (HF `attention_bias=mlp_bias=norm_bias=false`).
//!   token_embd.weight     [vocab, hidden]   (used directly, NO transpose)
//!   token_embd_norm.weight [hidden]          (RMSNorm over the embedding)
//!   output_norm.weight     [hidden]          (final RMSNorm)
//!   blk.N.attn_norm.weight [hidden]          (None for i==0 — layer 0 identity)
//!   blk.N.ffn_norm.weight  [hidden]          (pre-FFN RMSNorm)
//!   blk.N.attn_qkv.weight  [out=3*hidden, in=hidden] → [in, out]   (combined QKV)
//!   blk.N.attn_output.weight [out=hidden, in=hidden] → [in, out]
//!   blk.N.ffn_up.weight    [out=2*ffn, in=hidden] → [in, out]   (Wi: hidden→2*ffn)
//!   blk.N.ffn_down.weight  [out=hidden, in=ffn] → [in, out]     (Wo: ffn→hidden)
//!
//! ## ⚠ Correctness caveat
//! Compiles clean. The forward matches the HF `modeling_modern_bert.py`
//! reference (pre-norm block, full-head_dim RoPE, GLU, CLS-pool + L2) and all
//! architecture parameters are confirmed against the HF config. NOT numerically
//! validated here (torch isn't installed in this sandbox) — validate cosine
//! ~1.0 against `ibm-granite/granite-embedding-97m-multilingual-r2` once
//! running (a deferred `#[ignore]` probe, see the plan).

use std::io::{Read, Seek};

use anyhow::{anyhow, Result};
use candle_core::quantized::gguf_file::Content;
use candle_core::{Device, Module, Tensor};
use candle_nn::{ops, RmsNorm};

use crate::rag::gguf_gemma::{
    apply_rope, attn_bias, l2_normalize, linear, sliding_window_mask, GgufArch,
};

/// One ModernBert block: pre-norm attention (combined QKV + RoPE) + residual,
/// then pre-norm GLU FFN + residual. Layer 0 has `attn_norm = None` (identity).
struct ModernBertBlock {
    attn_norm: Option<RmsNorm>,
    ffn_norm: RmsNorm,
    /// Combined QKV `[in=hidden, out=3*hidden]`.
    qkv_w: Tensor,
    /// Attention output `[in=hidden, out=hidden]`.
    out_w: Tensor,
    /// Wi `[in=hidden, out=2*ffn]`.
    ffn_up_w: Tensor,
    /// Wo `[in=ffn, out=hidden]`.
    ffn_down_w: Tensor,
    /// Sliding-window layer? (false = full attention). `i % pattern != 0`.
    is_sliding: bool,
}

/// `modern-bert` encoder for embeddings (Granite Embedding 97M Multilingual R2).
pub struct ModernBertArch {
    token_embd: Tensor,
    token_embd_norm: RmsNorm,
    blocks: Vec<ModernBertBlock>,
    output_norm: RmsNorm,
    hidden: usize,
    n_head: usize,
    head_dim: usize,
    /// feed-forward (intermediate) size — half of `ffn_up`'s output dim.
    ffn: usize,
    sliding_window: usize,
    rope_full: f32,
    rope_swa: f32,
    /// Pooling type from the GGUF (`modern-bert.pooling_type`): 2 = CLS.
    pooling_type: u32,
    max_context: u32,
    device: Device,
}

impl ModernBertArch {
    /// Build from an already-read `Content` (the caller opens the file + reads
    /// Content ONCE and shares it with the tokenizer builder + the arch).
    pub fn from_content<R: Read + Seek>(
        file: &mut R,
        content: &Content,
        device: &Device,
    ) -> Result<Self> {
        let md = &content.metadata;
        let arch = md
            .get("general.architecture")
            .and_then(|v| v.to_string().ok())
            .ok_or_else(|| anyhow!("missing gguf key general.architecture"))?;
        let pfx = format!("{}.", arch);

        let get_u32 = |key: &str, default: Option<usize>| -> Result<usize> {
            match md.get(key).map(|v| v.to_u32()) {
                Some(Ok(v)) => Ok(v as usize),
                Some(Err(e)) => Err(anyhow!("read {} as u32: {}", key, e)),
                None => default.ok_or_else(|| anyhow!("missing gguf key {}", key)),
            }
        };
        let get_f32 = |key: &str, default: f32| -> f32 {
            md.get(key).and_then(|v| v.to_f32().ok()).unwrap_or(default)
        };

        let hidden = get_u32(&format!("{}embedding_length", pfx), None)?; // 384
        let block_count = get_u32(&format!("{}block_count", pfx), None)?; // 12
        let n_head = get_u32(&format!("{}attention.head_count", pfx), None)?; // 12
        let head_dim = hidden / n_head; // 32
        let ffn = get_u32(&format!("{}feed_forward_length", pfx), None)?; // 1536
        let sliding_window =
            get_u32(&format!("{}attention.sliding_window", pfx), Some(128))?;
        let pattern = get_u32(&format!("{}attention.sliding_window_pattern", pfx), Some(3))?;
        let rope_full = get_f32(&format!("{}rope.freq_base", pfx), 150_000.0);
        let rope_swa = get_f32(&format!("{}rope.freq_base_swa", pfx), rope_full);
        let eps = get_f32(&format!("{}attention.layer_norm_rms_epsilon", pfx), 1e-5) as f64;
        let pooling_type =
            get_u32(&format!("{}pooling_type", pfx), Some(2))? as u32; // 2 = CLS
        let max_context = get_u32(&format!("{}context_length", pfx), Some(32768))? as u32;

        // token_embd: candle returns logical [vocab, hidden] (use directly, NO
        // transpose — Tensor::embedding indexes dim 0 = vocab). dequantize to
        // f32 so index_select is contiguous (and, on CPU, lets accelerate/AMX
        // engage on the downstream linears).
        let token_embd = content
            .tensor(file, "token_embd.weight", device)?
            .dequantize(device)?;
        let token_embd_norm = rms_norm(file, content, "token_embd_norm", device, eps)?;
        let output_norm = rms_norm(file, content, "output_norm", device, eps)?;

        let mut blocks = Vec::with_capacity(block_count);
        for i in 0..block_count {
            // Layer 0's attn_norm is Identity — the GGUF has no
            // `blk.0.attn_norm.weight` tensor (HF: `nn.Identity()`). Absence → None.
            let attn_norm = content
                .tensor(file, &format!("blk.{i}.attn_norm.weight"), device)
                .ok()
                .map(|w| RmsNorm::new(w.dequantize(device).unwrap(), eps));
            let is_sliding = i % pattern != 0;
            blocks.push(ModernBertBlock {
                attn_norm,
                ffn_norm: rms_norm(file, content, &format!("blk.{i}.ffn_norm"), device, eps)?,
                qkv_w: lin(file, content, &format!("blk.{i}.attn_qkv.weight"), device)?,
                out_w: lin(file, content, &format!("blk.{i}.attn_output.weight"), device)?,
                ffn_up_w: lin(file, content, &format!("blk.{i}.ffn_up.weight"), device)?,
                ffn_down_w: lin(file, content, &format!("blk.{i}.ffn_down.weight"), device)?,
                is_sliding,
            });
        }

        Ok(Self {
            token_embd,
            token_embd_norm,
            blocks,
            output_norm,
            hidden,
            n_head,
            head_dim,
            ffn,
            sliding_window,
            rope_full,
            rope_swa,
            pooling_type,
            max_context,
            device: device.clone(),
        })
    }

    /// One pre-norm block:
    ///   h = h + attn(attn_norm(h))      (attn_norm = Identity on layer 0)
    ///   h = h + mlp(mlp_norm(h))        (GLU: Wo(silu(Wi_input) * Wi_gate))
    fn forward_block(
        &self,
        blk: &ModernBertBlock,
        h: &Tensor,
        positions: &Tensor,
        mask_bias: &Tensor,
    ) -> Result<Tensor> {
        let (b, seq, _hidden) = h.dims3()?;

        // --- Self-attention (pre-norm) ---
        let hn = match blk.attn_norm.as_ref() {
            Some(n) => n.forward(h)?,
            None => h.clone(), // layer 0: identity
        };
        // Combined QKV: [b, seq, 3*hidden] -> [b, seq, 3, n_head, head_dim].
        let qkv = linear(&hn, &blk.qkv_w)?; // [b, seq, 3*hidden]
        let qkv = qkv.reshape((b, seq, 3, self.n_head, self.head_dim))?;
        let q = qkv.narrow(2, 0, 1)?.squeeze(2)?; // [b, seq, n_head, head_dim]
        let k = qkv.narrow(2, 1, 1)?.squeeze(2)?;
        let v = qkv.narrow(2, 2, 1)?.squeeze(2)?;
        // RoPE on q + k over the FULL head_dim (ModernBert uses rotary_emb
        // fraction=1.0, same as Gemma3/nomic/lfm2 — `apply_rope` matches).
        let rope_base = if blk.is_sliding { self.rope_swa } else { self.rope_full };
        let q = apply_rope(&q, positions, self.head_dim, rope_base, &self.device)?;
        let k = apply_rope(&k, positions, self.head_dim, rope_base, &self.device)?;
        let q = q.transpose(1, 2)?.contiguous()?; // [b, n_head, seq, head_dim]
        let k = k.transpose(1, 2)?.contiguous()?;
        let v = v.transpose(1, 2)?.contiguous()?;
        let scale = 1.0f64 / (self.head_dim as f64).sqrt();
        let qk = q.matmul(&k.t()?)?.affine(scale, 0.0)?; // [b, n_head, seq, seq]
        // Attention mask: bidirectional. Sliding layers add a ±window band
        // mask on top of the padding bias; full layers use padding only.
        let qk = if blk.is_sliding {
            let sw = sliding_window_mask(seq, self.sliding_window, &self.device)?;
            qk.broadcast_add(&sw)?
        } else {
            qk
        };
        let qk = qk.broadcast_add(mask_bias)?; // pad masking (bidirectional)
        let probs = ops::softmax_last_dim(&qk)?;
        let attn = probs.matmul(&v.contiguous()?)?; // [b, n_head, seq, head_dim]
        let attn = attn.transpose(1, 2)?.reshape((b, seq, self.hidden))?;
        let attn = linear(&attn, &blk.out_w)?;
        let h = (h + &attn)?;

        // --- GLU FFN (pre-norm) ---
        let hn = blk.ffn_norm.forward(&h)?;
        let up = linear(&hn, &blk.ffn_up_w)?; // [b, seq, 2*ffn]
        // HF ModernBertMLP: `input, gate = Wi(x).chunk(2, -1); Wo(act(input) * gate)`.
        // First half = input (activated), second half = gate (passthrough).
        let input = up.narrow(2, 0, self.ffn)?.silu()?;
        let gate = up.narrow(2, self.ffn, self.ffn)?;
        let mid = input.broadcast_mul(&gate)?;
        let ffn_out = linear(&mid, &blk.ffn_down_w)?;
        Ok((h + &ffn_out)?)
    }
}

impl GgufArch for ModernBertArch {
    fn hidden_dim(&self) -> usize {
        self.hidden
    }

    fn max_context(&self) -> u32 {
        self.max_context
    }

    fn forward_embed(&self, input_ids: &Tensor, attention_mask: &Tensor) -> Result<Tensor> {
        let (b, seq) = input_ids.dims2()?;
        let ids_flat = input_ids.flatten_all()?;
        // Token embedding ([vocab, hidden] indexes dim 0) + embedding RMSNorm.
        // No positional / token-type embedding (RoPE handles position;
        // ModernBert has no segment embeddings).
        let mut x = self.token_embd.embedding(&ids_flat)?;
        x = x.reshape((b, seq, self.hidden))?;
        x = self.token_embd_norm.forward(&x)?;

        let mask_bias = attn_bias(attention_mask)?; // [b,1,1,seq] 0 / -1e9
        let positions = Tensor::arange(0u32, seq as u32, &self.device)?;
        for blk in self.blocks.iter() {
            x = self.forward_block(blk, &x, &positions, &mask_bias)?;
        }
        let x = self.output_norm.forward(&x)?;

        // Pool by `pooling_type` (GGUF metadata): 2 = CLS (first token = bos).
        // HF `1_Pooling/config.json` confirms CLS pooling for granite; fall back
        // to CLS for any unknown value and log it.
        match self.pooling_type {
            2 => {
                // CLS (first token). Then L2-normalize.
                let pooled = x.narrow(1, 0, 1)?.squeeze(1)?;
                l2_normalize(&pooled)
            }
            1 => {
                // MEAN (masked) + L2 — for completeness if a future modern-bert
                // GGUF declares mean pooling.
                crate::rag::gguf_gemma::pool_and_normalize(&x, attention_mask)
            }
            other => {
                log::warn!(
                    "[RAG] modern-bert unknown pooling_type {}, using CLS",
                    other
                );
                let pooled = x.narrow(1, 0, 1)?.squeeze(1)?;
                l2_normalize(&pooled)
            }
        }
    }
}

/// Load a 2D linear weight `[out, in]` → transpose to `[in, out]` (dequantized
/// f32). Same helper as `gguf_lfm2.rs::lin` — candle returns logical `[out, in]`
/// for GGUF linears, and the forward's `linear` expects `[in, out]`.
fn lin<R: Read + Seek>(
    file: &mut R,
    ct: &Content,
    name: &str,
    device: &Device,
) -> Result<Tensor> {
    Ok(ct
        .tensor(file, name, device)?
        .dequantize(device)?
        .t()?
        .contiguous()?)
}

/// Load an RMSNorm (weight only) from `<prefix>.weight` — standard candle_nn
/// RmsNorm (NO +1 weight shift; that's Gemma-specific).
fn rms_norm<R: Read + Seek>(
    file: &mut R,
    ct: &Content,
    prefix: &str,
    device: &Device,
    eps: f64,
) -> Result<RmsNorm> {
    let weight = ct
        .tensor(file, &format!("{prefix}.weight"), device)?
        .dequantize(device)?;
    Ok(RmsNorm::new(weight, eps))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke test: load the real Granite Embedding 97M GGUF and run one forward
    /// pass — verifies `from_content` parses the tensors + `forward_embed`
    /// produces a 384-dim, L2-normalized vector without panicking. NOT a
    /// numerical check (no torch reference here) — that's the deferred cosine
    /// probe. `#[ignore]` because it needs the bundled GGUF on disk + a candle
    /// CPU device; run with `cargo test --lib rag::gguf_modernbert -- --ignored`.
    #[test]
    #[ignore]
    fn granite_loads_and_embeds() {
        let gguf = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("runtimes/rag/model/granite/97m/granite-embedding-97M-multilingual-r2-Q8_0.gguf");
        if !gguf.exists() {
            eprintln!("skipping: {} not present", gguf.display());
            return;
        }
        let device = Device::Cpu;
        let mut file = std::fs::File::open(&gguf).expect("open gguf");
        let content =
            Content::read(&mut file).expect("read gguf");
        let arch = ModernBertArch::from_content(&mut file, &content, &device)
            .expect("from_content");
        assert_eq!(arch.hidden_dim(), 384);
        assert_eq!(arch.max_context(), 32768);
        assert_eq!(arch.pooling_type, 2); // CLS
        // One forward: [1, 6] ids -> [1, 384] pooled + L2-normalized.
        let ids = Tensor::new(vec![179934u32, 1u32, 2u32, 3u32, 4u32, 179938u32], &device)
            .unwrap()
            .reshape((1usize, 6usize))
            .unwrap();
        let mask = Tensor::new(vec![1u32, 1u32, 1u32, 1u32, 1u32, 1u32], &device)
            .unwrap()
            .reshape((1usize, 6usize))
            .unwrap();
        let emb = arch.forward_embed(&ids, &mask).expect("forward_embed");
        assert_eq!(emb.dims2().unwrap(), (1, 384));
        let v = emb.to_vec2::<f32>().unwrap().pop().unwrap();
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-3,
            "expected L2-normalized (norm≈1), got {norm}"
        );
    }

    /// Diagnostic: embed a set of probe texts (including the user's reported
    /// query "回家吃饭了，吃什么呢" vs clearly-unrelated docs) via the full
    /// GgufEmbedder pipeline (real tokenizer + deploy.json prefix + forward)
    /// and print a pairwise cosine matrix. Reveals whether unrelated queries
    /// scoring 0.66+ (reported) is:
    ///   - model anisotropy (small embedding models give cosine 0.5-0.9 even
    ///     for unrelated text — expected; mitigate with score_threshold), OR
    ///   - a forward bug (cosine ~0 for unrelated, ~1 for identical would be
    ///     correct; a uniform ~0.8 for everything suggests a pooling/normalize
    ///     bug). Run: `cargo test --lib rag::gguf_modernbert::tests::granite_cosine_probe -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn granite_cosine_probe() {
        use crate::rag::embedder::Embedder;
        let size_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("runtimes/rag/model/granite/97m");
        if !size_dir.exists() {
            eprintln!("skipping: {} not present", size_dir.display());
            return;
        }
        let gguf = std::fs::read_dir(&size_dir)
            .expect("read size dir")
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().and_then(|x| x.to_str()) == Some("gguf"))
            .expect("no .gguf in size dir");
        let mut embedder =
            crate::rag::gguf::GgufEmbedder::load(&gguf).expect("load GgufEmbedder");
        let cfg = crate::rag::embedder::read_deploy_config(&size_dir);
        eprintln!("deploy: query_prefix={:?} doc_prefix={:?}", cfg.search_query_prefix, cfg.import_doc_prefix);

        let probes: &[&str] = &[
            "回家吃饭了，吃什么呢",                 // the reported query
            "今天天气真好，适合出去玩",             // unrelated (weather)
            "The quick brown fox jumps over the lazy dog", // unrelated (English)
            "rust 编译错误如何修复 lifetime 问题",  // unrelated (tech)
            "回家吃饭了，吃什么呢",                 // duplicate (self-similarity = 1.0)
        ];
        // Apply the model's asymmetric prefixes like the service does: query
        // prefix on the first probe, doc prefix on the rest.
        let q_pfx = &cfg.search_query_prefix;
        let d_pfx = &cfg.import_doc_prefix;
        let embedded: Vec<(String, Vec<f32>)> = probes
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let text = if i == 0 {
                    format!("{q_pfx}{t}")
                } else {
                    format!("{d_pfx}{t}")
                };
                let v = embedder.embed(&text).expect("embed");
                (t.to_string(), v)
            })
            .collect();

        // Print each probe's cosine vs the query (row 0) + the self-similarity
        // check (identical texts must be ~1.0).
        eprintln!("\n=== cosine of each probe vs the QUERY (row 0) ===");
        let q = &embedded[0].1;
        for (i, (t, v)) in embedded.iter().enumerate() {
            let cos: f32 = q.iter().zip(v).map(|(a, b)| a * b).sum();
            eprintln!("  query x [{i}] = {cos:.4}   {t}");
        }
        // Self-check: identical texts (probe 0 and 4) must be ~1.0.
        let self_cos: f32 = embedded[0].1.iter().zip(&embedded[4].1).map(|(a, b)| a * b).sum();
        assert!((self_cos - 1.0).abs() < 1e-2, "identical texts should be ~1.0, got {self_cos}");
    }
}
