//! GGUF architecture strategy for `lfm2` (LiquidAI LFM2.5-Embedding), a hybrid
//! ShortConv + attention encoder.
//!
//! ## Architecture (verified against `LiquidAI/LFM2.5-Embedding-350M`'
//! `modeling_lfm2_bidirectional.py` + transformers `modeling_lfm2.py`)
//! 16 blocks, pre-norm RMSNorm (eps 1e-5, standard - no +1 shift). Two block
//! types, detected by tensor presence (`blk.N.attn_q.weight`):
//!   - **Attention blocks** (every 3rd: 2,5,8,10,12,14): GQA (16 Q heads, 8 KV
//!     heads, head_dim 64) with per-head q/k RMSNorm, full RoPE (NeoX
//!     rotate-half, base 1e6 - identical to Gemma3's `apply_rope`), bidirectional
//!     (no causal mask). SwiGLU FFN (`w2(silu(w1(x)) * w3(x))`).
//!   - **ShortConv blocks** (the rest): `in_proj` (H->3H) split into (B, C, x),
//!     `Bx = B*x`, depthwise conv1d (kernel 3, **non-causal** symmetric pad via
//!     the patched `_noncausal_shortconv_forward`), `y = C*conv`, `out_proj`
//!     (H->H). SwiGLU FFN. Pad tokens are zeroed before the conv.
//! `token_embd_norm` is the FINAL norm (`embedding_norm`, applied AFTER all
//! blocks - NOT after the token embedding). Pooling = CLS (first token = bos),
//! NO L2 normalize (the sentence-transformers `modules.json` has only Transformer
//! + Pooling, no Normalize module).
//!
//! ## Tensor layout
//! candle returns logical shapes (GGUF stores dims reversed on disk): 2D
//! linears `[out, in]`, `token_embd` `[vocab, hidden]`, conv weight `[H, k]`
//! (the `1` in PyTorch's `[H, 1, k]` is squeezed). Linears are transposed to
//! `[in, out]` at load so the forward does plain `x @ W` (same convention as
//! Gemma/Qwen/nomic). `token_embd` and the conv weight are used as-is.
//!
//! ## ⚠ Correctness caveat
//! Compiles clean. The forward matches the PyTorch reference but has not been
//! numerically validated here - validate cosine ~1.0 against the HF reference
//! once running.

use std::io::{Read, Seek};

use anyhow::{anyhow, Result};
use candle_core::quantized::gguf_file::{Content, Value};
use candle_core::{Device, Module, Tensor};
use candle_nn::{ops, RmsNorm};

use crate::rag::gguf_gemma::{apply_rope, attn_bias, l2_normalize, linear, GgufArch};

/// Attention block params (GQA + per-head q/k RMSNorm + RoPE).
struct AttnFields {
    q: Tensor,
    k: Tensor,
    v: Tensor,
    out: Tensor,
    q_norm: RmsNorm, // per-head RMSNorm over head_dim
    k_norm: RmsNorm,
}

/// ShortConv block params (in_proj + depthwise conv + out_proj).
struct ShortConvFields {
    in_proj: Tensor,  // [hidden, 3*hidden]
    out_proj: Tensor, // [hidden, hidden]
    conv: Tensor,     // [hidden, k]
}

/// One LFM2 block: pre-norm mixer (attention OR shortconv) + pre-norm SwiGLU FFN.
struct Lfm2Layer {
    is_attention: bool,
    attn_norm: RmsNorm, // operator_norm (before the mixer)
    ffn_norm: RmsNorm,
    // SwiGLU FFN: w2(silu(w1(x)) * w3(x)). w1=gate, w3=up, w2=down. [in, out].
    ffn_gate: Tensor,
    ffn_up: Tensor,
    ffn_down: Tensor,
    /// Present on attention blocks; `None` on shortconv blocks.
    attn: Option<AttnFields>,
    /// Present on shortconv blocks; `None` on attention blocks.
    shortconv: Option<ShortConvFields>,
}

/// LFM2.5 hybrid encoder for embeddings.
pub struct Lfm2Arch {
    token_embd: Tensor, // [vocab, hidden]
    final_norm: RmsNorm, // token_embd_norm = embedding_norm (applied AFTER blocks)
    layers: Vec<Lfm2Layer>,
    hidden: usize,
    n_head: usize,
    n_kv: usize,
    head_dim: usize,
    rope_base: f32,
    max_context: u32,
    device: Device,
}

impl Lfm2Arch {
    pub fn from_content<R: Read + Seek>(
        file: &mut R,
        content: &Content,
        device: &Device,
    ) -> Result<Self> {
        let md = &content.metadata;
        let pfx = "lfm2.";
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

        let hidden = get_u32(&format!("{}embedding_length", pfx), None)?; // 1024
        let block_count = get_u32(&format!("{}block_count", pfx), None)?; // 16
        let n_head = get_u32(&format!("{}attention.head_count", pfx), None)?; // 16
        // head_count_kv is a per-block array in LFM2 (8 for attention blocks,
        // 0 for shortconv) - take the max (the attention blocks' KV head count).
        let n_kv = match md.get(&format!("{}attention.head_count_kv", pfx)) {
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|v| match v {
                    Value::I32(x) => Some(*x as usize),
                    Value::U32(x) => Some(*x as usize),
                    _ => None,
                })
                .max()
                .unwrap_or(n_head),
            Some(v) => v.to_u32().ok().map(|x| x as usize).unwrap_or(n_head),
            None => n_head,
        }; // 8
        let head_dim = hidden / n_head; // 64
        let rope_base = get_f32(&format!("{}rope.freq_base", pfx), 1_000_000.0);
        let max_context = get_u32(&format!("{}context_length", pfx), Some(2048))? as u32;
        let rms_eps = get_f32(&format!("{}attention.layer_norm_rms_epsilon", pfx), 1e-5) as f64;

        // token_embd [vocab, hidden] (logical) - used directly for embedding.
        let token_embd = content.tensor(file, "token_embd.weight", device)?.dequantize(device)?;
        let final_norm = rms_norm(file, content, "token_embd_norm", device, rms_eps)?;

        let mut layers = Vec::with_capacity(block_count);
        for i in 0..block_count {
            // Detect block type by tensor presence (attention blocks ship
            // attn_q; shortconv blocks ship shortconv.in_proj).
            let is_attention = content
                .tensor_infos
                .contains_key(&format!("blk.{i}.attn_q.weight"));
            let attn_norm = rms_norm(file, content, &format!("blk.{i}.attn_norm"), device, rms_eps)?;
            let ffn_norm = rms_norm(file, content, &format!("blk.{i}.ffn_norm"), device, rms_eps)?;
            // SwiGLU FFN: linears [out, in] -> transpose to [in, out].
            let ffn_gate = lin(file, content, &format!("blk.{i}.ffn_gate.weight"), device)?;
            let ffn_up = lin(file, content, &format!("blk.{i}.ffn_up.weight"), device)?;
            let ffn_down = lin(file, content, &format!("blk.{i}.ffn_down.weight"), device)?;

            let (attn, shortconv) = if is_attention {
                (
                    Some(AttnFields {
                        q: lin(file, content, &format!("blk.{i}.attn_q.weight"), device)?,
                        k: lin(file, content, &format!("blk.{i}.attn_k.weight"), device)?,
                        v: lin(file, content, &format!("blk.{i}.attn_v.weight"), device)?,
                        out: lin(file, content, &format!("blk.{i}.attn_output.weight"), device)?,
                        q_norm: rms_norm(file, content, &format!("blk.{i}.attn_q_norm"), device, rms_eps)?,
                        k_norm: rms_norm(file, content, &format!("blk.{i}.attn_k_norm"), device, rms_eps)?,
                    }),
                    None,
                )
            } else {
                (
                    None,
                    Some(ShortConvFields {
                        in_proj: lin(file, content, &format!("blk.{i}.shortconv.in_proj.weight"), device)?,
                        out_proj: lin(file, content, &format!("blk.{i}.shortconv.out_proj.weight"), device)?,
                        // conv weight [H, k] (logical) - used as-is (no transpose).
                        conv: content.tensor(file, &format!("blk.{i}.shortconv.conv.weight"), device)?.dequantize(device)?,
                    }),
                )
            };
            layers.push(Lfm2Layer {
                is_attention,
                attn_norm,
                ffn_norm,
                ffn_gate,
                ffn_up,
                ffn_down,
                attn,
                shortconv,
            });
        }

        Ok(Self {
            token_embd,
            final_norm,
            layers,
            hidden,
            n_head,
            n_kv,
            head_dim,
            rope_base,
            max_context,
            device: device.clone(),
        })
    }

    /// Pre-norm block: `h = x + mixer(attn_norm(x)); out = h + ffn(ffn_norm(h))`.
    fn forward_layer(
        &self,
        l: &Lfm2Layer,
        x: &Tensor,
        positions: &Tensor,
        mask_bias: &Tensor,
        mask_2d: &Tensor,
    ) -> Result<Tensor> {
        let h = l.attn_norm.forward(x)?;
        let mixer_out = if l.is_attention {
            self.forward_attention(l.attn.as_ref().expect("attn block"), &h, positions, mask_bias)?
        } else {
            self.forward_shortconv(l.shortconv.as_ref().expect("shortconv block"), &h, mask_2d)?
        };
        let h = (&mixer_out + x)?;
        let ffn_out = self.forward_ffn(l, &l.ffn_norm.forward(&h)?)?;
        Ok((&ffn_out + &h)?)
    }

    /// GQA attention with per-head q/k RMSNorm + RoPE (bidirectional, pad-masked).
    fn forward_attention(
        &self,
        a: &AttnFields,
        h: &Tensor,
        positions: &Tensor,
        mask_bias: &Tensor,
    ) -> Result<Tensor> {
        let (b, seq, _h) = h.dims3()?;
        let q = linear(h, &a.q)?.reshape((b, seq, self.n_head, self.head_dim))?;
        let k = linear(h, &a.k)?.reshape((b, seq, self.n_kv, self.head_dim))?;
        let v = linear(h, &a.v)?.reshape((b, seq, self.n_kv, self.head_dim))?;
        // Per-head q/k RMSNorm (over head_dim) BEFORE RoPE.
        let q = a.q_norm.forward(&q)?;
        let k = a.k_norm.forward(&k)?;
        // Full RoPE over head_dim (identical to Gemma3's apply_rope).
        let q = apply_rope(&q, positions, self.head_dim, self.rope_base, &self.device)?;
        let k = apply_rope(&k, positions, self.head_dim, self.rope_base, &self.device)?;
        // GQA: repeat k, v from n_kv to n_head BEFORE the transpose (on
        // [b, seq, n_kv, head_dim], repeating dim 2 = n_kv). Repeating after the
        // transpose would multiply the seq dim instead.
        let rep = self.n_head / self.n_kv;
        let k = k.repeat((1, 1, rep, 1))?;
        let v = v.repeat((1, 1, rep, 1))?;
        let q = q.transpose(1, 2)?.contiguous()?;
        let k = k.transpose(1, 2)?.contiguous()?;
        let v = v.transpose(1, 2)?.contiguous()?;
        let scale = 1.0f64 / (self.head_dim as f64).sqrt();
        let qk = q.matmul(&k.t()?)?.affine(scale, 0.0)?;
        let qk = qk.broadcast_add(mask_bias)?; // bidirectional pad mask
        let probs = ops::softmax_last_dim(&qk)?;
        let attn = probs.matmul(&v.contiguous()?)?;
        let attn = attn.transpose(1, 2)?.reshape((b, seq, self.hidden))?;
        Ok(linear(&attn, &a.out)?)
    }

    /// Non-causal ShortConv: in_proj (H->3H) split into (B, C, x); Bx = B*x;
    /// depthwise conv1d (kernel 3, symmetric pad); y = C*conv; out_proj (H->H).
    /// Pad tokens are zeroed before the conv (matches `apply_mask_to_padding_states`).
    fn forward_shortconv(&self, s: &ShortConvFields, h: &Tensor, mask_2d: &Tensor) -> Result<Tensor> {
        let (b, seq, hidden) = h.dims3()?;
        // Zero pad tokens: h * mask[:,:,None].
        let mask_f = mask_2d.to_dtype(h.dtype())?.reshape((b, seq, 1))?;
        let h = h.broadcast_mul(&mask_f)?;
        // in_proj: [b, seq, 3H] -> [b, 3H, seq].
        let bcx = linear(&h, &s.in_proj)?.transpose(1, 2)?;
        let b_part = bcx.narrow(1, 0, hidden)?;
        let c_part = bcx.narrow(1, hidden, hidden)?;
        let x_part = bcx.narrow(1, 2 * hidden, hidden)?;
        let bx = b_part.broadcast_mul(&x_part)?; // B * x
        // Depthwise conv1d (non-causal, symmetric pad).
        let conv_out = depthwise_conv1d_noncausal(&bx, &s.conv)?;
        let y = c_part.broadcast_mul(&conv_out)?; // C * conv
        let y = y.transpose(1, 2)?.contiguous()?; // [b, seq, H]
        Ok(linear(&y, &s.out_proj)?)
    }

    /// SwiGLU FFN: `ffn_down(silu(ffn_gate(x)) * ffn_up(x))`.
    fn forward_ffn(&self, l: &Lfm2Layer, h: &Tensor) -> Result<Tensor> {
        let gate = linear(h, &l.ffn_gate)?.silu()?;
        let up = linear(h, &l.ffn_up)?;
        Ok(linear(&(&gate * &up)?, &l.ffn_down)?)
    }
}

impl GgufArch for Lfm2Arch {
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
        // NO embedding norm here - LFM2 applies embedding_norm (final_norm) at
        // the END, after all blocks.
        let mask_bias = attn_bias(attention_mask)?; // [b,1,1,seq] for attention
        let positions = Tensor::arange(0u32, seq as u32, &self.device)?;
        for layer in self.layers.iter() {
            x = self.forward_layer(layer, &x, &positions, &mask_bias, attention_mask)?;
        }
        // Final norm (embedding_norm = token_embd_norm).
        let x = self.final_norm.forward(&x)?;
        // CLS pooling: first token (bos). Then L2-normalize - the vectordb uses
        // L2 distance assuming normalized embeddings (L2 ranking == cosine), so
        // every arch must normalize even though LFM2's HF `modules.json` has no
        // Normalize module (un-normalized embeddings score near 0 under the
        // app's `1/(1+L2dist)` and get filtered out by any non-zero threshold).
        let pooled = x.narrow(1, 0, 1)?.squeeze(1)?;
        l2_normalize(&pooled)
    }
}

/// Depthwise conv1d, non-causal (symmetric zero padding), via shifts. `bx`
/// `[B, H, T]`, `conv_w` `[H, k]`. Output `[B, H, T]`. For k=3: out[t] =
/// w0*bx[t-1] + w1*bx[t] + w2*bx[t+1] (zero-padded at the boundaries).
fn depthwise_conv1d_noncausal(bx: &Tensor, conv_w: &Tensor) -> Result<Tensor> {
    let (b, h, t) = bx.dims3()?;
    let k = conv_w.dim(1)?;
    let pad = k / 2;
    let bx_pad = bx.pad_with_zeros(2, pad, pad)?; // [B, H, T + 2*pad]
    let mut out = Tensor::zeros((b, h, t), bx.dtype(), bx.device())?;
    for i in 0..k {
        let w_i = conv_w.narrow(1, i, 1)?.reshape((1, h, 1))?.broadcast_as((b, h, t))?;
        let shifted = bx_pad.narrow(2, i, t)?; // [B, H, T]
        out = out.broadcast_add(&shifted.broadcast_mul(&w_i)?)?;
    }
    Ok(out)
}

/// Load a 2D linear weight `[out, in]` -> transpose to `[in, out]` (dequantized).
fn lin<R: Read + Seek>(file: &mut R, ct: &Content, name: &str, device: &Device) -> Result<Tensor> {
    Ok(ct.tensor(file, name, device)?.dequantize(device)?.t()?.contiguous()?)
}

/// Load an RMSNorm (weight only) from `<prefix>.weight`.
fn rms_norm<R: Read + Seek>(
    file: &mut R,
    ct: &Content,
    prefix: &str,
    device: &Device,
    eps: f64,
) -> Result<RmsNorm> {
    let weight = ct.tensor(file, &format!("{prefix}.weight"), device)?.dequantize(device)?;
    Ok(RmsNorm::new(weight, eps))
}
