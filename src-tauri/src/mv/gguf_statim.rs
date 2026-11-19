//! GGUF architecture strategy for `laya`-arch decision models
//! (statim-decide-*, fine-tunes of convaiinnovations/laya-multilingual).
//!
//! Unlike the ggmlc graph models (julia/laya ship a precompiled graph), these
//! GGUFs carry plain encoder/head tensors that this module runs by hand —
//! semantics transcribed from the official C++ engine
//! (github.com/BEKO2210/statim, src/model.cpp + src/engine.cpp):
//!
//! - Encoder: mmBERT-base (768d, 22 layers, 12 heads, GeGLU FFN 1152),
//!   pre-LN, RoPE (NeoX half-split, theta 160000), alternating local
//!   (window 128 → ±64) / global bidirectional attention
//!   (`laya.encoder.layer_is_global`).
//! - Sequence: identical contract to laya —
//!   `[CLS] <type> question: ins [SEP] (MASK opt)* [SEP] state [SEP]`,
//!   options capped at 48 tokens, head budget = head_max_len(512) - options,
//!   min 8, per-option cap when the budget drops under 16.
//! - Head: type embedding added over the whole sequence, then 2 pre-norm
//!   transformer layers (LayerNorm WITH bias, eps 1e-5, ReLU FFN, biased
//!   linears). The LAST layer is "pruned": only [CLS] + the [MASK] marker
//!   rows are carried as queries (keys/values still span the full sequence).
//! - Scorer: marker rows → LayerNorm → Linear → gelu(erf) → Linear → 1 logit
//!   per option.
//! - Decoding: softmax(logits / T) with T from
//!   `laya.temperature_by_options` (`"<type>:<2|3-5|6-10|11+>"` buckets),
//!   falling back to `laya.temperature[qt]` (clamped to [0.5, 5]).
//!
//! The tokenizer ships INSIDE the GGUF (`tokenizer.statim.*`: Metaspace BPE +
//! Replace normalizer, 256000 vocab) — no auxiliary tokenizer.json needed.

use crate::mv::decision::{DecisionOutput, DecisionRequest, DecisionType};
use crate::mv::gguf_gemma::{apply_rope, linear};
use anyhow::{anyhow, Result};
use candle_core::quantized::gguf_file::{Content, Value};
use candle_core::{DType, Device, Tensor};
use candle_nn::Module;
use std::collections::HashMap;
use std::io::{Read, Seek};
use tokenizers::Tokenizer;

/// One encoder layer's weights (dequantized; linears stored `[in, out]`).
struct EncLayer {
    attn_norm: Option<Tensor>,
    wqkv: Tensor,   // [in, 3*out]
    wo: Tensor,     // [in, out]
    mlp_norm: Tensor,
    wi: Tensor,     // [in, 2*ff] (first half = input, second half = gate)
    wo_mlp: Tensor, // [ff, in]
}

/// One decision-head transformer layer (biased norms + biased linears).
struct HeadLayer {
    norm1_w: Tensor,
    norm1_b: Tensor,
    in_w: Tensor,  // [in, 3*d]
    in_b: Tensor,
    out_w: Tensor, // [in, d]
    out_b: Tensor,
    norm2_w: Tensor,
    norm2_b: Tensor,
    l1_w: Tensor, // [in, head_ff]
    l1_b: Tensor,
    l2_w: Tensor, // [head_ff, in]
    l2_b: Tensor,
}

pub struct StatimModel {
    pub tokenizer: Tokenizer,
    device: Device,
    // hparams
    n_head: usize,
    n_embd: usize,
    n_ff: usize,
    head_n_head: usize,
    head_ff: usize,
    local_window: usize, // full window; the mask uses ±window/2
    rope_theta: f32,
    norm_eps: f64,
    layer_is_global: Vec<bool>,
    max_len: usize,
    head_max_len: usize,
    cls_id: u32,
    sep_id: u32,
    mask_id: u32,
    // encoder
    tok_embd: Tensor,
    embd_norm: Tensor,
    layers: Vec<EncLayer>,
    final_norm: Tensor,
    // head
    type_emb: Tensor, // [3, d]
    head: Vec<HeadLayer>,
    // scorer
    sc_norm_w: Tensor,
    sc_norm_b: Tensor,
    sc1_w: Tensor,
    sc1_b: Tensor,
    sc2_w: Tensor,
    sc2_b: Tensor,
    // decoding temperatures
    temperature: Vec<f32>, // per qtype
    temperature_by_options: HashMap<String, f32>,
}

/// GGUF linear `[out, in]` → dequantized transposed `[in, out]`.
fn lin<'a, R: Read + Seek>(file: &mut R, ct: &'a Content, name: &str, device: &Device) -> Result<Tensor> {
    Ok(ct
        .tensor(file, name, device)?
        .dequantize(device)?
        .t()?
        .contiguous()?)
}

/// GGUF f32 vector `[n]` → f32 tensor.
fn vec_t<'a, R: Read + Seek>(file: &mut R, ct: &'a Content, name: &str, device: &Device) -> Result<Tensor> {
    Ok(ct.tensor(file, name, device)?.dequantize(device)?)
}

fn arr_strings(md: &HashMap<String, Value>, key: &str) -> Option<Vec<String>> {
    match md.get(key) {
        Some(Value::Array(a)) => Some(
            a.iter()
                .filter_map(|x| match x {
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                })
                .collect(),
        ),
        _ => None,
    }
}

fn meta_u32(md: &HashMap<String, Value>, key: &str) -> Option<u32> {
    match md.get(key)? {
        Value::U32(v) => Some(*v),
        _ => None,
    }
}

fn meta_f32(md: &HashMap<String, Value>, key: &str) -> Option<f32> {
    match md.get(key)? {
        Value::F32(v) => Some(*v),
        _ => None,
    }
}

impl StatimModel {
    pub fn load<R: Read + Seek>(file: &mut R, device: &Device) -> Result<Self> {
        let ct = Content::read(file).map_err(|e| anyhow!("read gguf: {e}"))?;
        let md = &ct.metadata;
        let get_u32 = |k: &str| -> Result<usize> {
            meta_u32(md, k)
                .map(|v| v as usize)
                .ok_or_else(|| anyhow!("missing {k}"))
        };
        let get_f32 = |k: &str| -> Result<f32> {
            meta_f32(md, k).ok_or_else(|| anyhow!("missing {k}"))
        };
        let n_embd = get_u32("laya.encoder.n_embd")?;
        let n_layer = get_u32("laya.encoder.n_layer")?;
        let n_head = get_u32("laya.encoder.n_head")?;
        let n_ff = get_u32("laya.encoder.n_ff")?;
        let local_window = get_u32("laya.encoder.local_window")?;
        let rope_theta = get_f32("laya.encoder.rope_theta_global")?;
        let norm_eps = get_f32("laya.encoder.norm_eps")? as f64;
        let max_len = get_u32("laya.max_len")?;
        let head_max_len = get_u32("laya.head_max_len")?;
        let head_n_head = get_u32("laya.head.n_head")?;
        let head_ff = get_u32("laya.head.n_ff")?;
        let layer_is_global: Vec<bool> = match md.get("laya.encoder.layer_is_global") {
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(|v| match v {
                    Value::Bool(x) => Some(*x),
                    _ => None,
                })
                .collect(),
            _ => return Err(anyhow!("missing laya.encoder.layer_is_global")),
        };
        if layer_is_global.len() != n_layer {
            return Err(anyhow!("layer_is_global length mismatch"));
        }

        let temperature: Vec<f32> = match md.get("laya.temperature") {
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(|x| match x {
                    Value::F32(v) => Some(*v),
                    _ => None,
                })
                .collect(),
            _ => vec![1.0, 1.0, 1.0],
        };
        let temperature_by_options: HashMap<String, f32> = md
            .get("laya.temperature_by_options")
            .and_then(|v| match v {
                Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .and_then(|s| serde_json::from_str::<HashMap<String, f64>>(&s).ok())
            .map(|m| m.into_iter().map(|(k, v)| (k, v as f32)).collect())
            .unwrap_or_default();

        // Tokenizer from the embedded `tokenizer.statim.*` config: BPE vocab +
        // merges + Replace(' '→'▁') normalizer + Metaspace pre-tokenizer +
        // byte fallback — exactly the config the official engine applies.
        let tokens = arr_strings(md, "tokenizer.statim.tokens")
            .ok_or_else(|| anyhow!("missing tokenizer.statim.tokens"))?;
        let merges_raw = arr_strings(md, "tokenizer.statim.merges").unwrap_or_default();
        let tokenizer = crate::mv::gguf::build_metaspace_bpe_from(&tokens, &merges_raw)?;

        let cls_id = meta_u32(md, "tokenizer.statim.cls_id").unwrap_or(2);
        let sep_id = meta_u32(md, "tokenizer.statim.sep_id").unwrap_or(1);
        let mask_id = meta_u32(md, "tokenizer.statim.mask_id").unwrap_or(4);

        // ---- weights
        let tok_embd = ct
            .tensor(file, "encoder.embeddings.tok_embeddings.weight", device)?
            .dequantize(device)?;
        let embd_norm = vec_t(file, &ct, "encoder.embeddings.norm.weight", device)?;
        let mut layers = Vec::with_capacity(n_layer);
        for l in 0..n_layer {
            let p = format!("encoder.layers.{l}");
            // Layer 0 ships no attn_norm (the embedding LN already ran).
            let attn_norm = if ct.tensor_infos.contains_key(&format!("{p}.attn_norm.weight")) {
                Some(vec_t(file, &ct, &format!("{p}.attn_norm.weight"), device)?)
            } else {
                None
            };
            layers.push(EncLayer {
                attn_norm,
                wqkv: lin(file, &ct, &format!("{p}.attn.Wqkv.weight"), device)?,
                wo: lin(file, &ct, &format!("{p}.attn.Wo.weight"), device)?,
                mlp_norm: vec_t(file, &ct, &format!("{p}.mlp_norm.weight"), device)?,
                wi: lin(file, &ct, &format!("{p}.mlp.Wi.weight"), device)?,
                wo_mlp: lin(file, &ct, &format!("{p}.mlp.Wo.weight"), device)?,
            });
        }
        let final_norm = vec_t(file, &ct, "encoder.final_norm.weight", device)?;
        let type_emb = vec_t(file, &ct, "type_emb.weight", device)?;
        let n_head_layers = get_u32("laya.head.n_layer")?;
        let mut head = Vec::with_capacity(n_head_layers);
        for l in 0..n_head_layers {
            let p = format!("head.layers.{l}");
            head.push(HeadLayer {
                norm1_w: vec_t(file, &ct, &format!("{p}.norm1.weight"), device)?,
                norm1_b: vec_t(file, &ct, &format!("{p}.norm1.bias"), device)?,
                in_w: lin(file, &ct, &format!("{p}.self_attn.in_proj_weight"), device)?,
                in_b: vec_t(file, &ct, &format!("{p}.self_attn.in_proj_bias"), device)?,
                out_w: lin(file, &ct, &format!("{p}.self_attn.out_proj.weight"), device)?,
                out_b: vec_t(file, &ct, &format!("{p}.self_attn.out_proj.bias"), device)?,
                norm2_w: vec_t(file, &ct, &format!("{p}.norm2.weight"), device)?,
                norm2_b: vec_t(file, &ct, &format!("{p}.norm2.bias"), device)?,
                l1_w: lin(file, &ct, &format!("{p}.linear1.weight"), device)?,
                l1_b: vec_t(file, &ct, &format!("{p}.linear1.bias"), device)?,
                l2_w: lin(file, &ct, &format!("{p}.linear2.weight"), device)?,
                l2_b: vec_t(file, &ct, &format!("{p}.linear2.bias"), device)?,
            });
        }
        let sc_norm_w = vec_t(file, &ct, "scorer.0.weight", device)?;
        let sc_norm_b = vec_t(file, &ct, "scorer.0.bias", device)?;
        let sc1_w = lin(file, &ct, "scorer.1.weight", device)?;
        let sc1_b = vec_t(file, &ct, "scorer.1.bias", device)?;
        let sc2_w = lin(file, &ct, "scorer.3.weight", device)?;
        let sc2_b = vec_t(file, &ct, "scorer.3.bias", device)?;

        Ok(Self {
            tokenizer,
            device: device.clone(),
            n_head,
            n_embd,
            n_ff,
            head_n_head,
            head_ff,
            local_window,
            rope_theta,
            norm_eps,
            layer_is_global,
            max_len,
            head_max_len,
            cls_id,
            sep_id,
            mask_id,
            tok_embd,
            embd_norm,
            layers,
            final_norm,
            type_emb,
            head,
            sc_norm_w,
            sc_norm_b,
            sc1_w,
            sc1_b,
            sc2_w,
            sc2_b,
            temperature,
            temperature_by_options,
        })
    }

    /// Packed sequence for one question — the official statim `build_item`
    /// (identical to the laya contract this codebase already implements for
    /// julia): `[CLS] <type> question: ins [SEP] (MASK opt)* [SEP] state [SEP]`.
    fn sequence(&self, req: &DecisionRequest) -> Result<(Vec<u32>, Vec<usize>)> {
        if req.options.len() < 2 || req.options.len() > 20 {
            return Err(anyhow!(
                "options must contain 2-20 entries (got {})",
                req.options.len()
            ));
        }
        if req.options.iter().any(|o| o.is_empty()) {
            return Err(anyhow!("options must be non-empty"));
        }
        const MAX_FIELD_CHARS: usize = 64 * 1024;
        const MAX_TOTAL_CHARS: usize = 128 * 1024;
        if req.question.chars().count() > MAX_FIELD_CHARS
            || req.state.chars().count() > MAX_FIELD_CHARS
            || req.options.iter().any(|o| o.chars().count() > MAX_FIELD_CHARS)
        {
            return Err(anyhow!(
                "question/state/options exceed the {MAX_FIELD_CHARS}-character limit"
            ));
        }
        let total_chars: usize = req.question.chars().count()
            + req.state.chars().count()
            + req.options.iter().map(|o| o.chars().count()).sum::<usize>();
        if total_chars > MAX_TOTAL_CHARS {
            return Err(anyhow!(
                "question+state+options exceed the {MAX_TOTAL_CHARS}-character total limit"
            ));
        }
        if req.decision_type == DecisionType::Noul
            && req.options != vec!["false".to_string(), "true".to_string()]
        {
            return Err(anyhow!(
                "noul decisions require exactly the option pair [\"false\", \"true\"]"
            ));
        }
        let enc = |t: &str| -> Result<Vec<u32>> {
            Ok(self
                .tokenizer
                .encode(t, false)
                .map_err(|e| anyhow!("tokenize: {e}"))?
                .get_ids()
                .to_vec())
        };
        // Official `render_options`: score questions get a `level N: ` prefix;
        // noul renders the fixed semantic pair; choice passes through. Mask
        // token characters in user text are replaced by spaces.
        let clean = |t: &str| t.replace("<mask>", " ");
        let rendered: Vec<String> = match req.decision_type {
            DecisionType::Score => req
                .options
                .iter()
                .enumerate()
                .map(|(i, o)| format!("level {i}: {}", clean(o)))
                .collect(),
            DecisionType::Noul => vec![
                "false: no, the statement does not hold".to_string(),
                "true: yes, the statement holds".to_string(),
            ],
            DecisionType::Choice => req.options.iter().map(|o| clean(o)).collect(),
        };

        let head_ids = enc(&format!(
            "{} question: {}",
            req.decision_type.as_str(),
            clean(&req.question)
        ))?;
        let mut options: Vec<Vec<u32>> = rendered
            .iter()
            .map(|o| -> Result<Vec<u32>> {
                let mut v = vec![self.mask_id];
                v.extend(enc(&format!(" {o}"))?.into_iter().take(48));
                Ok(v)
            })
            .collect::<Result<Vec<_>>>()?;
        let mut budget = self
            .head_max_len
            .saturating_sub(options.iter().map(|o| o.len()).sum::<usize>());
        if budget < 16 {
            let per_option =
                std::cmp::max(4, self.head_max_len.saturating_sub(16) / options.len());
            for o in options.iter_mut() {
                o.truncate(per_option);
            }
            budget = self
                .head_max_len
                .saturating_sub(options.iter().map(|o| o.len()).sum::<usize>());
        }
        let mut ids = vec![self.cls_id];
        ids.extend(head_ids.into_iter().take(std::cmp::max(8, budget)));
        ids.push(self.sep_id);
        let mut markers = Vec::with_capacity(options.len());
        for o in &options {
            markers.push(ids.len());
            ids.extend_from_slice(o);
        }
        ids.push(self.sep_id);
        let room = self.max_len.saturating_sub(ids.len() + 1);
        let state_ids = enc(&req.state)?;
        let n = state_ids.len().min(room);
        ids.extend_from_slice(&state_ids[..n]);
        ids.push(self.sep_id);
        ids.truncate(self.max_len);
        markers.retain(|&m| m < ids.len());
        if markers.len() != req.options.len() {
            return Err(anyhow!(
                "options exceed head_max_len={}",
                self.head_max_len
            ));
        }
        Ok((ids, markers))
    }

    /// Multi-head bidirectional attention with an additive bias mask.
    /// q/k/v: [1, L, H, hd]; mask: [1, 1, Q, L] additive (0 / -1e9).
    fn attention(
        &self,
        q: &Tensor,
        k: &Tensor,
        v: &Tensor,
        mask: &Tensor,
        scale: f64,
    ) -> Result<Tensor> {
        let (_, l, h, hd) = q.dims4()?;
        let qp = q.transpose(1, 2)?.contiguous()?; // [1, H, L, hd]
        let kp = k.transpose(1, 2)?.contiguous()?;
        let vp = v.transpose(1, 2)?.contiguous()?;
        let scores = (qp.matmul(&kp.t()?)? * scale)?;
        let scores = scores.broadcast_add(mask)?;
        let probs = candle_nn::ops::softmax(&scores, candle_core::D::Minus1)?;
        let out = probs.matmul(&vp.contiguous()?)?; // [1, H, L, hd]
        Ok(out.transpose(1, 2)?.reshape((1, l, h * hd))?)
    }

    pub fn decide(&self, req: &DecisionRequest) -> Result<DecisionOutput> {
        let (ids, markers) = self.sequence(req)?;
        let l = ids.len();
        let device = &self.device;
        let ids_t = Tensor::new(ids.as_slice(), device)?.to_dtype(DType::U32)?;
        let positions = Tensor::arange(0u32, l as u32, device)?;

        // ---- encoder
        let mut x = self.tok_embd.embedding(&ids_t)?.unsqueeze(0)?; // [1, L, d]
        x = layer_norm(&x, &self.embd_norm, None, self.norm_eps)?;
        let half = self.local_window / 2;
        let global_mask = Tensor::zeros((1, 1, l, l), DType::F32, device)?;
        let local_mask = local_window_mask(l, half, device)?;
        let hd = self.n_embd / self.n_head;
        let scale = 1.0 / (hd as f64).sqrt();
        for (li, ly) in self.layers.iter().enumerate() {
            let a = match &ly.attn_norm {
                Some(w) => layer_norm(&x, w, None, self.norm_eps)?,
                None => x.clone(),
            };
            let qkv = linear(&a, &ly.wqkv)?; // [1, L, 3d]
            let q = qkv
                .narrow(2, 0, self.n_embd)?
                .reshape((1, l, self.n_head, hd))?;
            let k = qkv
                .narrow(2, self.n_embd, self.n_embd)?
                .reshape((1, l, self.n_head, hd))?;
            let v = qkv
                .narrow(2, 2 * self.n_embd, self.n_embd)?
                .reshape((1, l, self.n_head, hd))?;
            let q = apply_rope(&q, &positions, hd, self.rope_theta, device)?;
            let k = apply_rope(&k, &positions, hd, self.rope_theta, device)?;
            let mask = if self.layer_is_global[li] {
                &global_mask
            } else {
                &local_mask
            };
            let o = self.attention(&q, &k, &v, mask, scale)?;
            x = (x + linear(&o, &ly.wo)?)?;
            let m = layer_norm(&x, &ly.mlp_norm, None, self.norm_eps)?;
            let wi = linear(&m, &ly.wi)?; // [1, L, 2*ff]
            let inp = wi.narrow(2, 0, self.n_ff)?;
            let gate = wi.narrow(2, self.n_ff, self.n_ff)?;
            // GeGLU (erf gelu on the input half, gated) — official geglu_rows:
            // dst = gelu_erf(src[i]) * src[ff + i].
            let m = (gelu(&inp)? * gate)?;
            x = (x + linear(&m, &ly.wo_mlp)?)?;
        }
        x = layer_norm(&x, &self.final_norm, None, self.norm_eps)?;

        // ---- head: type embedding over the whole sequence
        let te = self
            .type_emb
            .narrow(0, req.decision_type.id() as usize, 1)?; // [1, d]
        let mut xh = x.broadcast_add(&te.reshape((1, 1, self.n_embd))?)?;

        let n_opt = markers.len();
        let qn = 1 + n_opt; // [CLS] + marker rows
        let hhd = self.n_embd / self.head_n_head;
        let hscale = 1.0 / (hhd as f64).sqrt();
        let n_head_layers = self.head.len();
        for (li, ly) in self.head.iter().enumerate() {
            let a = layer_norm(&xh, &ly.norm1_w, Some(&ly.norm1_b), 1e-5)?;
            let last = li + 1 == n_head_layers;
            if !last {
                let qkv = linear(&a, &ly.in_w)?
                    .broadcast_add(&ly.in_b.reshape((1, 1, 3 * self.n_embd))?)?;
                let q = qkv
                    .narrow(2, 0, self.n_embd)?
                    .reshape((1, l, self.head_n_head, hhd))?;
                let k = qkv
                    .narrow(2, self.n_embd, self.n_embd)?
                    .reshape((1, l, self.head_n_head, hhd))?;
                let v = qkv
                    .narrow(2, 2 * self.n_embd, self.n_embd)?
                    .reshape((1, l, self.head_n_head, hhd))?;
                // Head layers use the GLOBAL mask (official io.mask_global).
                let o = self.attention(&q, &k, &v, &global_mask, hscale)?;
                xh = (xh
                    + linear(&o, &ly.out_w)?
                        .broadcast_add(&ly.out_b.reshape((1, 1, self.n_embd))?)?)?;
                let f = layer_norm(&xh, &ly.norm2_w, Some(&ly.norm2_b), 1e-5)?;
                let f = linear(&f, &ly.l1_w)?
                    .broadcast_add(&ly.l1_b.reshape((1, 1, self.head_ff))?)?;
                let f = f.relu()?;
                xh = (xh
                    + linear(&f, &ly.l2_w)?
                        .broadcast_add(&ly.l2_b.reshape((1, 1, self.n_embd))?)?)?;
            } else {
                // Pruned layer: queries = [CLS] + marker rows only; keys and
                // values still span the full sequence. mask_q makes every real
                // token visible (no padding in a single packed sequence).
                let qkv = linear(&a, &ly.in_w)?
                    .broadcast_add(&ly.in_b.reshape((1, 1, 3 * self.n_embd))?)?;
                let k = qkv
                    .narrow(2, self.n_embd, self.n_embd)?
                    .reshape((1, l, self.head_n_head, hhd))?;
                let v = qkv
                    .narrow(2, 2 * self.n_embd, self.n_embd)?
                    .reshape((1, l, self.head_n_head, hhd))?;
                let mut qrows: Vec<u32> = vec![0];
                qrows.extend(markers.iter().map(|&m| m as u32));
                let qrows_t = Tensor::new(qrows.as_slice(), device)?.to_dtype(DType::U32)?;
                let a_flat = a.reshape((l, self.n_embd))?;
                let aq = a_flat.index_select(&qrows_t, 0)?; // [Qn, d]
                // in_w is [in, out] after lin(); the Q slice is the first d
                // output columns (official: first d rows of the [out, in] GGUF).
                let wq = ly.in_w.narrow(1, 0, self.n_embd)?.contiguous()?;
                let bq = ly.in_b.narrow(0, 0, self.n_embd)?;
                let q = linear(&aq, &wq)?.broadcast_add(&bq.reshape((1, self.n_embd))?)?; // [Qn, d]
                let q = q
                    .reshape((qn, self.head_n_head, hhd))?
                    .transpose(0, 1)?
                    .unsqueeze(0)?
                    .contiguous()?; // [1, H, Qn, hd]
                let kp = k.transpose(1, 2)?.contiguous()?; // [1, H, L, hd]
                let vp = v.transpose(1, 2)?.contiguous()?;
                let scores = (q.matmul(&kp.t()?)? * hscale)?; // [1, H, Qn, L]
                let probs = candle_nn::ops::softmax(&scores, candle_core::D::Minus1)?;
                let o = probs.matmul(&vp.contiguous()?)?; // [1, H, Qn, hd]
                let o = o.transpose(1, 2)?.reshape((1, qn, self.n_embd))?;
                let xq = {
                    let flat = xh.reshape((l, self.n_embd))?;
                    flat.index_select(&qrows_t, 0)?.unsqueeze(0)?
                }; // [1, Qn, d]
                let xq = (xq
                    + linear(&o, &ly.out_w)?
                        .broadcast_add(&ly.out_b.reshape((1, 1, self.n_embd))?)?)?;
                let f = layer_norm(&xq, &ly.norm2_w, Some(&ly.norm2_b), 1e-5)?;
                let f = linear(&f, &ly.l1_w)?
                    .broadcast_add(&ly.l1_b.reshape((1, 1, self.head_ff))?)?;
                let f = f.relu()?;
                let xq = (xq
                    + linear(&f, &ly.l2_w)?
                        .broadcast_add(&ly.l2_b.reshape((1, 1, self.n_embd))?)?)?;
                xh = xq;
            }
        }

        // ---- scorer on the marker rows. After a pruned last layer, xh rows
        // 1..=n_opt ARE the marker rows; otherwise gather from the full seq.
        let mk = if n_head_layers > 1 {
            xh.reshape((qn, self.n_embd))?.narrow(0, 1, n_opt)?
        } else {
            let gather: Vec<u32> = markers.iter().map(|&m| m as u32).collect();
            let idx = Tensor::new(gather.as_slice(), device)?.to_dtype(DType::U32)?;
            xh.reshape((l, self.n_embd))?.index_select(&idx, 0)?
        }; // [n_opt, d]
        let mk = mk.reshape((1, n_opt, self.n_embd))?;
        let s = layer_norm(&mk, &self.sc_norm_w, Some(&self.sc_norm_b), 1e-5)?;
        let s = linear(&s, &self.sc1_w)?
            .broadcast_add(&self.sc1_b.reshape((1, 1, self.n_embd))?)?;
        let s = gelu(&s)?;
        let s = linear(&s, &self.sc2_w)?
            .broadcast_add(&self.sc2_b.reshape((1, 1, 1))?)?; // [1, n_opt, 1]
        let s = s.squeeze(2)?.squeeze(0)?; // [n_opt]
        let raw: Vec<f32> = s.to_vec1()?;

        // ---- decode: temperature from the qtype:size bucket (official
        // temperature_for), then softmax.
        let t = self.temperature_for(req.decision_type, n_opt);
        let max = raw.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let exps: Vec<f32> = raw.iter().map(|v| ((v - max) / t).exp()).collect();
        let sum: f32 = exps.iter().sum();
        let probabilities: Vec<f32> = exps.iter().map(|e| e / sum).collect();
        let index = raw
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i)
            .unwrap_or(0);
        Ok(DecisionOutput {
            decision_type: req.decision_type,
            scores: raw,
            probabilities,
            index,
            option: req.options[index].clone(),
        })
    }

    fn temperature_for(&self, qt: DecisionType, k: usize) -> f32 {
        let size = if k <= 2 {
            "2"
        } else if k <= 5 {
            "3-5"
        } else if k <= 10 {
            "6-10"
        } else {
            "11+"
        };
        let bucket = format!("{}:{}", qt.as_str(), size);
        self.temperature_by_options
            .get(&bucket)
            .copied()
            .unwrap_or_else(|| {
                self.temperature
                    .get(qt.id() as usize)
                    .copied()
                    .unwrap_or(1.0)
            })
            .clamp(0.5, 5.0)
    }
}

/// Exact (erf) GELU — `0.5 * x * (1 + erf(x / sqrt(2)))`, matching the
/// official engine's geglu_rows / scorer gelu_erf.
fn gelu(x: &Tensor) -> Result<Tensor> {
    let t = x.affine(1.0 / std::f64::consts::SQRT_2, 0.0)?.erf()?;
    let t = (t + 1.0)?.affine(0.5, 0.0)?;
    Ok((x * t)?)
}

fn layer_norm(x: &Tensor, w: &Tensor, b: Option<&Tensor>, eps: f64) -> Result<Tensor> {
    let d = x.dim(candle_core::D::Minus1)?;
    let ln = candle_nn::LayerNorm::new(
        w.reshape(d)?,
        match b {
            Some(b) => b.reshape(d)?,
            None => Tensor::zeros(d, DType::F32, x.device())?,
        },
        eps,
    );
    Ok(ln.forward(x)?)
}

/// Bidirectional local-window mask [1,1,L,L]: 0 within ±half, -1e9 outside.
fn local_window_mask(l: usize, half: usize, device: &Device) -> Result<Tensor> {
    let mut m = vec![0f32; l * l];
    for i in 0..l {
        for j in 0..l {
            if (i as isize - j as isize).abs() as usize > half {
                m[i * l + j] = -1e9;
            }
        }
    }
    Ok(Tensor::from_vec(m, (1, 1, l, l), device)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mv::decision::DecisionType;
    use std::io::BufReader;
    use std::path::PathBuf;

    const TEMP_GGUF: &str = "runtimes/decision/model/statim/multilingual/model.gguf";

    fn load_model() -> Result<StatimModel> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(TEMP_GGUF);
        let f = std::fs::File::open(&path)?;
        let mut r = BufReader::new(f);
        StatimModel::load(&mut r, &Device::Cpu)
    }

    /// Probe: load the temp statim GGUF and run decisions. Verified against
    /// the official statim engine (github.com/BEKO2210/statim, v0.11.0 built
    /// from source) on the same inputs — token ids, marker positions and
    /// decisions match exactly; raw logits differ only by the official
    /// engine's int8 activation-quantized GEMM (this f32 path is the more
    /// precise reference):
    ///   q1 choice [2.478, -3.804]  vs official [2.582, -3.881] -> 继续运行
    ///   q2 choice [-0.502, 0.183]  vs official [-0.456, 0.035] -> "3"
    ///   q3 noul   [1.547, 0.121]   vs official [1.478, 0.077]  -> false
    ///   q4 score  [-0.671, -0.519] vs official [-0.669, -0.585] -> 正面
    /// `cargo test --lib mv::gguf_statim::tests::statim_probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn statim_probe() {
        let m = load_model().expect("load");
        let cases = vec![
            (DecisionType::Choice, "用户想退出程序", vec!["继续运行", "退出程序"]),
            (DecisionType::Choice, "1+1", vec!["2", "3"]),
            (DecisionType::Noul, "今天天气很好", vec!["false", "true"]),
            (DecisionType::Score, "这段文字的情感倾向", vec!["负面", "正面"]),
        ];
        for (qt, q, opts) in cases {
            let req = DecisionRequest {
                decision_type: qt,
                state: "对话进行中".to_string(),
                question: q.to_string(),
                options: opts.iter().map(|s| s.to_string()).collect(),
            };
            let (ids, _) = m.sequence(&req).expect("seq");
            println!("seq_len={} ids={:?}", ids.len(), ids);
            let out = m.decide(&req).expect("decide");
            println!(
                "{qt:?} q={q:?} opts={opts:?} -> scores={:?} probs={:?} idx={} opt={}",
                out.scores, out.probabilities, out.index, out.option
            );
            assert!(out.probabilities.iter().all(|p| p.is_finite()));
            assert!((out.probabilities.iter().sum::<f32>() - 1.0).abs() < 1e-4);
        }
    }

    /// Tokenizer parity vs the official statim engine (tok_dump harness).
    #[test]
    #[ignore]
    fn statim_tokenizer_probe() {
        let m = load_model().expect("load");
        let texts = [
            "choice question: 用户想退出程序",
            " choice 继续运行", " choice 退出程序",
            "choice question: 1+1", " choice 2", " choice 3",
            "对话进行中",
            "choice question: 这段文字的情感倾向", " choice 负面", " choice 正面",
            "noul question: 今天天气很好", " noul false", " noul true",
        ];
        let mut total = 0;
        for text in texts {
            let enc = m.tokenizer.encode(text, false).expect("encode");
            let ids = enc.get_ids().to_vec();
            total += ids.len();
            println!("{}\t{text:?}\t{ids:?}", ids.len());
        }
        println!("TOTAL {total}");
    }
}
