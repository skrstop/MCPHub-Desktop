//! GGUF architecture strategy: the model forward + pooling + normalization,
//! abstracted over architecture so new GGUF model families can be added as
//! new `GgufArch` impls. Currently only Gemma/Gemma3 is wired up
//! (`Gemma3EmbedArch`).
//!
//! ## Architecture (verified against candle-transformers 0.11 `gemma3.rs` +
//! the model.gguf tensor names)
//! Gemma3 has FOUR RMSNorms per layer (input / post-attention / pre-FFN /
//! post-FFN) and a custom RMSNorm `x / sqrt(mean(x²)+eps) * (weight + 1.0)`
//! (the +1 shift is Gemma-specific - NOT `candle_nn::RmsNorm`). The embedding
//! variant uses **bidirectional** attention (no causal mask; the model was
//! trained with `use_bidirectional_attention: true`) + mean-pool + L2-norm.
//! Sliding-window layers (every `pattern`-th is full) limit attention to ±window
//! tokens bidirectionally; for typical RAG chunks (≤ window) this is a no-op.
//!
//! ## ⚠ Correctness caveat
//! Compiles clean (cargo check 0 errors). The GGUF here is `gemma-embedding`
//! (300M, 330M_Q4_0). Validate embeddings against the ONNX reference (cosine ~1.0).

use std::io::{Read, Seek};

use anyhow::{anyhow, Result};
use candle_core::quantized::gguf_file::Content;
use candle_core::quantized::QMatMul;
use candle_core::{D, DType, Device, Module, Tensor};
use candle_nn::ops;

/// A GGUF model architecture: forward pass from token ids + attention mask to
/// pooled, L2-normalized `[batch, hidden]` embeddings.
pub trait GgufArch: Send + Sync {
    fn forward_embed(&self, input_ids: &Tensor, attention_mask: &Tensor) -> Result<Tensor>;
    fn hidden_dim(&self) -> usize;
    fn max_context(&self) -> u32;
}

/// Gemma RMSNorm: `x / sqrt(mean(x²) + eps) * (weight + 1.0)`. The +1 weight
/// shift is Gemma-specific (candle_nn::RmsNorm does `* weight` - wrong here).
struct GemmaRmsNorm {
    weight: Tensor,
    eps: f64,
}

impl GemmaRmsNorm {
    fn new(weight: Tensor, eps: f64) -> Self {
        Self { weight, eps }
    }
}

impl Module for GemmaRmsNorm {
    fn forward(&self, x: &Tensor) -> candle_core::Result<Tensor> {
        let internal_dtype = match x.dtype() {
            DType::F16 | DType::BF16 => DType::F32,
            d => d,
        };
        let hidden = x.dim(D::Minus1)? as f64;
        let x_in = x.to_dtype(internal_dtype)?;
        let mean_sq = (x_in.sqr()?.sum_keepdim(D::Minus1)? / hidden)?;
        let x_normed = x_in.broadcast_div(&(mean_sq + self.eps)?.sqrt()?)?;
        x_normed
            .to_dtype(x.dtype())?
            .broadcast_mul(&(&self.weight + 1.0)?)
    }
}

/// One Gemma3 transformer layer (4 RMSNorms + bidirectional attention + MLP).
struct Gemma3Layer {
    input_norm: GemmaRmsNorm,
    post_attn_norm: GemmaRmsNorm,
    pre_ffn_norm: GemmaRmsNorm,
    post_ffn_norm: GemmaRmsNorm,
    wq: QMatMul,
    wk: QMatMul,
    wv: QMatMul,
    wo: QMatMul,
    q_norm: GemmaRmsNorm,
    k_norm: GemmaRmsNorm,
    gate: QMatMul,
    up: QMatMul,
    down: QMatMul,
    is_sliding: bool,
    sliding_window: usize,
    rope_base: f32,
}

/// Gemma3 embedding model (bidirectional, quantized GGUF).
pub struct Gemma3EmbedArch {
    token_embd: Tensor,
    layers: Vec<Gemma3Layer>,
    final_norm: GemmaRmsNorm,
    hidden: usize,
    n_head: usize,
    n_kv_head: usize,
    head_dim: usize,
    max_context: u32,
    device: Device,
}

impl Gemma3EmbedArch {
    /// Build from an already-read `Content` (the caller - `GgufEmbedder::load` -
    /// opens the file + reads Content ONCE and shares it with the tokenizer
    /// builder + the arch, avoiding a double-read). `file` is needed to read
    /// tensor data via `content.tensor(file, ...)`.
    pub fn from_content<R: Read + Seek>(
        file: &mut R,
        content: &Content,
        device: &Device,
    ) -> Result<Self> {
        let md = &content.metadata;

        // Metadata key prefix = architecture name (general.architecture) + ".".
        // e.g. "gemma-embedding." -> "gemma-embedding.embedding_length".
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

        let hidden = get_u32(&format!("{}embedding_length", pfx), None)?;
        let block_count = get_u32(&format!("{}block_count", pfx), None)?;
        let n_head = get_u32(&format!("{}attention.head_count", pfx), None)?;
        let n_kv_head = get_u32(&format!("{}attention.head_count_kv", pfx), None)?;
        let head_dim = get_u32(&format!("{}attention.key_length", pfx), Some(hidden / n_head))?;
        let sliding_window = get_u32(&format!("{}attention.sliding_window", pfx), Some(512))?;
        // Gemma3: every `sliding_window_pattern`-th layer is full attention.
        // pattern default 6 (config.json layer_types: 5 sliding + 1 full).
        let pattern = get_u32(&format!("{}sliding_window_pattern", pfx), Some(6))?;
        let rope_theta = get_f32(&format!("{}rope.freq_base", pfx), 1_000_000.0);
        let rope_local = get_f32(&format!("{}rope.local_freq_base", pfx), 10_000.0);
        let max_context = get_u32(&format!("{}context_length", pfx), Some(2048))? as u32;
        let rms_eps = get_f32(&format!("{}attention.layer_norm_rms_epsilon", pfx), 1e-6) as f64;

        let token_embd = content.tensor(file, "token_embd.weight", device)?;
        let token_embd = token_embd.dequantize(device)?;
        let final_norm = gemma_rms_norm(file, &content, "output_norm.weight", device, rms_eps)?;

        let mut layers = Vec::with_capacity(block_count);
        for i in 0..block_count {
            // Sliding-window layer if (i+1) % pattern != 0 (i.e. NOT the
            // pattern-th); matches candle gemma3.rs `(idx+1) % pattern > 0`.
            let is_sliding = (i + 1) % pattern != 0;
            let rope_base = if is_sliding { rope_local } else { rope_theta };
            layers.push(Gemma3Layer {
                input_norm: gemma_rms_norm(file, &content, &format!("blk.{i}.attn_norm.weight"), device, rms_eps)?,
                post_attn_norm: gemma_rms_norm(file, &content, &format!("blk.{i}.post_attention_norm.weight"), device, rms_eps)?,
                pre_ffn_norm: gemma_rms_norm(file, &content, &format!("blk.{i}.ffn_norm.weight"), device, rms_eps)?,
                post_ffn_norm: gemma_rms_norm(file, &content, &format!("blk.{i}.post_ffw_norm.weight"), device, rms_eps)?,
                wq: qmatmul(file, &content, &format!("blk.{i}.attn_q.weight"), device)?,
                wk: qmatmul(file, &content, &format!("blk.{i}.attn_k.weight"), device)?,
                wv: qmatmul(file, &content, &format!("blk.{i}.attn_v.weight"), device)?,
                wo: qmatmul(file, &content, &format!("blk.{i}.attn_output.weight"), device)?,
                q_norm: gemma_rms_norm(file, &content, &format!("blk.{i}.attn_q_norm.weight"), device, rms_eps)?,
                k_norm: gemma_rms_norm(file, &content, &format!("blk.{i}.attn_k_norm.weight"), device, rms_eps)?,
                gate: qmatmul(file, &content, &format!("blk.{i}.ffn_gate.weight"), device)?,
                up: qmatmul(file, &content, &format!("blk.{i}.ffn_up.weight"), device)?,
                down: qmatmul(file, &content, &format!("blk.{i}.ffn_down.weight"), device)?,
                is_sliding,
                sliding_window,
                rope_base,
            });
        }

        Ok(Self {
            token_embd,
            layers,
            final_norm,
            hidden,
            n_head,
            n_kv_head,
            head_dim,
            max_context,
            device: device.clone(),
        })
    }
}

impl GgufArch for Gemma3EmbedArch {
    fn hidden_dim(&self) -> usize {
        self.hidden
    }

    fn max_context(&self) -> u32 {
        self.max_context
    }

    fn forward_embed(&self, input_ids: &Tensor, attention_mask: &Tensor) -> Result<Tensor> {
        let (b, seq) = input_ids.dims2()?;
        let ids_flat = input_ids.flatten_all()?;
        // Gemma2/3 do NOT scale token embeddings (Gemma1 did).
        let mut x = self.token_embd.embedding(&ids_flat)?;
        x = x.reshape((b, seq, self.hidden))?;

        // Padding bias [b,1,1,seq]: 0 real / -1e9 pad. Bidirectional (no causal).
        let mask_bias = attn_bias(attention_mask)?;
        let positions = Tensor::arange(0u32, seq as u32, &self.device)?;

        for layer in self.layers.iter() {
            x = forward_layer(
                layer, &x, &positions, &mask_bias,
                self.head_dim, self.n_head, self.n_kv_head, &self.device,
            )?;
        }
        let x = self.final_norm.forward(&x)?;
        pool_and_normalize(&x, attention_mask)
    }
}

/// One Gemma3 layer:
///   residual + post_attn_norm(attn(input_norm(x)))
///   residual + post_ffn_norm(mlp(pre_ffn_norm(residual)))
fn forward_layer(
    l: &Gemma3Layer,
    x: &Tensor,
    positions: &Tensor,
    mask_bias: &Tensor,
    head_dim: usize,
    n_head: usize,
    n_kv_head: usize,
    device: &Device,
) -> Result<Tensor> {
    let (b, seq, _hidden) = x.dims3()?;
    // --- Self-attention ---
    let h = l.input_norm.forward(x)?;
    let q = l.wq.forward(&h)?;
    let k = l.wk.forward(&h)?;
    let v = l.wv.forward(&h)?;
    let q = q.reshape((b, seq, n_head, head_dim))?;
    let k = k.reshape((b, seq, n_kv_head, head_dim))?;
    let v = v.reshape((b, seq, n_kv_head, head_dim))?;
    // Per-head q/k RMSNorm (Gemma norm, weight+1 shift).
    let q = l.q_norm.forward(&q)?;
    let k = l.k_norm.forward(&k)?;
    let q = apply_rope(&q, positions, head_dim, l.rope_base, device)?;
    let k = apply_rope(&k, positions, head_dim, l.rope_base, device)?;
    // GQA: repeat k, v from n_kv_head to n_head.
    let rep = n_head / n_kv_head;
    let k = k.repeat((1, 1, rep, 1))?;
    let v = v.repeat((1, 1, rep, 1))?;
    let q = q.transpose(1, 2)?.contiguous()?;
    let k = k.transpose(1, 2)?.contiguous()?;
    let v = v.transpose(1, 2)?.contiguous()?;
    let scale = 1.0f64 / (head_dim as f64).sqrt();
    let qk = q.matmul(&k.t()?)?.affine(scale, 0.0)?;
    // Sliding-window mask (bidirectional band) for sliding layers.
    let qk = if l.is_sliding {
        let sw = sliding_window_mask(seq, l.sliding_window, device)?;
        qk.broadcast_add(&sw)?
    } else {
        qk
    };
    let qk = qk.broadcast_add(mask_bias)?;
    let probs = ops::softmax_last_dim(&qk)?;
    let attn = probs.matmul(&v.contiguous()?)?;
    let attn = attn.transpose(1, 2)?.reshape((b, seq, n_head * head_dim))?;
    let attn = l.wo.forward(&attn)?;
    let attn = l.post_attn_norm.forward(&attn)?;
    let residual = (x + &attn)?;

    // --- MLP (gated GeLU-tanh) ---
    let h = l.pre_ffn_norm.forward(&residual)?;
    let gate = l.gate.forward(&h)?.gelu()?;
    let up = l.up.forward(&h)?;
    let mlp = l.down.forward(&(&gate * &up)?)?;
    let mlp = l.post_ffn_norm.forward(&mlp)?;
    Ok((residual + &mlp)?)
}

/// Padding bias [b,1,1,seq]: 0 real / -1e9 pad (bidirectional, no causal).
pub(crate) fn attn_bias(mask: &Tensor) -> Result<Tensor> {
    let (b, s) = mask.dims2()?;
    let mask_f = mask.to_dtype(DType::F32)?;
    // 1-mask -> 0 real / 1 pad; * -1e9 -> 0 / -1e9.
    let bias = mask_f.affine(-1.0, 1.0)?.affine(-1e9, 0.0)?;
    Ok(bias.reshape((b, 1, 1, s))?)
}

/// Sliding-window bidirectional mask [1,1,seq,seq]: 0 inside (|i-j|<=window),
/// -1e9 outside. Built as a plain `Vec<f32>` on CPU (index math in Rust) and
/// uploaded once - avoids Metal op gaps (e.g. `uabs U32` isn't implemented on
/// Metal, so an `arange(u32).abs()` path would error there). For the typical
/// chunk (≤ window tokens) this mask is a no-op anyway (all in-window).
/// `pub(crate)` so the modern-bert arch (sliding-window layers) can reuse it.
pub(crate) fn sliding_window_mask(seq: usize, window: usize, device: &Device) -> Result<Tensor> {
    let mut m = vec![0f32; seq * seq];
    for ii in 0..seq {
        let row = ii * seq;
        for jj in 0..seq {
            let dist = (ii as isize - jj as isize).abs() as usize;
            if dist > window {
                m[row + jj] = -1e9;
            } // else 0.0 (inside the window)
        }
    }
    Ok(Tensor::from_vec(m, (1, 1, seq, seq), device)?)
}

/// RoPE (rotate-half) over `head_dim` (full head). cos/sin precomputed per-seq.
pub(crate) fn apply_rope(
    q: &Tensor,
    positions: &Tensor,
    head_dim: usize,
    rope_base: f32,
    device: &Device,
) -> Result<Tensor> {
    let half = head_dim / 2;
    let inv_freq: Vec<f32> = (0..half)
        .map(|i| 1.0 / rope_base.powf((2 * i) as f32 / head_dim as f32))
        .collect();
    let inv_freq = Tensor::from_vec(inv_freq, half, device)?;
    let positions = positions.to_dtype(DType::F32)?.reshape((positions.dim(0)?, 1))?;
    let freqs = positions.matmul(&inv_freq.reshape((1, half))?)?;
    let cos = freqs.cos()?;
    let sin = freqs.sin()?;
    let (b, seq_d, n_head, _) = q.dims4()?;
    let cos = cos.reshape((1, seq_d, 1, half))?.broadcast_as((b, seq_d, n_head, half))?;
    let sin = sin.reshape((1, seq_d, 1, half))?.broadcast_as((b, seq_d, n_head, half))?;
    let q1 = q.narrow(3, 0, half)?;
    let q2 = q.narrow(3, half, half)?;
    let o1 = (q1.broadcast_mul(&cos)? - q2.broadcast_mul(&sin)?)?;
    let o2 = (q1.broadcast_mul(&sin)? + q2.broadcast_mul(&cos)?)?;
    Ok(Tensor::cat(&[o1, o2], 3)?)
}

/// Matmul a hidden tensor `h` (`[..., in]`) by a 2D weight `w` (`[in, out]`),
/// returning `[..., out]`. candle's `matmul` requires both operands to share the
/// same rank (3D @ 2D is rejected) with no batch broadcasting, so this flattens
/// the leading dims to 2D, matmuls, and reshapes back. Used by the nomic/lfm2
/// arches which hold dequantized 2D `[in, out]` weights (QMatMul, which handles
/// 3D internally, expects `[out, in]` and would double-transpose these).
pub(crate) fn linear(h: &Tensor, w: &Tensor) -> Result<Tensor> {
    let in_dim = w.dim(0)?;
    let out_dim = w.dim(1)?;
    let h_dims = h.dims();
    match h_dims.len() {
        2 => Ok(h.matmul(w)?),
        3 => {
            let (b, seq) = (h_dims[0], h_dims[1]);
            Ok(h.reshape((b * seq, in_dim))?.matmul(w)?.reshape((b, seq, out_dim))?)
        }
        _ => Err(anyhow!("linear: unsupported hidden rank {}", h_dims.len())),
    }
}

/// L2-normalize a `[b, dim]` tensor row-wise. The vectordb uses L2 distance
/// assuming normalized embeddings (so L2 ranking == cosine ranking) - every
/// arch must normalize its pooled output, even when the HF reference doesn't
/// (e.g. LFM2's `modules.json` has no Normalize module).
pub(crate) fn l2_normalize(x: &Tensor) -> Result<Tensor> {
    let b = x.dim(0)?;
    let norm = x
        .sqr()?
        .sum(1)?
        .sqrt()?
        .broadcast_add(&Tensor::new(&[1e-12f32], x.device())?)?
        .reshape((b, 1))?;
    Ok(x.broadcast_div(&norm)?)
}

/// Mean-pool (masked) over seq + L2-normalize. [b,seq,hidden] -> [b,hidden].
pub(crate) fn pool_and_normalize(x: &Tensor, mask: &Tensor) -> Result<Tensor> {
    let (b, seq, _hidden) = x.dims3()?;
    let mask_f = mask.to_dtype(DType::F32)?.reshape((b, seq, 1))?;
    let masked = x.broadcast_mul(&mask_f)?;
    let sum = masked.sum(1)?;
    let denom = mask_f.sum(1)?.broadcast_add(&Tensor::new(&[1e-9f32], x.device())?)?;
    let pooled = sum.broadcast_div(&denom)?;
    l2_normalize(&pooled)
}

fn gemma_rms_norm<R: Read + Seek>(
    reader: &mut R,
    ct: &Content,
    name: &str,
    device: &Device,
    eps: f64,
) -> Result<GemmaRmsNorm> {
    let w = ct.tensor(reader, name, device)?.dequantize(device)?;
    Ok(GemmaRmsNorm::new(w, eps))
}

fn qmatmul<R: Read + Seek>(
    reader: &mut R,
    ct: &Content,
    name: &str,
    device: &Device,
) -> Result<QMatMul> {
    // On CPU, dequantize to f32 so the forward uses Apple Accelerate (AMX
    // sgemm) via candle's accelerate feature - AMX is ~50-100x NEON f32
    // throughput, far outweighing the 8x data increase vs int4. candle's int4
    // k_quants is pure-Rust NEON (no AMX) and much slower in practice.
    // On GPU (Metal), keep int4 (Metal quantized kernels are fast there).
    let q = ct.tensor(reader, name, device)?;
    match device {
        Device::Cpu => Ok(QMatMul::Tensor(q.dequantize(device)?)),
        _ => Ok(QMatMul::from_qtensor(q)?),
    }
}
