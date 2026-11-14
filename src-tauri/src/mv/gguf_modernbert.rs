//! GGUF architecture strategy for `modern-bert` (IBM Granite Embedding 97M
//! Multilingual R2, and any other `modern-bert` GGUF embedding).
//!
//! ## Architecture (verified against the GGUF metadata + tensor layout AND the
//! HF reference: `ibm-granite/granite-embedding-97m-r2-multilingual-r2`'
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
//! ~1.0 against `ibm-granite/granite-embedding-97m-r2-multilingual-r2` once
//! running (a deferred `#[ignore]` probe, see the plan).

use std::io::{Read, Seek};

use anyhow::{anyhow, Result};
use candle_core::quantized::gguf_file::Content;
use candle_core::{Device, Module, Tensor};
use candle_nn::{ops, RmsNorm};

use crate::mv::gguf_gemma::{
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
        if n_head == 0 {
            return Err(anyhow!("invalid GGUF metadata: n_head must be > 0"));
        }
        let head_dim = hidden / n_head; // 32
        let ffn = get_u32(&format!("{}feed_forward_length", pfx), None)?; // 1536
        let sliding_window =
            get_u32(&format!("{}attention.sliding_window", pfx), Some(128))?;
        let pattern = get_u32(&format!("{}attention.sliding_window_pattern", pfx), Some(3))?;
        if pattern == 0 {
            return Err(anyhow!("invalid GGUF metadata: pattern must be > 0"));
        }
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
            // `blk.0.attn_norm.weight` tensor (HF: `nn.Identity()`). Absence
            // is legal ONLY for layer 0; layers 1.. must propagate load
            // errors (`?`), otherwise a wrong tensor name / corrupt file
            // would silently build a model with missing norms and produce
            // degraded embeddings. The inner dequantize is `?` too (was an
            // unwrap → panic on the async load thread).
            let attn_norm = if i == 0 {
                match content.tensor(file, "blk.0.attn_norm.weight", device) {
                    Ok(w) => Some(RmsNorm::new(w.dequantize(device)?, eps)),
                    Err(_) => None,
                }
            } else {
                let w = content.tensor(file, &format!("blk.{i}.attn_norm.weight"), device)?;
                Some(RmsNorm::new(w.dequantize(device)?, eps))
            };
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
                crate::mv::gguf_gemma::pool_and_normalize(&x, attention_mask)
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
            .join("runtimes/rag/model/embedding/granite/97m-r2/model.gguf");
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
        use crate::mv::embedder::Embedder;
        let size_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("runtimes/rag/model/embedding/granite/97m-r2");
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
            crate::mv::gguf::GgufEmbedder::load(&gguf).expect("load GgufEmbedder");
        let cfg = crate::mv::embedder::read_deploy_config(&size_dir);
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
        // Probes 0 and 4 are the same content but 0 carries the QUERY prefix
        // while 4 carries the DOC prefix (asymmetric prefixes) — the cosine
        // must stay high (same content) without being exactly 1.0.
        let self_cos: f32 = embedded[0].1.iter().zip(&embedded[4].1).map(|(a, b)| a * b).sum();
        assert!(self_cos > 0.8, "same content across prefixes should be > 0.8, got {self_cos}");
    }

    /// Cross-lingual probe for the Smart Routing model (F2LLM-v2-80M): does a
    /// Chinese query embed near its English paraphrase and near an English
    /// tool description? Mirrors the live failure "如何查询公网ip" vs the
    /// getPublicIp tool text. Run:
    /// Live probe: harrier BF16 GGUF (mykor) vs the bundled Q8_0 — does the
    /// higher-precision quantization restore official-level discrimination?
    /// `cargo test --lib mv::gguf_modernbert::tests::harrier_bf16_probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn harrier_bf16_probe() {
        use crate::mv::embedder::Embedder;
        let gguf = std::path::PathBuf::from(std::env::var("HARRIER_GGUF").unwrap_or_else(|_| "/tmp/harrier_bf16.gguf".into()));
        if !gguf.is_file() {
            eprintln!("SKIP: /tmp/harrier_bf16.gguf not present");
            return;
        }
        let mut embedder = crate::mv::gguf::GgufEmbedder::load(&gguf).expect("load");
        let pfx = "Instruct: Given a web search query, retrieve relevant passages that answer the query.\nQuery: {query}";
        let qt = |q: &str| crate::mv::embedder::render_query_embed_text(pfx, q);
        let tool = "getPublicIp Get Public IP Address\n\nReturns the public IP address of the client making the request.";
        let et = embedder.embed(tool).expect("tool");
        let pairs: &[(&str, &str)] = &[
            ("zh", "如何查询公网ip"),
            ("en", "get public ip address"),
            ("neg1", "今天天气怎么样"),
            ("neg2", "the fox jumps over the lazy dog"),
            ("neg3", "帮我写一首诗"),
        ];
        for (k, a) in pairs {
            let qv = embedder.embed(&qt(a)).expect("q");
            let c: f32 = qv.iter().zip(&et).map(|(a, b)| a * b).sum();
            eprintln!("harrier-bf16 {k}: cos = {c:.4}");
        }
    }

    /// `cargo test --lib mv::gguf_modernbert::tests::f2llm_zh_crosslingual_probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn f2llm_zh_crosslingual_probe() {
        use crate::mv::embedder::Embedder;
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtimes/rag/model/embedding");
        // Every bundled size dir that ships a model.gguf.
        let mut dirs: Vec<std::path::PathBuf> = std::fs::read_dir(&root)
            .expect("read model root")
            .flatten()
            .filter(|e| e.path().is_dir())
            .flat_map(|fam| std::fs::read_dir(fam.path()).expect("read fam").flatten().map(|e| e.path()).collect::<Vec<_>>())
            .filter(|d| d.join("model.gguf").is_file())
            .collect();
        dirs.sort();
        for size_dir in dirs {
            let label = format!(
                "{}/{}",
                size_dir.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or("?"),
                size_dir.file_name().and_then(|n| n.to_str()).unwrap_or("?")
            );
            eprintln!("\n########## MODEL: {label} ##########");
            let mut embedder =
                match crate::mv::gguf::GgufEmbedder::load(&size_dir.join("model.gguf")) {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("  LOAD FAILED: {e}");
                        continue;
                    }
                };
            let cfg = crate::mv::embedder::read_deploy_config(&size_dir);
            eprintln!("deploy: query_prefix={:?}", cfg.search_query_prefix);

        // (query-side, doc-side) pairs: zh query vs en description, en vs en
        // (control), zh vs zh-server-name row (the indexed server row).
        let pairs: &[(&str, &str)] = &[
            (
                "如何查询公网ip",
                "getPublicIp Get Public IP Address\n\nReturns the public IP address of the client making the request.",
            ),
            ("get public ip address", "getPublicIp Get Public IP Address\n\nReturns the public IP address of the client making the request."),
            ("如何查询公网ip", "本机公网ip查询"),
            ("get public ip address", "本机公网ip查询"),
            ("如何查询公网ip", "如何查询公网ip"), // near-self sanity (prefix asymmetry applies)
        ];
        let q_pfx = &cfg.search_query_prefix;
            let negs: &[(&str, &str)] = &[
                ("如何查询公网ip", "今天天气真好适合出去玩"),
                ("如何查询公网ip", "The quick brown fox jumps over the lazy dog"),
                ("get public ip address", "帮我写一首关于春天的诗"),
            ];

            // Threshold sweep: for each candidate threshold, does the zh
            // query hit the en tool description (pass) while negatives stay
            // out (no false hit)? Printed as a compact matrix per model.
            eprintln!("--- threshold sweep (pair0=zh↔enTool, pair1=en↔enTool, neg0/1/2) ---");
            eprintln!("  thr |  zh-enTool  en-enTool | neg0  neg1  neg2 | zh pass? en pass? clean?");
            for thr in [0.30f32, 0.35, 0.40, 0.45, 0.50, 0.55, 0.60, 0.65, 0.70, 0.75, 0.80, 0.85, 0.90] {
                let qtext0 = crate::mv::embedder::render_query_embed_text(q_pfx, pairs[0].0);
                let qtext1 = crate::mv::embedder::render_query_embed_text(q_pfx, pairs[1].0);
                let d0 = embedder.embed(pairs[0].1).expect("d0");
                let qv0 = embedder.embed(&qtext0).expect("q0");
                let qv1 = embedder.embed(&qtext1).expect("q1");
                let c0: f32 = qv0.iter().zip(&d0).map(|(a, b)| a * b).sum();
                let c1: f32 = qv1.iter().zip(&d0).map(|(a, b)| a * b).sum();
                let mut negs_cos = Vec::new();
                for (q, d) in negs {
                    let qt = crate::mv::embedder::render_query_embed_text(q_pfx, q);
                    let qv = embedder.embed(&qt).expect("nq");
                    let dv = embedder.embed(d).expect("nd");
                    negs_cos.push(qv.iter().zip(&dv).map(|(a, b)| a * b).sum::<f32>());
                }
                let zh_pass = c0 >= thr;
                let en_pass = c1 >= thr;
                let clean = !negs_cos.iter().any(|&c| c >= thr);
                eprintln!(
                    "  {:.2} |    {:.4}     {:.4}   | {:.3} {:.3} {:.3} |   {:?}      {:?}     {:?}",
                    thr, c0, c1, negs_cos[0], negs_cos[1], negs_cos[2], zh_pass, en_pass, clean
                );
            }
            // Negative controls: unrelated zh↔en pairs. A model whose
            // unrelated pairs score nearly as high as related ones is
            // anisotropic (uniform-high cosine) — its high "related" scores
            // carry no discriminative signal.
            eprintln!("--- negative controls (unrelated) ---");
            for use_prefix in [false] {
                for (i, (q, d)) in negs.iter().enumerate() {
                    let qtext = if use_prefix {
                        crate::mv::embedder::render_query_embed_text(q_pfx, q)
                    } else {
                        q.to_string()
                    };
                    let qv = embedder.embed(&qtext).expect("embed q");
                    let dv = embedder.embed(d).expect("embed d");
                    let cos: f32 = qv.iter().zip(&dv).map(|(a, b)| a * b).sum();
                    let qshow: String = q.chars().take(12).collect();
                    let dshow: String = d.chars().take(24).collect();
                    eprintln!("neg {i}: cos = {cos:.4}   {qshow:?} vs {dshow:?}");
                }
            }
            // Two passes: with the model's asymmetric query prefix, and
            // without (both sides raw). If the prefix pass scores WORSE
            // across the board (especially identical-text pair 4 << 1.0),
            // the template is misapplied for this model and raw symmetric
            // embedding is better.
            for use_prefix in [true, false] {
                eprintln!("=== use_query_prefix = {use_prefix} ===");
                for (i, (q, d)) in pairs.iter().enumerate() {
                    let qtext = if use_prefix {
                        crate::mv::embedder::render_query_embed_text(q_pfx, q)
                    } else {
                        q.to_string()
                    };
                    let qv = embedder.embed(&qtext).expect("embed q");
                    let dv = embedder.embed(d).expect("embed d");
                    let cos: f32 = qv.iter().zip(&dv).map(|(a, b)| a * b).sum();
                    let qshow: String = q.chars().take(12).collect();
                    let dshow: String = d.chars().take(30).collect();
                    eprintln!("pair {i}: cos = {cos:.4}   {qshow:?} vs {dshow:?}");
                }
            }
            let _ = q_pfx;
        }
    }
}

    /// Probe: can our embedder path load the Julia-1 decision GGUF
    /// (`decision/julia/1`)? It is a modern-bert classifier, not an embedder —
    /// this documents what our current code does with it.
    /// `cargo test --lib mv::gguf_modernbert::tests::julia_decision_probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn julia_decision_probe() {
        use crate::mv::embedder::Embedder;
        let gguf = std::path::PathBuf::from(
            "runtimes/rag/model/decision/julia/1/model.gguf",
        );
        if !gguf.is_file() {
            eprintln!("SKIP: julia_1 model.gguf not present");
            return;
        }
        match crate::mv::gguf::GgufEmbedder::load(&gguf) {
            Ok(mut e) => {
                eprintln!("julia: loaded OK, dim = {}", e.embed("test").unwrap().len());
                for t in ["如何查询公网ip", "get public ip address"] {
                    let v = e.embed(t).unwrap();
                    let n: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                    eprintln!("julia embed {:?}: dim {} norm {:.4} first3 {:?}",
                        t, v.len(), n, &v[..3.min(v.len())]);
                }
            }
            Err(e) => eprintln!("julia: LOAD FAILED: {e}"),
        }
    }
