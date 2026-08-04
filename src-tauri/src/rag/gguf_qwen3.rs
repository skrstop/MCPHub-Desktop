//! Qwen3 GGUF architecture strategy (bidirectional/mean OR causal/last, driven
//! by `pooling_type`): 2-norm pre-norm layers (ln1 before attn, ln2 before
//! MLP), per-head q_norm/k_norm RMSNorm, RoPE, GQA, gated SwiGLU MLP. No
//! sliding window (Qwen3 supports it but F2LLMv2-style embedders don't).
//!
//! Pooling (`qwen3.pooling_type`): 1=mean (bidirectional), 2=cls
//! (bidirectional, first token), 3=last (causal, last real token). F2LLMv2 is
//! pooling_type=3 (last). Verified against candle-transformers 0.11 `qwen3.rs`
//! + the F2LLMv2 80M_Q4_K_M GGUF tensor names.

use std::io::{Read, Seek};

use anyhow::{anyhow, Result};
use candle_core::quantized::gguf_file::Content;
use candle_core::quantized::QMatMul;
use candle_core::{DType, Device, Module, Tensor};
use candle_nn::{ops, RmsNorm};

use crate::rag::gguf_gemma::GgufArch;

/// One Qwen3 decoder layer (pre-norm: 2 RMSNorms + attention + MLP).
struct Qwen3Layer {
    ln1: RmsNorm,
    ln2: RmsNorm,
    wq: QMatMul,
    wk: QMatMul,
    wv: QMatMul,
    wo: QMatMul,
    q_norm: RmsNorm,
    k_norm: RmsNorm,
    gate: QMatMul,
    up: QMatMul,
    down: QMatMul,
}

/// Qwen3 embedding model (quantized GGUF). Attention direction + pooling follow
/// `pooling_type`: mean/cls -> bidirectional; last -> causal + last token.
pub struct Qwen3EmbedArch {
    token_embd: Tensor,
    layers: Vec<Qwen3Layer>,
    final_norm: RmsNorm,
    hidden: usize,
    n_head: usize,
    n_kv_head: usize,
    head_dim: usize,
    max_context: u32,
    pooling_type: u32,
    rope_base: f32,
    device: Device,
}

impl Qwen3EmbedArch {
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

        let hidden = get_u32(&format!("{}embedding_length", pfx), None)?;
        let block_count = get_u32(&format!("{}block_count", pfx), None)?;
        let n_head = get_u32(&format!("{}attention.head_count", pfx), None)?;
        let n_kv_head = get_u32(&format!("{}attention.head_count_kv", pfx), None)?;
        let head_dim = get_u32(&format!("{}attention.key_length", pfx), Some(hidden / n_head))?;
        let max_context = get_u32(&format!("{}context_length", pfx), Some(2048))? as u32;
        let rope_base = get_f32(&format!("{}rope.freq_base", pfx), 1_000_000.0);
        let rms_eps = get_f32(&format!("{}attention.layer_norm_rms_epsilon", pfx), 1e-6) as f64;
        // pooling_type: 0=none, 1=mean, 2=cls, 3=last. Default mean for embedders.
        let pooling_type = get_u32(&format!("{}pooling_type", pfx), Some(1))? as u32;

        let token_embd = content.tensor(file, "token_embd.weight", device)?.dequantize(device)?;
        let final_norm = rms_norm(file, content, "output_norm.weight", device, rms_eps)?;

        let mut layers = Vec::with_capacity(block_count);
        for i in 0..block_count {
            layers.push(Qwen3Layer {
                ln1: rms_norm(file, content, &format!("blk.{i}.attn_norm.weight"), device, rms_eps)?,
                ln2: rms_norm(file, content, &format!("blk.{i}.ffn_norm.weight"), device, rms_eps)?,
                wq: qmatmul(file, content, &format!("blk.{i}.attn_q.weight"), device)?,
                wk: qmatmul(file, content, &format!("blk.{i}.attn_k.weight"), device)?,
                wv: qmatmul(file, content, &format!("blk.{i}.attn_v.weight"), device)?,
                wo: qmatmul(file, content, &format!("blk.{i}.attn_output.weight"), device)?,
                q_norm: rms_norm(file, content, &format!("blk.{i}.attn_q_norm.weight"), device, rms_eps)?,
                k_norm: rms_norm(file, content, &format!("blk.{i}.attn_k_norm.weight"), device, rms_eps)?,
                gate: qmatmul(file, content, &format!("blk.{i}.ffn_gate.weight"), device)?,
                up: qmatmul(file, content, &format!("blk.{i}.ffn_up.weight"), device)?,
                down: qmatmul(file, content, &format!("blk.{i}.ffn_down.weight"), device)?,
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
            pooling_type,
            rope_base,
            device: device.clone(),
        })
    }
}

impl GgufArch for Qwen3EmbedArch {
    fn hidden_dim(&self) -> usize {
        self.hidden
    }

    fn max_context(&self) -> u32 {
        self.max_context
    }

    fn forward_embed(&self, input_ids: &Tensor, attention_mask: &Tensor) -> Result<Tensor> {
        let (b, seq) = input_ids.dims2()?;
        let ids_flat = input_ids.flatten_all()?;
        let mut x = self.token_embd.embedding(&ids_flat)?;
        x = x.reshape((b, seq, self.hidden))?;

        // Padding bias [b,1,1,seq]: 0 real / -1e9 pad.
        let pad_bias = attn_bias(attention_mask)?;
        // Causal bias [1,1,seq,seq]: 0 lower-tri / -1e9 upper (for last-token pooling).
        let causal = self.pooling_type == 3;
        let causal_bias = if causal {
            Some(causal_mask(seq, &self.device)?)
        } else {
            None
        };
        let positions = Tensor::arange(0u32, seq as u32, &self.device)?;

        for layer in self.layers.iter() {
            x = forward_layer(
                layer, &x, &positions, &pad_bias, causal_bias.as_ref(),
                self.head_dim, self.n_head, self.n_kv_head, self.rope_base, &self.device,
            )?;
        }
        let x = self.final_norm.forward(&x)?;
        pool(&x, attention_mask, self.pooling_type)
    }
}

/// One Qwen3 layer (pre-norm):
///   x = x + attn(ln1(x))
///   x = x + mlp(ln2(x))
fn forward_layer(
    l: &Qwen3Layer,
    x: &Tensor,
    positions: &Tensor,
    pad_bias: &Tensor,
    causal_bias: Option<&Tensor>,
    head_dim: usize,
    n_head: usize,
    n_kv_head: usize,
    rope_base: f32,
    device: &Device,
) -> Result<Tensor> {
    let (b, seq, _hidden) = x.dims3()?;
    let h = l.ln1.forward(x)?;
    let q = l.wq.forward(&h)?;
    let k = l.wk.forward(&h)?;
    let v = l.wv.forward(&h)?;
    let q = q.reshape((b, seq, n_head, head_dim))?;
    let k = k.reshape((b, seq, n_kv_head, head_dim))?;
    let v = v.reshape((b, seq, n_kv_head, head_dim))?;
    // Per-head q/k RMSNorm (over head_dim).
    let q = l.q_norm.forward(&q)?;
    let k = l.k_norm.forward(&k)?;
    let q = apply_rope(&q, positions, head_dim, rope_base, device)?;
    let k = apply_rope(&k, positions, head_dim, rope_base, device)?;
    let rep = n_head / n_kv_head;
    let k = k.repeat((1, 1, rep, 1))?;
    let v = v.repeat((1, 1, rep, 1))?;
    let q = q.transpose(1, 2)?.contiguous()?;
    let k = k.transpose(1, 2)?.contiguous()?;
    let v = v.transpose(1, 2)?.contiguous()?;
    let scale = 1.0f64 / (head_dim as f64).sqrt();
    let mut scores = q.matmul(&k.t()?)?.affine(scale, 0.0)?;
    if let Some(cb) = causal_bias {
        scores = scores.broadcast_add(cb)?;
    }
    scores = scores.broadcast_add(pad_bias)?;
    let probs = ops::softmax_last_dim(&scores)?;
    let attn = probs.matmul(&v.contiguous()?)?;
    let attn = attn.transpose(1, 2)?.reshape((b, seq, n_head * head_dim))?;
    let attn = l.wo.forward(&attn)?;
    let residual = (x + &attn)?;
    let h2 = l.ln2.forward(&residual)?;
    let gate = l.gate.forward(&h2)?.silu()?;
    let up = l.up.forward(&h2)?;
    let mlp = l.down.forward(&(&gate * &up)?)?;
    Ok((residual + &mlp)?)
}

/// Padding bias [b,1,1,seq]: 0 real / -1e9 pad.
fn attn_bias(mask: &Tensor) -> Result<Tensor> {
    let (b, s) = mask.dims2()?;
    let mask_f = mask.to_dtype(DType::F32)?;
    let bias = mask_f.affine(-1.0, 1.0)?.affine(-1e9, 0.0)?;
    Ok(bias.reshape((b, 1, 1, s))?)
}

/// Causal bias [1,1,seq,seq]: 0 where j<=i (can see), -1e9 where j>i (future).
/// Built on CPU as a Vec (index math in Rust) - avoids Metal op gaps.
fn causal_mask(seq: usize, device: &Device) -> Result<Tensor> {
    let mut m = vec![0f32; seq * seq];
    for i in 0..seq {
        for j in 0..seq {
            if j > i {
                m[i * seq + j] = -1e9;
            }
        }
    }
    Ok(Tensor::from_vec(m, (1, 1, seq, seq), device)?)
}

/// RoPE (rotate-half) over `head_dim`. cos/sin precomputed per-seq.
fn apply_rope(
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

/// Pool over the sequence dim per `pooling_type`, then L2-normalize. [b,seq,hidden] -> [b,hidden].
///   1 (mean): masked mean over real tokens.
///   2 (cls):  first token (position 0).
///   3 (last): last real token (per row, via gather).
///   _:       default to last real token.
fn pool(x: &Tensor, mask: &Tensor, pooling_type: u32) -> Result<Tensor> {
    let (b, _seq, _hidden) = x.dims3()?;
    let pooled = match pooling_type {
        1 => {
            // masked mean
            let (b2, s) = mask.dims2()?;
            let mask_f = mask.to_dtype(DType::F32)?.reshape((b2, s, 1))?;
            let masked = x.broadcast_mul(&mask_f)?;
            let sum = masked.sum(1)?;
            let denom = mask_f.sum(1)?.broadcast_add(&Tensor::new(&[1e-9f32], x.device())?)?;
            sum.broadcast_div(&denom)?
        }
        2 => {
            // cls: first token (position 0) per row
            x.narrow(1, 0, 1)?.reshape((b, x.dim(2)?))?
        }
        _ => {
            // last real token per row: idx = mask.sum(1) - 1, gather along the
            // seq dim. indexes must broadcast to [b, 1, hidden] so each row's
            // single position is gathered across the full hidden dim
            // (out[b,0,k] = x[b, last_idx[b], k]).
            let mask_f = mask.to_dtype(DType::F32)?;
            let counts = mask_f.sum(1)?; // [b]
            let idx = counts.affine(1.0, -1.0)?.to_dtype(DType::I64)?; // [b], last pos per row
            let hidden = x.dim(2)?;
            let idx = idx
                .reshape((b, 1, 1))?
                .broadcast_as((b, 1, hidden))?
                .contiguous()?; // [b, 1, hidden], value=last_idx[b]
            let gathered = x.gather(&idx, 1)?; // [b, 1, hidden]
            gathered.reshape((b, hidden))?
        }
    };
    // L2-normalize each row.
    let norm = pooled
        .sqr()?
        .sum(1)?
        .sqrt()?
        .broadcast_add(&Tensor::new(&[1e-12f32], x.device())?)?
        .reshape((b, 1))?;
    Ok(pooled.broadcast_div(&norm)?)
}

fn rms_norm<R: Read + Seek>(
    reader: &mut R,
    ct: &Content,
    name: &str,
    device: &Device,
    eps: f64,
) -> Result<RmsNorm> {
    let w = ct.tensor(reader, name, device)?.dequantize(device)?;
    Ok(RmsNorm::new(w, eps))
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
