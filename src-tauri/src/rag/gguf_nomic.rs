//! GGUF architecture strategy for `nomic-bert-moe` (nomic-embed-text-v2-moe).
//!
//! ## Architecture (verified against `nomic-ai/nomic-bert-2048`'
//! `modeling_hf_nomic_bert.py` + the GGUF tensor layout)
//! 12-layer BERT encoder, **post-norm** LayerNorm (eps 1e-5), GELU activation,
//! full RoPE (`rotary_emb_fraction=1.0`, non-interleaved = NeoX rotate-half,
//! base 10000), `type_vocab_size=1` (single segment embedding), bidirectional
//! (no causal mask). MoE every 2 layers (blocks 1,3,5,7,9,11): 8 experts,
//! top-2 routing, **no** weight renormalization (`moe_normalize_expert_weights=
//! false`), no shared experts, no expert bias. Dense GELU FFN on the other
//! blocks. Pooling = mean (masked) + L2 normalize (sentence-transformers
//! `1_Pooling/config.json`). No final norm (each block ends with
//! `layer_output_norm`).
//!
//! ## Tensor layout
//! candle's `Content::tensor` returns the LOGICAL shape (GGUF stores dims
//! reversed on disk; candle un-reverses) - so 2D linears are `[out, in]` and
//! `token_embd` is `[vocab, hidden]`, the SAME convention as Gemma/Qwen. This
//! arch dequantizes each weight to f32 and transposes linears to `[in, out]`
//! at load so the forward does plain `x @ W`. `QMatMul` isn't used only because
//! the 3D MoE expert tensors can't go through it (it's 2D-only) - they're
//! dequantized + permuted to `[n_experts, in, out]` instead.
//!   token_embd.weight        [vocab, hidden]   (used directly for embedding)
//!   token_embd_norm.{w,b}    [hidden]          (embedding LayerNorm = emb_ln)
//!   token_types.weight       [hidden]          (type_vocab_size=1, one vector)
//!   blk.N.attn_qkv.{w,b}     [out=3*hidden, in=hidden] (combined QKV)
//!   blk.N.attn_output.{w,b}  [out=hidden, in=hidden]
//!   blk.N.attn_output_norm.{w,b}  [hidden]     (post-attention LayerNorm)
//!   blk.N.layer_output_norm.{w,b} [hidden]     (post-FFN LayerNorm)
//!   dense (even N): blk.N.ffn_up.{w,b} [out=ffn, in=hidden], ffn_down.{w,b} [out=hidden, in=ffn]
//!   MoE (odd N): blk.N.ffn_gate_inp.weight [out=8, in=hidden] (router, no bias),
//!                blk.N.ffn_up_exps.weight   [n_experts, out=ffn, in=hidden] -> permute [n_experts, in, ffn]
//!                blk.N.ffn_down_exps.weight [n_experts, out=hidden, in=ffn] -> permute [n_experts, ffn, in]
//!
//! ## ⚠ Correctness caveat
//! Compiles clean. The forward matches the PyTorch reference (post-norm block,
//! RoPE, top-2 MoE) but has not been numerically validated against the HF
//! reference embeddings here - validate cosine ~1.0 against
//! `nomic-ai/nomic-embed-text-v2-moe` once running.

use std::io::{Read, Seek};

use anyhow::{anyhow, Result};
use candle_core::quantized::gguf_file::Content;
use candle_core::{Device, Module, Tensor};
use candle_nn::{ops, LayerNorm};

use crate::rag::gguf_gemma::{apply_rope, attn_bias, linear, pool_and_normalize, GgufArch};

/// One transformer block: attention (combined QKV + RoPE) + post-norm, then an
/// FFN (dense GELU OR top-2 MoE) + post-norm.
struct NomicBertLayer {
    attn_qkv_w: Tensor,
    attn_qkv_b: Tensor,
    attn_out_w: Tensor,
    attn_out_b: Tensor,
    attn_norm: LayerNorm,
    ffn_norm: LayerNorm,
    ffn: Ffn,
}

/// The position-wise FFN: either a dense 2-layer GELU MLP, or a top-2 MoE over
/// 8 expert MLPs (no shared expert, no expert bias - the GGUF has none).
enum Ffn {
    Dense {
        up_w: Tensor,
        up_b: Tensor,
        down_w: Tensor,
        down_b: Tensor,
    },
    Moe {
        router_w: Tensor,
        /// [n_experts, hidden, ffn] (permuted from GGUF [hidden, ffn, n_experts]).
        exp_up_w: Tensor,
        /// [n_experts, ffn, hidden] (permuted from GGUF [ffn, hidden, n_experts]).
        exp_down_w: Tensor,
        n_experts: usize,
        top_k: usize,
    },
}

/// nomic-embed-text-v2-moe (`nomic-bert-moe`): a BERT-MoE encoder for
/// multilingual embeddings.
pub struct NomicBertMoeArch {
    /// [vocab, hidden] - candle returns this logical shape directly (no
    /// transpose); `Tensor::embedding` indexes dim 0 = vocab.
    token_embd: Tensor,
    /// [hidden] - single token-type embedding (type_vocab_size=1), added to
    /// every token's embedding.
    token_types: Tensor,
    emb_ln: LayerNorm,
    layers: Vec<NomicBertLayer>,
    hidden: usize,
    n_head: usize,
    head_dim: usize,
    rope_base: f32,
    max_context: u32,
    device: Device,
}

impl NomicBertMoeArch {
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

        let hidden = get_u32(&format!("{}embedding_length", pfx), None)?; // 768
        let block_count = get_u32(&format!("{}block_count", pfx), None)?; // 12
        let n_head = get_u32(&format!("{}attention.head_count", pfx), None)?; // 12
        let head_dim = hidden / n_head; // 64
        let moe_every = get_u32(&format!("{}moe_every_n_layers", pfx), Some(0))?; // 2
        let n_experts = get_u32(&format!("{}expert_count", pfx), Some(0))?; // 8
        let top_k = get_u32(&format!("{}expert_used_count", pfx), Some(2))?; // 2
        let rope_base = get_f32(&format!("{}rope.freq_base", pfx), 10_000.0);
        let max_context = get_u32(&format!("{}context_length", pfx), Some(512))? as u32;
        let ln_eps = get_f32(&format!("{}attention.layer_norm_epsilon", pfx), 1e-5) as f64;

        // token_embd: candle returns the LOGICAL shape [vocab, hidden] (GGUF
        // stores dims reversed on disk; candle un-reverses). Use directly for
        // `Tensor::embedding` (indexes dim 0 = vocab). NO transpose. dequantize
        // yields a contiguous tensor (index_select requires contiguous source).
        let token_embd = content
            .tensor(file, "token_embd.weight", device)?
            .dequantize(device)?;
        let token_types = content
            .tensor(file, "token_types.weight", device)?
            .dequantize(device)?; // [hidden]
        let emb_ln = layer_norm(file, content, "token_embd_norm", device, ln_eps)?;

        let mut layers = Vec::with_capacity(block_count);
        for i in 0..block_count {
            // moe_every_n_layers>0 + i%every==1 -> MoE block (matches the
            // PyTorch `moe=i%every_n==1`). For nomic v2: blocks 1,3,5,7,9,11.
            let is_moe = moe_every > 0 && i % moe_every == 1;
            let ffn = if is_moe {
                // candle returns logical [out, in] for linears (GGUF stores dims
                // reversed on disk; candle un-reverses) - SAME convention as
                // Gemma/Qwen. Transpose to [in, out] so the forward's `x @ W`
                // works. (nomic is NOT [in, out]; the on-disk reversed order
                // only looked that way in the raw GGUF dump.)
                let router_w = content
                    .tensor(file, &format!("blk.{i}.ffn_gate_inp.weight"), device)?
                    .dequantize(device)?
                    .t()?
                    .contiguous()?; // [hidden, n_experts]
                // candle [n_experts, out, in] -> [n_experts, in, out] so each
                // per-expert slice is [in, out] for `x @ W` in the MoE loop.
                let exp_up_w = content
                    .tensor(file, &format!("blk.{i}.ffn_up_exps.weight"), device)?
                    .dequantize(device)?
                    .permute([0usize, 2, 1])?
                    .contiguous()?;
                let exp_down_w = content
                    .tensor(file, &format!("blk.{i}.ffn_down_exps.weight"), device)?
                    .dequantize(device)?
                    .permute([0usize, 2, 1])?
                    .contiguous()?;
                Ffn::Moe { router_w, exp_up_w, exp_down_w, n_experts, top_k }
            } else {
                Ffn::Dense {
                    up_w: content.tensor(file, &format!("blk.{i}.ffn_up.weight"), device)?.dequantize(device)?.t()?.contiguous()?,
                    up_b: content.tensor(file, &format!("blk.{i}.ffn_up.bias"), device)?.dequantize(device)?,
                    down_w: content.tensor(file, &format!("blk.{i}.ffn_down.weight"), device)?.dequantize(device)?.t()?.contiguous()?,
                    down_b: content.tensor(file, &format!("blk.{i}.ffn_down.bias"), device)?.dequantize(device)?,
                }
            };
            layers.push(NomicBertLayer {
                attn_qkv_w: content.tensor(file, &format!("blk.{i}.attn_qkv.weight"), device)?.dequantize(device)?.t()?.contiguous()?,
                attn_qkv_b: content.tensor(file, &format!("blk.{i}.attn_qkv.bias"), device)?.dequantize(device)?,
                attn_out_w: content.tensor(file, &format!("blk.{i}.attn_output.weight"), device)?.dequantize(device)?.t()?.contiguous()?,
                attn_out_b: content.tensor(file, &format!("blk.{i}.attn_output.bias"), device)?.dequantize(device)?,
                attn_norm: layer_norm(file, content, &format!("blk.{i}.attn_output_norm"), device, ln_eps)?,
                ffn_norm: layer_norm(file, content, &format!("blk.{i}.layer_output_norm"), device, ln_eps)?,
                ffn,
            });
        }

        Ok(Self {
            token_embd,
            token_types,
            emb_ln,
            layers,
            hidden,
            n_head,
            head_dim,
            rope_base,
            max_context,
            device: device.clone(),
        })
    }

    /// One post-norm block:
    ///   h = attn_output_norm(attn(x) + x)
    ///   out = layer_output_norm(ffn(h) + h)
    fn forward_layer(
        &self,
        l: &NomicBertLayer,
        x: &Tensor,
        positions: &Tensor,
        mask_bias: &Tensor,
    ) -> Result<Tensor> {
        let (b, seq, _h) = x.dims3()?;
        // --- Self-attention (combined QKV, no GQA, no q/k norm) ---
        let qkv = linear(x, &l.attn_qkv_w)?.broadcast_add(&l.attn_qkv_b)?; // [b, seq, 3*hidden]
        let q = qkv.narrow(2, 0, self.hidden)?;
        let k = qkv.narrow(2, self.hidden, self.hidden)?;
        let v = qkv.narrow(2, 2 * self.hidden, self.hidden)?;
        let q = q.reshape((b, seq, self.n_head, self.head_dim))?;
        let k = k.reshape((b, seq, self.n_head, self.head_dim))?;
        let v = v.reshape((b, seq, self.n_head, self.head_dim))?;
        // Full RoPE over head_dim (rotary_emb_fraction=1.0), q + k only.
        let q = apply_rope(&q, positions, self.head_dim, self.rope_base, &self.device)?;
        let k = apply_rope(&k, positions, self.head_dim, self.rope_base, &self.device)?;
        let q = q.transpose(1, 2)?.contiguous()?; // [b, n_head, seq, head_dim]
        let k = k.transpose(1, 2)?.contiguous()?;
        let v = v.transpose(1, 2)?.contiguous()?;
        let scale = 1.0f64 / (self.head_dim as f64).sqrt();
        let qk = q.matmul(&k.t()?)?.affine(scale, 0.0)?; // [b, n_head, seq, seq]
        let qk = qk.broadcast_add(mask_bias)?; // pad masking (bidirectional)
        let probs = ops::softmax_last_dim(&qk)?;
        let attn = probs.matmul(&v.contiguous()?)?; // [b, n_head, seq, head_dim]
        let attn = attn.transpose(1, 2)?.reshape((b, seq, self.hidden))?;
        let attn = linear(&attn, &l.attn_out_w)?.broadcast_add(&l.attn_out_b)?;
        // post-attention norm
        let h = l.attn_norm.forward(&(&attn + x)?)?;

        // --- FFN (dense or MoE) ---
        let ffn_out = match &l.ffn {
            Ffn::Dense { up_w, up_b, down_w, down_b } => {
                let u = linear(&h, up_w)?.broadcast_add(up_b)?.gelu()?;
                linear(&u, down_w)?.broadcast_add(down_b)?
            }
            Ffn::Moe { router_w, exp_up_w, exp_down_w, n_experts, top_k } => {
                self.forward_moe(&h, router_w, exp_up_w, exp_down_w, *n_experts, *top_k)?
            }
        };
        // post-FFN norm
        Ok(l.ffn_norm.forward(&(&ffn_out + &h)?)?)
    }

    /// Top-2 MoE FFN. Router logits -> fp32 softmax -> top-2 mask (no
    /// renormalization) -> all `n_experts` expert MLPs evaluated batched,
    /// weighted by the masked softmax, summed. Computing all experts (vs only
    /// the routed 2) is 4x the FFN compute for nomic's 8/2 but is simple and
    /// correct (the non-top-2 weights are zeroed by the mask). The router mask
    /// is computed on CPU (the logits are [N, 8], tiny) to avoid a `topk`
    /// dependency and to match the fp32 softmax exactly.
    fn forward_moe(
        &self,
        h: &Tensor,
        router_w: &Tensor,
        exp_up_w: &Tensor,
        exp_down_w: &Tensor,
        n_experts: usize,
        top_k: usize,
    ) -> Result<Tensor> {
        let (b, seq, hidden) = h.dims3()?;
        let n = b * seq;
        let x_flat = h.reshape((n, hidden))?;

        // Router: logits [N, E] -> top-k masked softmax weights (CPU). The
        // logits are [N, E] (tiny), and computing the top-k mask on CPU avoids a
        // `topk` dependency and matches the fp32 softmax exactly.
        let logits = x_flat.matmul(router_w)?; // [N, E]
        let logits_cpu = logits.flatten_all()?.to_vec1::<f32>()?; // [N*E]
        let mut eff = vec![0.0f32; n * n_experts];
        for i in 0..n {
            let row = &logits_cpu[i * n_experts..i * n_experts + n_experts];
            let max = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let mut exps = vec![0.0f32; n_experts];
            let mut sum = 0.0f32;
            for e in 0..n_experts {
                exps[e] = (row[e] - max).exp();
                sum += exps[e];
            }
            // top-k expert indices by logit (descending). Ties: lower index
            // first (stable sort by value then index).
            let mut idx: Vec<usize> = (0..n_experts).collect();
            idx.sort_by(|&a, &bb| {
                row[bb]
                    .partial_cmp(&row[a])
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            // moe_normalize_expert_weights=false: keep the RAW softmax weight
            // for the top-k experts (NOT renormalized to sum to 1).
            for k in 0..top_k {
                let e = idx[k];
                eff[i * n_experts + e] = exps[e] / sum;
            }
        }
        let eff = Tensor::from_vec(eff, (n, n_experts), &self.device)?; // [N, E]

        // Top-k experts, summed. Loop the experts (each a plain 2D matmul - the
        // well-tested path) rather than a batched matmul over a broadcast batch
        // dim, which candle backends handle inconsistently. Non-routed experts
        // contribute 0 (eff is masked), so iterating all is correct; only the
        // top-k carry nonzero weight.
        let mut out = Tensor::zeros((n, hidden), x_flat.dtype(), &self.device)?;
        for e in 0..n_experts {
            let w1 = exp_up_w.narrow(0, e, 1)?.squeeze(0)?; // [hidden, ffn]
            let w2 = exp_down_w.narrow(0, e, 1)?.squeeze(0)?; // [ffn, hidden]
            let down_e = x_flat.matmul(&w1)?.gelu()?.matmul(&w2)?; // [N, hidden]
            let eff_e = eff.narrow(1, e, 1)?; // [N, 1]
            out = out.broadcast_add(&down_e.broadcast_mul(&eff_e)?)?;
        }
        Ok(out.reshape((b, seq, hidden))?)
    }
}

impl GgufArch for NomicBertMoeArch {
    fn hidden_dim(&self) -> usize {
        self.hidden
    }

    fn max_context(&self) -> u32 {
        self.max_context
    }

    fn forward_embed(&self, input_ids: &Tensor, attention_mask: &Tensor) -> Result<Tensor> {
        let (b, seq) = input_ids.dims2()?;
        let ids_flat = input_ids.flatten_all()?;
        // Token embedding ([vocab, hidden] indexes dim 0) + single token-type
        // embedding (broadcast over the sequence).
        let mut x = self.token_embd.embedding(&ids_flat)?; // [b*seq, hidden]
        x = x.reshape((b, seq, self.hidden))?;
        x = x.broadcast_add(&self.token_types.reshape((1, 1, self.hidden))?)?;
        // Embedding LayerNorm (emb_ln). No positional embedding (RoPE handles
        // positions; max_position_embeddings=0 when rotary_emb_fraction>0).
        x = self.emb_ln.forward(&x)?;

        let mask_bias = attn_bias(attention_mask)?; // [b,1,1,seq] 0 / -1e9
        let positions = Tensor::arange(0u32, seq as u32, &self.device)?;
        for layer in self.layers.iter() {
            x = self.forward_layer(layer, &x, &positions, &mask_bias)?;
        }
        // No final norm (each block ends with layer_output_norm). Mean-pool
        // (masked) + L2 normalize.
        pool_and_normalize(&x, attention_mask)
    }
}

/// Load a LayerNorm (weight + bias) from `<prefix>.weight` / `<prefix>.bias`.
fn layer_norm<R: Read + Seek>(
    file: &mut R,
    ct: &Content,
    prefix: &str,
    device: &Device,
    eps: f64,
) -> Result<LayerNorm> {
    let weight = ct.tensor(file, &format!("{prefix}.weight"), device)?.dequantize(device)?;
    let bias = ct.tensor(file, &format!("{prefix}.bias"), device)?.dequantize(device)?;
    Ok(LayerNorm::new(weight, bias, eps))
}
