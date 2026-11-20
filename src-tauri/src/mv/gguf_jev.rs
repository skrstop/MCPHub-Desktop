//! GGUF architecture strategy for `qwen35`-arch decision models
//! (Jev-Style-0.8B-Decision-v3, a fine-tune of chaoliangUNSW's Qwen3.5-0.8B
//! hybrid decoder). Semantics transcribed from the official runtime
//! (`jev_style_decision.py`, Apache-2.0) and the transformers
//! `qwen3_5` implementation:
//!
//! - Hybrid decoder, 24 layers: every 4th layer is full GQA attention
//!   (8 Q heads / 2 KV heads, head_dim 256, per-head RMS q/k norm, partial
//!   mRoPE on the first 64 dims — for text-only input the three mRoPE grids
//!   collapse to standard RoPE — and a sigmoid gate on the attention output);
//!   the rest are GatedDeltaNet linear-attention layers (depthwise causal
//!   conv1d k=4 + silu, L2-normalized q/k, per-token gated delta rule with
//!   state [16 heads, 128, 128], gated RMSNorm output).
//! - Decision = prefill-only "verdict" readout (macjev-render-v1): the prompt
//!   ends with `<option> ->` lines; the score of option k is
//!   `h_slot · (w_yes − w_no)` at the k-th " ->" token, computed from the
//!   final-normed hidden state and the tied embedding rows (float32).
//! - Probabilities = softmax(scores / T); with no category (our schema has
//!   none) T = `temperatures.global` = 0.8800546821789332, clamped [0.3, 5].
//!
//! The tokenizer ships INSIDE the GGUF (gpt2 BPE, Qwen3 split pre-tokenizer,
//! 248320 vocab) — no auxiliary tokenizer.json needed.

use crate::mv::decision::{DecisionOutput, DecisionRequest, DecisionType};
use anyhow::{anyhow, Result};
use candle_core::quantized::gguf_file::{Content, Value};
use candle_core::{DType, Device, Tensor};
use std::collections::HashMap;
use std::io::{Read, Seek};
use tokenizers::Tokenizer;

// readout_config.json constants (macjev-readout-v1)
const SLOT_YES: u32 = 9542; // " yes"
const SLOT_NO: u32 = 874; // " no"
const SLOT_ARROW: u32 = 1411; // " ->"
const TEMPERATURE_GLOBAL: f32 = 0.8800546821789332;
const TEMPERATURE_CLAMP: (f32, f32) = (0.3, 5.0);
const CONTEXT_LIMIT: usize = 25_600;
const HEAD_MAX: usize = 2_048;

/// One GatedDeltaNet (linear attention) layer.
struct SsmLayer {
    attn_norm: Tensor,
    wqkv: Tensor,  // [in, 6144] (q 2048 | k 2048 | v 2048)
    wgate: Tensor, // [in, 2048] (z gate, 16 heads x 128)
    conv: Tensor,  // [6144, 4] f32 (channel, kernel)
    a: Tensor,     // [16] f32 (= -exp(A_log))
    dt: Tensor,    // [16] f32 bias
    walpha: Tensor, // [in, 16]
    wbeta: Tensor,  // [in, 16]
    norm: Tensor,   // [128]
    wout: Tensor,   // [in, 2048]
    post_norm: Tensor,
    ffn_gate: Tensor, // [in, ff]
    ffn_up: Tensor,   // [in, ff]
    ffn_down: Tensor, // [ff, in]
}

/// One full-attention layer (Q carries a fused per-head gate).
struct AttnLayer {
    attn_norm: Tensor,
    wq: Tensor,    // [in, 4096] per-head [q 256 | gate 256] x 8
    wk: Tensor,    // [in, 512]
    wv: Tensor,    // [in, 512]
    wo: Tensor,    // [in, 2048]
    q_norm: Tensor, // [256]
    k_norm: Tensor, // [256]
    post_norm: Tensor,
    ffn_gate: Tensor,
    ffn_up: Tensor,
    ffn_down: Tensor,
}

enum Layer {
    Ssm(SsmLayer),
    Attn(AttnLayer),
}

pub struct JevModel {
    pub tokenizer: Tokenizer,
    device: Device,
    eps: f64,
    rope_theta: f32,
    rope_dim: usize, // 64 rotary dims of the 256-dim head
    n_head: usize,        // 8
    n_kv_head: usize,     // 2
    head_dim: usize,      // 256
    n_v_head: usize,      // 16 (ssm)
    head_k_dim: usize,    // 128 (ssm)
    head_v_dim: usize,    // 128 (ssm)
    conv_kernel: usize,   // 4
    tok_embd: Tensor, // [vocab, d] (tied output)
    final_norm: Tensor,
    layers: Vec<Layer>,
}

/// GGUF linear `[out, in]` → dequantized transposed `[in, out]`.
fn lin<'a, R: Read + Seek>(file: &mut R, ct: &'a Content, name: &str, device: &Device) -> Result<Tensor> {
    Ok(ct
        .tensor(file, name, device)?
        .dequantize(device)?
        .t()?
        .contiguous()?)
}

/// GGUF f32 vector/matrix → f32 tensor (kept in GGUF orientation).
fn vec_t<'a, R: Read + Seek>(file: &mut R, ct: &'a Content, name: &str, device: &Device) -> Result<Tensor> {
    Ok(ct.tensor(file, name, device)?.dequantize(device)?)
}

fn meta_u32(md: &HashMap<String, Value>, key: &str) -> Result<u32> {
    match md.get(key) {
        Some(Value::U32(x)) => Ok(*x),
        Some(v) => Err(anyhow!("{key}: expected u32, got {v:?}")),
        None => Err(anyhow!("{key}: missing")),
    }
}

fn meta_f32(md: &HashMap<String, Value>, key: &str) -> Result<f32> {
    match md.get(key) {
        Some(Value::F32(x)) => Ok(*x),
        Some(v) => Err(anyhow!("{key}: expected f32, got {v:?}")),
        None => Err(anyhow!("{key}: missing")),
    }
}


// -- math helpers ---------------------------------------------------------------------------

fn rms(x: &Tensor, w: &Tensor, eps: f64) -> Result<Tensor> {
    let x = x.to_dtype(DType::F32)?;
    let var = x.sqr()?.mean_keepdim(candle_core::D::Minus1)?;
    let normed = x.broadcast_mul(&var.affine(1.0, eps)?.sqrt()?.recip()?)?;
    Ok(normed.broadcast_mul(&w.to_dtype(DType::F32)?)?)
}

/// Per-head RMS over the last dim (used for q/k norm on head_dim 256).
fn rms_head(x: &Tensor, w: &Tensor, eps: f64) -> Result<Tensor> {
    rms(x, w, eps)
}

fn l2norm(x: &Tensor, eps: f64) -> Result<Tensor> {
    let x = x.to_dtype(DType::F32)?;
    let inv = x
        .sqr()?
        .sum_keepdim(candle_core::D::Minus1)?
        .affine(1.0, eps)?
        .sqrt()?
        .recip()?;
    Ok(x.broadcast_mul(&inv)?)
}

fn silu(x: &Tensor) -> Result<Tensor> {
    let x = x.to_dtype(DType::F32)?;
    Ok(x.broadcast_mul(&candle_nn::ops::sigmoid(&x)?)?)
}

fn softplus(x: &Tensor) -> Result<Tensor> {
    // log1p(exp(x)) computed stably: max(x,0) + log1p(exp(-|x|))
    let x = x.to_dtype(DType::F32)?;
    let abs = x.abs()?;
    let pos = x.clamp(0.0, f32::MAX)?;
    let neg = abs.affine(-1.0, 0.0)?.exp()?.affine(1.0, 1.0)?.log()?;
    Ok((pos + neg)?)
}

impl JevModel {
    pub fn load<R: Read + Seek>(file: &mut R, device: &Device) -> Result<Self> {
        let ct = Content::read(file).map_err(|e| anyhow!("read gguf: {e}"))?;
        let ct = &ct;
        let md = &ct.metadata;
        let n_layers = meta_u32(md, "qwen35.block_count")? as usize;
        let eps = meta_f32(md, "qwen35.attention.layer_norm_rms_epsilon")? as f64;
        let rope_theta = meta_f32(md, "qwen35.rope.freq_base")?;
        let rope_dim = meta_u32(md, "qwen35.rope.dimension_count")? as usize;
        let n_head = meta_u32(md, "qwen35.attention.head_count")? as usize;
        let n_kv_head = meta_u32(md, "qwen35.attention.head_count_kv")? as usize;
        let head_dim = meta_u32(md, "qwen35.attention.key_length")? as usize;
        let n_v_head = meta_u32(md, "qwen35.ssm.time_step_rank")? as usize;
        let head_k_dim = meta_u32(md, "qwen35.ssm.state_size")? as usize;
        let head_v_dim = head_k_dim;
        let conv_kernel = meta_u32(md, "qwen35.ssm.conv_kernel")? as usize;
        let recurrent: Vec<u32> = match md.get("qwen35.attention.recurrent_layers") {
            Some(Value::Array(a)) => a
                .iter()
                .map(|v| match v {
                    Value::U32(x) => Ok(*x),
                    Value::I32(x) => Ok(*x as u32),
                    Value::Bool(b) => Ok(*b as u32),
                    _ => Err(anyhow!("recurrent_layers: bad entry {v:?}")),
                })
                .collect::<Result<Vec<u32>>>()?,
            _ => (0..n_layers as u32).map(|i| if i % 4 == 3 { 0 } else { 1 }).collect(),
        };

        let tok_embd = ct.tensor(file, "token_embd.weight", device)?.dequantize(device)?;
        let final_norm = vec_t(file, ct, "output_norm.weight", device)?;

        let mut layers = Vec::with_capacity(n_layers);
        for i in 0..n_layers {
            let attn_norm = vec_t(file, ct, &format!("blk.{i}.attn_norm.weight"), device)?;
            let post_norm = vec_t(file, ct, &format!("blk.{i}.post_attention_norm.weight"), device)?;
            let ffn_gate = lin(file, ct, &format!("blk.{i}.ffn_gate.weight"), device)?;
            let ffn_up = lin(file, ct, &format!("blk.{i}.ffn_up.weight"), device)?;
            let ffn_down = lin(file, ct, &format!("blk.{i}.ffn_down.weight"), device)?;
            let layer = if recurrent.get(i).copied().unwrap_or(1) == 1 {
                Layer::Ssm(SsmLayer {
                    attn_norm,
                    wqkv: lin(file, ct, &format!("blk.{i}.attn_qkv.weight"), device)?,
                    wgate: lin(file, ct, &format!("blk.{i}.attn_gate.weight"), device)?,
                    conv: vec_t(file, ct, &format!("blk.{i}.ssm_conv1d.weight"), device)?,
                    a: vec_t(file, ct, &format!("blk.{i}.ssm_a"), device)?,
                    dt: vec_t(file, ct, &format!("blk.{i}.ssm_dt.bias"), device)?,
                    walpha: lin(file, ct, &format!("blk.{i}.ssm_alpha.weight"), device)?,
                    wbeta: lin(file, ct, &format!("blk.{i}.ssm_beta.weight"), device)?,
                    norm: vec_t(file, ct, &format!("blk.{i}.ssm_norm.weight"), device)?,
                    wout: lin(file, ct, &format!("blk.{i}.ssm_out.weight"), device)?,
                    post_norm,
                    ffn_gate,
                    ffn_up,
                    ffn_down,
                })
            } else {
                Layer::Attn(AttnLayer {
                    attn_norm,
                    wq: lin(file, ct, &format!("blk.{i}.attn_q.weight"), device)?,
                    wk: lin(file, ct, &format!("blk.{i}.attn_k.weight"), device)?,
                    wv: lin(file, ct, &format!("blk.{i}.attn_v.weight"), device)?,
                    wo: lin(file, ct, &format!("blk.{i}.attn_output.weight"), device)?,
                    q_norm: vec_t(file, ct, &format!("blk.{i}.attn_q_norm.weight"), device)?,
                    k_norm: vec_t(file, ct, &format!("blk.{i}.attn_k_norm.weight"), device)?,
                    post_norm,
                    ffn_gate,
                    ffn_up,
                    ffn_down,
                })
            };
            layers.push(layer);
        }

        // Tokenizer: gpt2 BPE with the Qwen3 split pre-tokenizer (identical
        // regex in the official tokenizer.json).
        let tokenizer = crate::mv::gguf::build_qwen3_tokenizer(md)?;
        // Readout sanity: the three slot tokens must encode to single ids.
        for (text, want) in [(" yes", SLOT_YES), (" no", SLOT_NO), (" ->", SLOT_ARROW)] {
            let got = tokenizer
                .encode(text, false)
                .map_err(|e| anyhow!("tokenizer encode {text:?}: {e}"))?
                .get_ids()
                .to_vec();
            if got != [want] {
                return Err(anyhow!("tokenizer mismatch: {text:?} -> {got:?}, expect [{want}]"));
            }
        }

        Ok(Self {
            tokenizer,
            device: device.clone(),
            eps,
            rope_theta,
            rope_dim,
            n_head,
            n_kv_head,
            head_dim,
            n_v_head,
            head_k_dim,
            head_v_dim,
            conv_kernel,
            tok_embd,
            final_norm,
            layers,
        })
    }

    // -- rendering (macjev-render-v1) -------------------------------------------------------

    fn render_options(req: &DecisionRequest) -> Vec<String> {
        match req.decision_type {
            // options arrive pre-rendered ("name" or "name: description")
            DecisionType::Choice => req.options.clone(),
            DecisionType::Score => req
                .options
                .iter()
                .enumerate()
                .map(|(i, c)| format!("level {i}: {c}"))
                .collect(),
            DecisionType::Noul => vec![
                "false: no, the statement does not hold".to_string(),
                "true: yes, the statement holds".to_string(),
            ],
        }
    }

    /// Returns (ids, slot positions of the " ->" tokens).
    fn render(&self, req: &DecisionRequest) -> Result<(Vec<u32>, Vec<usize>)> {
        let enc = |s: &str| -> Result<Vec<u32>> {
            Ok(self
                .tokenizer
                .encode(s, false)
                .map_err(|e| anyhow!("encode: {e}"))?
                .get_ids()
                .to_vec())
        };
        let opts = Self::render_options(req);
        let mut prefix = enc("State:\n")?;
        prefix.extend(enc(&req.state)?);
        prefix.extend(enc("\n\n")?);

        let mut suffix = enc(&format!("Question [{}]: {}\nOptions:\n", req.decision_type.as_str(), req.question))?;
        for o in &opts {
            suffix.extend(enc("- ")?);
            suffix.extend(enc(o)?);
            suffix.extend(enc("\n")?);
        }
        suffix.extend(enc("Judge each option:\n")?);
        let mut slots = Vec::with_capacity(opts.len());
        for o in &opts {
            suffix.extend(enc(o)?);
            suffix.extend(enc(" ->")?);
            slots.push(prefix.len() + suffix.len() - 1);
            suffix.extend(enc("\n")?);
        }
        if suffix.len() > HEAD_MAX {
            return Err(anyhow!(
                "question + options + readout need {} tokens; the head budget is {HEAD_MAX}",
                suffix.len()
            ));
        }
        let ids = [prefix, suffix].concat();
        if ids.len() > CONTEXT_LIMIT {
            return Err(anyhow!(
                "input needs {} tokens; the limit is {CONTEXT_LIMIT}",
                ids.len()
            ));
        }
        Ok((ids, slots))
    }

    // -- forward ----------------------------------------------------------------------------

    fn rope_cos_sin(&self, seq: usize) -> Result<(Tensor, Tensor)> {
        // text-only input: all three mRoPE grids use position = token index,
        // so the recomposed frequencies equal standard RoPE over `rope_dim`.
        let half = self.rope_dim / 2;
        let theta = self.rope_theta as f64;
        let inv_freq: Vec<f32> = (0..half)
            .map(|i| (theta.powf(-(2.0 * i as f64) / self.rope_dim as f64)) as f32)
            .collect();
        let inv = Tensor::from_vec(inv_freq, (1, half), &self.device)?;
        let pos: Vec<f32> = (0..seq).map(|p| p as f32).collect();
        let pos = Tensor::from_vec(pos, (seq, 1), &self.device)?;
        let ang = pos.matmul(&inv)?; // [seq, half]
        let cos = ang.cos()?;
        let sin = ang.sin()?;
        // duplicate to [seq, rope_dim] (cat(freqs, freqs))
        Ok((Tensor::cat(&[&cos, &cos], 1)?, Tensor::cat(&[&sin, &sin], 1)?))
    }

    /// Partial NeoX RoPE on the first `rope_dim` dims of a [seq, H, D] tensor.
    fn apply_partial_rope(&self, x: &Tensor, cos: &Tensor, sin: &Tensor) -> Result<Tensor> {
        let rd = self.rope_dim;
        let half = rd / 2;
        let rot = x.narrow(candle_core::D::Minus1, 0, rd)?;
        let r1 = rot.narrow(candle_core::D::Minus1, 0, half)?;
        let r2 = rot.narrow(candle_core::D::Minus1, half, half)?;
        // cos/sin [seq, rd] -> [seq, 1, rd] -> split halves [seq, 1, half]
        let cos = cos.unsqueeze(1)?.narrow(candle_core::D::Minus1, 0, half)?;
        let sin = sin.unsqueeze(1)?.narrow(candle_core::D::Minus1, 0, half)?;
        let o1 = ((r1.broadcast_mul(&cos))? - r2.broadcast_mul(&sin)?)?;
        let o2 = ((r2.broadcast_mul(&cos))? + r1.broadcast_mul(&sin)?)?;
        let rot_new = Tensor::cat(&[o1, o2], candle_core::D::Minus1)?;
        Ok(Tensor::cat(
            &[rot_new, x.narrow(candle_core::D::Minus1, rd, self.head_dim - rd)?],
            candle_core::D::Minus1,
        )?)
    }

    fn ffn(&self, x: &Tensor, gate_w: &Tensor, up_w: &Tensor, down_w: &Tensor) -> Result<Tensor> {
        let g = silu(&x.matmul(gate_w)?)?;
        let u = x.matmul(up_w)?;
        Ok(g.broadcast_mul(&u)?.matmul(down_w)?)
    }

    fn ssm_forward(&self, l: &SsmLayer, x: &Tensor) -> Result<Tensor> {
        let (seq, _) = x.dims2()?;
        let qkv = x.matmul(&l.wqkv)?; // [seq, 6144]
        // depthwise causal conv1d (zero left pad) + silu
        let mut acc = Tensor::zeros((seq, self.conv_channels()), DType::F32, &self.device)?;
        let padded = qkv.pad_with_zeros(0, self.conv_kernel - 1, 0)?;
        for j in 0..self.conv_kernel {
            let wj = l.conv.narrow(1, j, 1)?.squeeze(1)?; // [channels]
            let shifted = padded.narrow(0, j, seq)?;
            acc = (acc + shifted.broadcast_mul(&wj.unsqueeze(0)?)?)?;
        }
        let conv = silu(&acc)?;
        let kd = self.head_k_dim;
        let vd = self.head_v_dim;
        let q = conv.narrow(1, 0, self.n_v_head * kd)?.reshape((seq, self.n_v_head, kd))?;
        let k = conv
            .narrow(1, self.n_v_head * kd, self.n_v_head * kd)?
            .reshape((seq, self.n_v_head, kd))?;
        let v = conv
            .narrow(1, 2 * self.n_v_head * kd, self.n_v_head * vd)?
            .reshape((seq, self.n_v_head, vd))?;
        let q = l2norm(&q, 1e-6)?.affine(1.0 / (kd as f64).sqrt(), 0.0)?;
        let k = l2norm(&k, 1e-6)?;
        let beta = candle_nn::ops::sigmoid(&x.matmul(&l.wbeta)?)?; // [seq, 16]
        let g = softplus(&x.matmul(&l.walpha)?.broadcast_add(&l.dt.unsqueeze(0)?)?)?
            .broadcast_mul(&l.a.unsqueeze(0)?)?; // [seq, 16] (negative)
        let z = x.matmul(&l.wgate)?.reshape((seq, self.n_v_head, vd))?;

        // per-token gated delta rule scan, state [heads, k_dim, v_dim] f32
        let mut state = Tensor::zeros((self.n_v_head, kd, vd), DType::F32, &self.device)?;
        let mut outs = Vec::with_capacity(seq);
        for t in 0..seq {
            let gt = g.narrow(0, t, 1)?.squeeze(0)?; // [16]
            let kt = k.narrow(0, t, 1)?.squeeze(0)?; // [16, kd]
            let vt = v.narrow(0, t, 1)?.squeeze(0)?; // [16, vd]
            let qt = q.narrow(0, t, 1)?.squeeze(0)?; // [16, kd]
            let bt = beta.narrow(0, t, 1)?.squeeze(0)?; // [16]
            state = state.broadcast_mul(&gt.exp()?.unsqueeze(1)?.unsqueeze(2)?)?;
            // kv_mem = sum over k_dim of state * k -> [16, vd]
            let kv_mem = state.broadcast_mul(&kt.unsqueeze(2)?)?.sum(1)?;
            let delta = ((vt - kv_mem)?).broadcast_mul(&bt.unsqueeze(1)?)?; // [16, vd]
            state = (state + kt.unsqueeze(2)?.broadcast_mul(&delta.unsqueeze(1)?)?)?;
            let out_t = state.broadcast_mul(&qt.unsqueeze(2)?)?.sum(1)?; // [16, vd]
            outs.push(out_t);
        }
        let out = Tensor::stack(&outs, 0)?; // [seq, 16, vd]
        // gated RMSNorm: rms(out) * weight * silu(z)
        let out = rms(&out, &l.norm, self.eps)?;
        let out = out.broadcast_mul(&silu(&z)?)?;
        let out = out.reshape((seq, self.n_v_head * vd))?;
        let o = out.matmul(&l.wout)?;
        Ok(o)
    }

    fn conv_channels(&self) -> usize {
        self.n_v_head * self.head_k_dim * 3
    }

    fn attn_forward(&self, l: &AttnLayer, x: &Tensor, cos: &Tensor, sin: &Tensor) -> Result<Tensor> {
        let (seq, _) = x.dims2()?;
        let qf = x.matmul(&l.wq)?; // [seq, 4096]
        let qf = qf.reshape((seq, self.n_head, self.head_dim * 2))?;
        let gate = qf.narrow(2, self.head_dim, self.head_dim)?; // [seq, 8, 256]
        let q = rms_head(&qf.narrow(2, 0, self.head_dim)?, &l.q_norm, self.eps)?;
        let k = rms_head(
            &x.matmul(&l.wk)?.reshape((seq, self.n_kv_head, self.head_dim))?,
            &l.k_norm,
            self.eps,
        )?;
        let v = x.matmul(&l.wv)?.reshape((seq, self.n_kv_head, self.head_dim))?;
        let q = self.apply_partial_rope(&q, cos, sin)?; // [seq, 8, 256]
        let k = self.apply_partial_rope(&k, cos, sin)?; // [seq, 2, 256]

        // expand kv heads (interleave: each kv head serves n_rep q heads)
        let n_rep = self.n_head / self.n_kv_head;
        let kx = k
            .unsqueeze(2)?
            .expand((seq, self.n_kv_head, n_rep, self.head_dim))?
            .reshape((seq, self.n_head, self.head_dim))?
            .contiguous()?;
        let vx = v
            .unsqueeze(2)?
            .expand((seq, self.n_kv_head, n_rep, self.head_dim))?
            .reshape((seq, self.n_head, self.head_dim))?
            .contiguous()?;

        // [H, seq, D]
        let q = q.transpose(0, 1)?.contiguous()?;
        let kx = kx.transpose(0, 1)?.contiguous()?;
        let vx = vx.transpose(0, 1)?.contiguous()?;
        let scale = 1.0 / (self.head_dim as f64).sqrt();
        let scores = q.matmul(&kx.transpose(1, 2)?)?.affine(scale, 0.0)?;
        // causal mask
        let mut mask = vec![f32::NEG_INFINITY; seq * seq];
        for i in 0..seq {
            for j in 0..=i {
                mask[i * seq + j] = 0.0;
            }
        }
        let mask = Tensor::from_vec(mask, (1, seq, seq), &self.device)?;
        let scores = scores.broadcast_add(&mask)?;
        let probs = candle_nn::ops::softmax(&scores, candle_core::D::Minus1)?;
        let out = probs.matmul(&vx)?; // [8, seq, 256]
        let out = out.transpose(0, 1)?.reshape((seq, self.n_head * self.head_dim))?;
        // sigmoid gate
        let gate = candle_nn::ops::sigmoid(&gate.reshape((seq, self.n_head * self.head_dim))?)?;
        let out = out.broadcast_mul(&gate)?;
        Ok(out.matmul(&l.wo)?)
    }

    /// Full prefill; returns the final-normed hidden states [seq, d].
    fn forward(&self, ids: &[u32]) -> Result<Tensor> {
        let seq = ids.len();
        let ids_t = Tensor::from_vec(ids.to_vec(), (seq,), &self.device)?;
        let mut x = self.tok_embd.index_select(&ids_t, 0)?; // [seq, d]
        let (cos, sin) = self.rope_cos_sin(seq)?;
        for layer in self.layers.iter() {
            match layer {
                Layer::Ssm(l) => {
                    let h = rms(&x, &l.attn_norm, self.eps)?;
                    let m = self.ssm_forward(l, &h)?;
                    let x1 = (x + m)?;
                    let h2 = rms(&x1, &l.post_norm, self.eps)?;
                    x = (x1 + self.ffn(&h2, &l.ffn_gate, &l.ffn_up, &l.ffn_down)?)?;
                }
                Layer::Attn(l) => {
                    let h = rms(&x, &l.attn_norm, self.eps)?;
                    let m = self.attn_forward(l, &h, &cos, &sin)?;
                    let x1 = (x + m)?;
                    let h2 = rms(&x1, &l.post_norm, self.eps)?;
                    x = (x1 + self.ffn(&h2, &l.ffn_gate, &l.ffn_up, &l.ffn_down)?)?;
                }
            }
        }
        let out = rms(&x, &self.final_norm, self.eps)?;
        Ok(out)
    }

    pub fn decide(&self, req: &DecisionRequest) -> Result<DecisionOutput> {
        if req.options.len() < 2 || req.options.len() > 20 {
            return Err(anyhow!("options must contain 2-20 entries"));
        }
        let (ids, slots) = self.render(req)?;
        let hidden = self.forward(&ids)?; // [seq, d]
        let slot_ids = Tensor::from_vec(
            slots.iter().map(|s| *s as u32).collect::<Vec<_>>(),
            (slots.len(),),
            &self.device,
        )?;
        let hs = hidden.index_select(&slot_ids, 0)?; // [n, d]
        let w_yes = self.tok_embd.narrow(0, SLOT_YES as usize, 1)?; // [1, d]
        let w_no = self.tok_embd.narrow(0, SLOT_NO as usize, 1)?;
        let dir = (w_yes - w_no)?; // [1, d]
        let scores_t = hs.matmul(&dir.transpose(0, 1)?)?.squeeze(1)?; // [n]
        let scores: Vec<f32> = scores_t.to_vec1()?;

        let t = TEMPERATURE_GLOBAL.clamp(TEMPERATURE_CLAMP.0, TEMPERATURE_CLAMP.1);
        let max = scores.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let exps: Vec<f32> = scores.iter().map(|s| ((s - max) / t).exp()).collect();
        let sum: f32 = exps.iter().sum();
        let probs: Vec<f32> = exps.iter().map(|e| e / sum).collect();
        let index = probs
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap_or(0);
        Ok(DecisionOutput {
            decision_type: req.decision_type,
            scores,
            probabilities: probs,
            index,
            option: req.options[index].clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;
    use std::path::PathBuf;

    const TEMP_GGUF: &str =
        "runtimes/decision/model/jev-style/0.8b/model.gguf";

    fn load_model() -> Result<JevModel> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(TEMP_GGUF);
        let f = std::fs::File::open(&path)?;
        let mut r = BufReader::new(f);
        JevModel::load(&mut r, &Device::Cpu)
    }

    /// Parity probe against the official runtime
    /// (jev_style_decision.py, transformers Qwen3_5ForCausalLM, fp32 CPU).
    /// Official reference values (scores, probs) were produced with
    /// `JevStyleDecision(model_dir, device='cpu', dtype='float32')` on the
    /// same 8 cases; T = temperatures.global = 0.8800546821789332.
    ///
    /// `cargo test --lib mv::gguf_jev::tests::jev_probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn jev_probe() {
        let m = load_model().expect("load");
        // official reference: (qtype, state, question, options, official scores, official probs)
        let cases: Vec<(DecisionType, &str, &str, Vec<&str>, [f32; 4], [f32; 4])> = vec![
            (
                DecisionType::Choice,
                "对话进行中",
                "用户想退出程序",
                vec!["继续运行", "退出程序"],
                [0.5019, 0.1845, f32::NAN, f32::NAN],
                [0.5892, 0.4108, f32::NAN, f32::NAN],
            ),
            (
                DecisionType::Choice,
                "对话进行中",
                "1+1",
                vec!["2", "3"],
                [-0.8099, -1.5486, f32::NAN, f32::NAN],
                [0.6983, 0.3017, f32::NAN, f32::NAN],
            ),
            (
                DecisionType::Noul,
                "对话进行中",
                "今天天气很好",
                vec!["false", "true"],
                [-0.0261, -2.3273, f32::NAN, f32::NAN],
                [0.9318, 0.0682, f32::NAN, f32::NAN],
            ),
            (
                DecisionType::Score,
                "对话进行中",
                "这段文字的情感倾向",
                vec!["负面", "正面"],
                [-1.4753, -0.1773, f32::NAN, f32::NAN],
                [0.1862, 0.8138, f32::NAN, f32::NAN],
            ),
            (
                DecisionType::Choice,
                "I was charged twice for March, please refund the duplicate today or we cancel.",
                "Which department should handle this request?",
                vec![
                    "billing: invoices, payments, refunds",
                    "technical: bugs, outages, system errors",
                    "sales: pricing, new contracts",
                    "other: everything else",
                ],
                [2.9391, -2.0317, -1.7519, -1.0597],
                [0.9814, 0.0035, 0.0048, 0.0104],
            ),
            (
                DecisionType::Choice,
                "I was charged twice for March, please refund the duplicate today or we cancel.",
                "What is the sentiment of the message?",
                vec!["positive", "neutral", "negative"],
                [-2.0445, 0.0025, -0.0232, f32::NAN],
                [0.0472, 0.4834, 0.4694, f32::NAN],
            ),
            (
                DecisionType::Score,
                "I was charged twice for March, please refund the duplicate today or we cancel.",
                "How urgent is this request?",
                vec!["not urgent", "soon", "critical deadline or blocking issue"],
                [-0.9073, 1.1198, 1.2324, f32::NAN],
                [0.0447, 0.4472, 0.5082, f32::NAN],
            ),
            (
                DecisionType::Noul,
                "I was charged twice for March, please refund the duplicate today or we cancel.",
                "Does the user threaten to cancel or leave?",
                vec!["false", "true"],
                [0.3145, 2.1011, f32::NAN, f32::NAN],
                [0.1161, 0.8839, f32::NAN, f32::NAN],
            ),
        ];
        for (qt, st, q, opts, ref_scores, ref_probs) in cases {
            let req = DecisionRequest {
                decision_type: qt,
                state: st.to_string(),
                question: q.to_string(),
                options: opts.iter().map(|s| s.to_string()).collect(),
            };
            let (ids, slots) = m.render(&req).expect("render");
            let t0 = std::time::Instant::now();
            let out = m.decide(&req).expect("decide");
            let dt = t0.elapsed().as_millis();
            println!(
                "{qt:?} q={q:?} ids={} slots={slots:?} ({}ms)\n  ours  scores={:?} probs={:?}",
                ids.len(),
                dt,
                fmt(&out.scores),
                fmt(&out.probabilities)
            );
            println!(
                "  offic scores={:?} probs={:?}",
                fmt(&ref_scores[..opts.len()]),
                fmt(&ref_probs[..opts.len()])
            );
            let max_diff = out
                .scores
                .iter()
                .zip(ref_scores.iter())
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            println!("  max |Δscore| = {max_diff:.4}, argmax ours={} official={}",
                out.index,
                ref_probs[..opts.len()].iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).map(|(i, _)| i).unwrap());
            assert!(max_diff < 0.35, "score deviation too large: {max_diff}");
        }
    }

    fn fmt(v: &[f32]) -> Vec<String> {
        v.iter()
            .filter(|x| x.is_finite())
            .map(|x| format!("{x:.4}"))
            .collect()
    }
}
