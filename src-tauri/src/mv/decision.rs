//! Julia-1 style decision model (`modern-bert` + decision head): turns a
//! state, a question and 2-20 candidate options into one structured decision
//! (a score per option + probabilities + the argmax option).
//!
//! Reference: SupersonicLabs/Julia-1 (`julia/model.py` + `julia/data.py`).
//! The GGUF conversion folds the 2-layer Transformer head into `blk.22/23`
//! (with biases + larger FFN) and the scorer into `cls.*`; `token_types` is
//! the `type_emb` (choice/score/noul). Verified against the official fp32
//! safetensors: identical decisions, scores within Q8_0 quantization noise.

use crate::mv::gguf_gemma::{apply_rope, attn_bias, linear, sliding_window_mask};
use crate::mv::gguf::{build_gguf_tokenizer, meta_u32};
use anyhow::{anyhow, Result};
use candle_core::quantized::gguf_file::{Content, Value};
use candle_core::{Device, Tensor};
use candle_core::Module;
use candle_nn::{ops, LayerNorm};
use std::io::{Read, Seek};
use std::path::Path;
use tokenizers::Tokenizer;

/// Question types (official `QTYPES`); `noul` options must be [false, true].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecisionType {
    Choice,
    Score,
    Noul,
}

impl DecisionType {
    fn id(self) -> u32 {
        match self {
            DecisionType::Choice => 0,
            DecisionType::Score => 1,
            DecisionType::Noul => 2,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            DecisionType::Choice => "choice",
            DecisionType::Score => "score",
            DecisionType::Noul => "noul",
        }
    }
}

/// One decision request (official `validate_row` semantics).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionRequest {
    #[serde(default = "default_decision_type")]
    pub decision_type: DecisionType,
    /// Context the decision is grounded in (text or JSON string).
    pub state: String,
    pub question: String,
    /// 2-20 non-empty candidate descriptions.
    pub options: Vec<String>,
}

fn default_decision_type() -> DecisionType {
    DecisionType::Choice
}

/// Structured decision result.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionOutput {
    pub decision_type: DecisionType,
    /// Raw logit per option (input order).
    pub scores: Vec<f32>,
    /// Softmax over `scores`.
    pub probabilities: Vec<f32>,
    /// Argmax index into `options`.
    pub index: usize,
    /// The selected option text.
    pub option: String,
}

/// One ModernBert encoder block (RMSNorm pre-norm, GLU-silu FFN, RoPE +
/// sliding window, no biases). Locally owned so the decision loader stays
/// independent of the embedding arch's private struct.
struct EncoderBlock {
    attn_norm: Option<LayerNorm>,
    ffn_norm: LayerNorm,
    qkv_w: Tensor,      // [in, out=3*hidden]
    out_w: Tensor,      // [in, out=hidden]
    ffn_up_w: Tensor,   // [in, out=2*ffn]
    ffn_down_w: Tensor, // [in=ffn, out=hidden]
    is_sliding: bool,
}

/// One PyTorch `TransformerEncoderLayer` (norm_first=True, ReLU MLP, biases).
struct HeadBlock {
    norm1_w: Tensor,
    norm1_b: Tensor,
    in_proj_w: Tensor, // [hidden, 3*hidden]
    in_proj_b: Tensor,
    out_proj_w: Tensor, // [hidden, hidden]
    out_proj_b: Tensor,
    norm2_w: Tensor,
    norm2_b: Tensor,
    linear1_w: Tensor, // [hidden, 4*hidden]
    linear1_b: Tensor,
    linear2_w: Tensor, // [4*hidden, hidden]
    linear2_b: Tensor,
}

enum Block {
    Encoder(EncoderBlock),
    Head(HeadBlock),
}

pub struct DecisionModel {
    tokenizer: Tokenizer,
    token_embd: Tensor, // [vocab, hidden]
    token_embd_norm_w: Tensor,
    output_norm_w: Tensor,
    type_emb: Tensor, // [3, hidden]
    scorer_norm_w: Tensor,
    scorer_norm_b: Tensor,
    scorer_w: Tensor, // [hidden, hidden]
    scorer_b: Tensor,
    output_w: Tensor, // [hidden, 1]
    output_b: Tensor,
    blocks: Vec<Block>,
    encoder_layers: usize,
    hidden: usize,
    n_head: usize,
    head_dim: usize,
    sliding_window: usize,
    rope_base: f32,
    /// Encoder FFN activation from `modern-bert.hidden_activation`
    /// ("gelu" for Julia-1, "silu" for granite-style conversions).
    hidden_activation: String,
    /// Question-head token budget (official `head_length`).
    head_length: usize,
    max_context: usize,
    device: Device,
    cls_id: u32,
    sep_id: u32,
    mask_id: u32,
}

impl DecisionModel {
    pub fn load<R: Read + Seek>(
        file: &mut R,
        size_dir: &Path,
        device: &Device,
        tokenizer_fallback: Option<&Path>,
    ) -> Result<Self> {
        let mut content = Content::read(file).map_err(|e| anyhow!("read gguf: {e}"))?;
        let md = &content.metadata;
        let pfx = "modern-bert.";

        let get_u32 = |key: &str, default: Option<usize>| -> Result<usize> {
            match md.get(key).map(|v| v.to_u32()) {
                Some(Ok(v)) => Ok(v as usize),
                Some(Err(e)) => Err(anyhow!("read {key} as u32: {e}")),
                None => default.ok_or_else(|| anyhow!("missing gguf key {key}")),
            }
        };
        let hidden = get_u32(&format!("{pfx}embedding_length"), None)?;
        let block_count = get_u32(&format!("{pfx}block_count"), None)?;
        let n_head = get_u32(&format!("{pfx}attention.head_count"), None)?;
        if n_head == 0 {
            return Err(anyhow!("invalid GGUF metadata: n_head must be > 0"));
        }
        let head_dim = hidden / n_head;
        let head_layers = get_u32(&format!("{pfx}decision.block_count"), Some(2))?;
        if head_layers == 0 || head_layers >= block_count {
            return Err(anyhow!(
                "invalid decision.block_count {head_layers} (must be 1..{block_count})"
            ));
        }
        let encoder_layers = block_count - head_layers;
        // Per-layer FFN (decision head blocks use 4*hidden) — the GGUF stores
        // an array; a scalar is accepted for uniform conversions.
        let ffn_arr: Vec<usize> = match md.get(&format!("{pfx}feed_forward_length")) {
            Some(Value::Array(a)) => a
                .iter()
                .map(|v| match v { Value::U32(x) => Ok(*x as usize), Value::I32(x) if *x > 0 => Ok(*x as usize), _ => Err(anyhow!("not a u32: {v:?}")) })
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| anyhow!("read feed_forward_length array: {e}"))?,
            Some(Value::U32(v)) => vec![*v as usize; block_count],
            other => return Err(anyhow!("unsupported feed_forward_length: {other:?}")),
        };
        if ffn_arr.len() < block_count {
            return Err(anyhow!(
                "feed_forward_length has {} entries, expected {block_count}",
                ffn_arr.len()
            ));
        }
        let sliding_window = get_u32(&format!("{pfx}attention.sliding_window"), Some(128))?;
        let pattern = get_u32(&format!("{pfx}attention.sliding_window_pattern"), Some(3))?;
        if pattern == 0 {
            return Err(anyhow!("invalid GGUF metadata: pattern must be > 0"));
        }
        let rope_base = md
            .get(&format!("{pfx}rope.freq_base"))
            .and_then(|v| v.to_f32().ok())
            .unwrap_or(150_000.0);
        let hidden_activation = md
            .get(&format!("{pfx}hidden_activation"))
            .and_then(|v| v.to_string().ok())
            .cloned()
            .unwrap_or_else(|| "silu".to_string());
        let head_length = get_u32(&format!("{pfx}decision.max_head_tokens"), Some(256))?;
        let max_context = get_u32(&format!("{pfx}context_length"), Some(8192))?;

        // Tokenizer: prefer a shipped tokenizer.json (official, exact); else
        // the caller-supplied bundled fallback (downloaded dirs contain only
        // model.gguf); else reconstruct BPE from the GGUF metadata. The GGUF
        // fallback is a last resort: julia_1's GGUF declares a GPT-2 vocab
        // while the real vocab is Metaspace, so ByteLevel pre-tokenization
        // yields mass <unk> and garbage decisions.
        let tokenizer_path = size_dir.join("tokenizer.json");
        let tokenizer = if tokenizer_path.exists() {
            Tokenizer::from_file(&tokenizer_path).map_err(|e| {
                // File name only — absolute paths must not leak to MCP
                // clients / remote callers (resolve_model_path convention).
                anyhow!(
                    "load tokenizer {}: {e}",
                    tokenizer_path.file_name().and_then(|n| n.to_str()).unwrap_or("tokenizer.json")
                )
            })?
        } else if let Some(fb) = tokenizer_fallback.filter(|p| p.exists()) {
            Tokenizer::from_file(fb).map_err(|e| {
                anyhow!(
                    "load bundled tokenizer {}: {e}",
                    fb.file_name().and_then(|n| n.to_str()).unwrap_or("tokenizer.json")
                )
            })?
        } else {
            build_gguf_tokenizer(md)?
        };

        // Official config: cls_token_id = sep_token_id = eos_token_id = 1,
        // bos = 2, mask = 4. The GGUF's bos/eos keys are the CLS/SEP source
        // for this model family. Sanity-check the ids against the loaded
        // tokenizer's vocab (this family uses <bos>/<eos>/<mask>-style
        // special tokens — no bracket heuristic) so a GGUF/tokenizer.json
        // mismatch fails loudly instead of silently pooling the wrong
        // position.
        let cls_id = meta_u32(md, "tokenizer.ggml.bos_token_id").unwrap_or(1);
        let sep_id = meta_u32(md, "tokenizer.ggml.eos_token_id").unwrap_or(1);
        let mask_id = meta_u32(md, "tokenizer.ggml.mask_token_id")
            .ok_or_else(|| anyhow!("missing gguf key tokenizer.ggml.mask_token_id"))?;
        for (id, expect) in [
            (cls_id, "cls"),
            (sep_id, "sep"),
            (mask_id, "mask"),
        ] {
            match tokenizer.id_to_token(id) {
                Some(t) if !t.is_empty() => {
                    let _ = t;
                }
                Some(_) => {
                    return Err(anyhow!(
                        "tokenizer special-token mismatch: {expect} id {id} resolves to an empty token"
                    ));
                }
                None => {
                    return Err(anyhow!(
                        "tokenizer special-token mismatch: {expect} id {id} not in tokenizer vocab"
                    ));
                }
            }
        }

        fn dq<R: Read + Seek>(
            file: &mut R,
            content: &mut Content,
            name: &str,
            device: &Device,
        ) -> Result<Tensor> {
            content
                .tensor(file, name, device)
                .map_err(|e| anyhow!("tensor {name}: {e}"))?
                .dequantize(device)
                .map_err(|e| anyhow!("dequantize {name}: {e}"))
        }
        macro_rules! dq {
            ($n:expr) => {
                dq(file, &mut content, $n, device)?
            };
        }
        // candle returns logical shapes (GGUF dims reversed on disk):
        // 2D linears [out, in], token_embd [vocab, hidden]. Linears are
        // transposed to [in, out] at load so forward does plain `x @ W`.
        let token_embd = dq!("token_embd.weight");
        let token_embd_norm_w = dq!("token_embd_norm.weight");
        let output_norm_w = dq!("output_norm.weight");
        let type_emb_t = dq!("token_types.weight"); // logical [3, hidden]
        let type_emb = if type_emb_t.dims()[0] == 3 {
            type_emb_t
        } else {
            type_emb_t.t()?
        };

        // Julia's modernbert variant normalizes with BIAS-FREE LayerNorm
        // (mean+variance), unlike granite's RMSNorm — verified against the
        // official encoder (emb/final/attn/mlp norms are all LayerNorm with
        // bias=None). The GGUF ships weight-only norm tensors.
        let ln = |w: Tensor| -> Result<LayerNorm> {
            let h = w.dim(0)?;
            let zero = Tensor::zeros(h, candle_core::DType::F32, device)?;
            Ok(LayerNorm::new(w, zero, 1e-5))
        };
        let mut blocks = Vec::with_capacity(block_count);
        for i in 0..block_count {
            if i < encoder_layers {
                let attn_norm = if i == 0 {
                    match content.tensor(file, "blk.0.attn_norm.weight", device) {
                        Ok(w) => Some(ln(w.dequantize(device)?)?),
                        Err(_) => None,
                    }
                } else {
                    let w = content
                        .tensor(file, &format!("blk.{i}.attn_norm.weight"), device)?
                        .dequantize(device)?;
                    Some(ln(w)?)
                };
                let ffn_norm_w = content
                    .tensor(file, &format!("blk.{i}.ffn_norm.weight"), device)?
                    .dequantize(device)?;
                let qkv_w = content
                    .tensor(file, &format!("blk.{i}.attn_qkv.weight"), device)?
                    .dequantize(device)?
                    .t()?;
                let out_w = content
                    .tensor(file, &format!("blk.{i}.attn_output.weight"), device)?
                    .dequantize(device)?
                    .t()?;
                let ffn_up_w = content
                    .tensor(file, &format!("blk.{i}.ffn_up.weight"), device)?
                    .dequantize(device)?
                    .t()?;
                let ffn_down_w = content
                    .tensor(file, &format!("blk.{i}.ffn_down.weight"), device)?
                    .dequantize(device)?
                    .t()?;
                blocks.push(Block::Encoder(EncoderBlock {
                    attn_norm,
                    ffn_norm: ln(ffn_norm_w)?,
                    qkv_w,
                    out_w,
                    ffn_up_w,
                    ffn_down_w,
                    is_sliding: i % pattern != 0,
                }));
            } else {
                let norm1_w = dq(file, &mut content, &format!("blk.{i}.attn_norm.weight"), device)?;
                let norm1_b = dq(file, &mut content, &format!("blk.{i}.attn_norm.bias"), device)?;
                let in_proj_w = dq(file, &mut content, &format!("blk.{i}.attn_qkv.weight"), device)?.t()?;
                let in_proj_b = dq(file, &mut content, &format!("blk.{i}.attn_qkv.bias"), device)?;
                let out_proj_w = dq(file, &mut content, &format!("blk.{i}.attn_output.weight"), device)?.t()?;
                let out_proj_b = dq(file, &mut content, &format!("blk.{i}.attn_output.bias"), device)?;
                let norm2_w = dq(file, &mut content, &format!("blk.{i}.ffn_norm.weight"), device)?;
                let norm2_b = dq(file, &mut content, &format!("blk.{i}.ffn_norm.bias"), device)?;
                let linear1_w = dq(file, &mut content, &format!("blk.{i}.ffn_up.weight"), device)?.t()?;
                let linear1_b = dq(file, &mut content, &format!("blk.{i}.ffn_up.bias"), device)?;
                let linear2_w = dq(file, &mut content, &format!("blk.{i}.ffn_down.weight"), device)?.t()?;
                let linear2_b = dq(file, &mut content, &format!("blk.{i}.ffn_down.bias"), device)?;
                blocks.push(Block::Head(HeadBlock {
                    norm1_w,
                    norm1_b,
                    in_proj_w,
                    in_proj_b,
                    out_proj_w,
                    out_proj_b,
                    norm2_w,
                    norm2_b,
                    linear1_w,
                    linear1_b,
                    linear2_w,
                    linear2_b,
                }));
            }
        }

        Ok(Self {
            tokenizer,
            token_embd,
            token_embd_norm_w,
            output_norm_w,
            type_emb,
            scorer_norm_w: dq!("cls.norm.weight"),
            scorer_norm_b: dq!("cls.norm.bias"),
            scorer_w: dq!("cls.weight").t()?,
            scorer_b: dq!("cls.bias"),
            output_w: {
                // 1-D [hidden] in this GGUF (hf stores [1, hidden]); reshape
                // to [hidden, 1] for the final linear.
                let w = dq!("cls.output.weight");
                if w.rank() == 1 {
                    w.reshape((w.dim(0)?, 1))?
                } else {
                    w.t()?
                }
            },
            output_b: dq!("cls.output.bias"),
            blocks,
            encoder_layers,
            hidden,
            n_head,
            head_dim,
            sliding_window,
            rope_base,
            hidden_activation,
            head_length,
            max_context,
            device: device.clone(),
            cls_id,
            sep_id,
            mask_id,
        })
    }

    /// Resolve the selected decision model GGUF via the shared kind-aware
    /// resolver (`decision/<family>/<size>/model.gguf`, downloaded copy
    /// preferred over bundled).
    pub fn resolve_model_path(app: &tauri::AppHandle, name: &str) -> Result<std::path::PathBuf> {
        if name.is_empty()
            || name == "."
            || name.contains("..")
            || name.contains('/')
            || name.contains('\\')
        {
            return Err(anyhow!("invalid decision model name"));
        }
        let dir = super::models::resolve_model_paths_kind(
            app,
            super::models::ModelKind::Decision,
            name,
        )?
        .ok_or_else(|| anyhow!("decision model '{name}' is not ready - download it first"))?;
        Ok(dir.join("model.gguf"))
    }

    /// Tokenize + serialize one request into `(ids, marker_positions)`
    /// following the official `sequence()` exactly.
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
        // Resource-exhaustion guard: tokenization cost is proportional to
        // input length and truncation (take(48)/take(budget)/take(room))
        // happens only AFTER the full encode — reject absurd inputs before
        // any tokenization. The model context is ~8k tokens; 64k chars per
        // field / 128k total is far above anything useful.
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
        // Official validate_row contract: noul decisions are the fixed
        // [false, true] option pair. Enforced here (not only in the MCP arg
        // parser) so the Tauri command path shares one enforcement point.
        if req.decision_type == DecisionType::Noul
            && req.options != vec!["false".to_string(), "true".to_string()]
        {
            return Err(anyhow!(
                "noul decisions require exactly the option pair [\"false\", \"true\"]"
            ));
        }
        // An empty mask string would make `replace("", " ")` insert a space
        // between EVERY character — fail loudly instead of silently mangling
        // all input text.
        let mask_str = self.tokenizer.id_to_token(self.mask_id).ok_or_else(|| {
            anyhow!("tokenizer has no token for mask id {}", self.mask_id)
        })?;
        let clean = |t: &str| t.replace(&mask_str, " ");
        let enc = |t: &str| -> Result<Vec<u32>> {
            Ok(self
                .tokenizer
                .encode(t, false)
                .map_err(|e| anyhow!("tokenize: {e}"))?
                .get_ids()
                .to_vec())
        };
        let head_ids = enc(&format!(
            "{} question: {}",
            req.decision_type.as_str(),
            clean(&req.question)
        ))?;
        let mut options: Vec<Vec<u32>> = req
            .options
            .iter()
            .map(|o| -> Result<Vec<u32>> {
                let mut v = vec![self.mask_id];
                v.extend(enc(&format!(" {}", clean(o)))?.into_iter().take(48));
                Ok(v)
            })
            .collect::<Result<Vec<_>>>()?;
        let mut budget = self
            .head_length
            .saturating_sub(options.iter().map(|o| o.len()).sum::<usize>());
        if budget < 16 {
            let per_option = std::cmp::max(4, self.head_length.saturating_sub(16) / options.len());
            for o in options.iter_mut() {
                o.truncate(per_option);
            }
            budget = self
                .head_length
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
        let state_ids = enc(&clean(&req.state))?;
        let room = self.max_context.saturating_sub(ids.len() + 1);
        if room < 1 {
            return Err(anyhow!("question/options exceed the sequence budget"));
        }
        ids.extend(state_ids.into_iter().take(room));
        ids.push(self.sep_id);
        Ok((ids, markers))
    }

    /// Run the full decision forward and return the structured output.
    pub fn decide(&self, req: &DecisionRequest) -> Result<DecisionOutput> {
        let (ids, markers) = self.sequence(req)?;
        let seq = ids.len();
        let device = &self.device;
        let input = Tensor::new(ids.as_slice(), device)?.reshape((1, seq))?;
        let attn = Tensor::ones((1, seq), candle_core::DType::F32, device)?;
        let ids_flat = input.flatten_all()?;

        // Encoder: token embedding + RMSNorm, then ModernBert blocks.
        let mut x = self.token_embd.embedding(&ids_flat)?;
        x = x.reshape((1, seq, self.hidden))?;
        let emb_ln = LayerNorm::new(
            self.token_embd_norm_w.clone(),
            Tensor::zeros(self.hidden, candle_core::DType::F32, device)?,
            1e-5,
        );
        x = emb_ln.forward(&x)?;
        let mask_bias = attn_bias(&attn)?;
        let positions = Tensor::arange(0u32, seq as u32, device)?;
        for blk in self.blocks.iter() {
            match blk {
                Block::Encoder(b) => x = self.forward_encoder(b, &x, &positions, &mask_bias)?,
                Block::Head(_) => break,
            }
        }
        // Official model: hidden = encoder(...) + type_emb(qtype) BEFORE the
        // head; the head blocks then run over the full sequence.
        let te = self.type_emb.narrow(0, req.decision_type.id() as usize, 1)?; // [1, hidden]
        x = x.broadcast_add(&te.reshape((1, 1, self.hidden))?)?;
        for blk in self.blocks.iter().skip(self.encoder_layers) {
            match blk {
                Block::Head(b) => x = self.forward_head(b, &x, &mask_bias)?,
                Block::Encoder(_) => {}
            }
        }
        let out_ln = LayerNorm::new(
            self.output_norm_w.clone(),
            Tensor::zeros(self.hidden, candle_core::DType::F32, device)?,
            1e-5,
        );
        x = out_ln.forward(&x)?;

        // Gather the marker positions and score each: LayerNorm → Linear →
        // GELU → Linear → scalar.
        let n_opt = markers.len();
        let mk_u32: Vec<u32> = markers.iter().map(|&m| m as u32).collect();
        let mk_idx = Tensor::new(&mk_u32[..], device)?.reshape((1, n_opt, 1))?;
        let mk = x.gather(&mk_idx.expand((1, n_opt, self.hidden))?.contiguous()?, 1)?; // [1, n_opt, hidden]
        let s = LayerNorm::new(
            self.scorer_norm_w.clone(),
            self.scorer_norm_b.clone(),
            1e-5,
        )
        .forward(&mk)?;
        let s = linear(&s, &self.scorer_w)?;
        let s = s.broadcast_add(&self.scorer_b.reshape((1, 1, self.hidden))?)?;
        let s = gelu(&s)?;
        let s = linear(&s, &self.output_w)?; // [1, n_opt, 1]
        let s = s.broadcast_add(&self.output_b.reshape((1, 1, 1))?)?
            .squeeze(2)?
            .squeeze(0)?; // [n_opt]
        let scores: Vec<f32> = s.to_vec1()?;
        let max = scores.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let exps: Vec<f32> = scores.iter().map(|v| (v - max).exp()).collect();
        let sum: f32 = exps.iter().sum();
        let probabilities: Vec<f32> = exps.iter().map(|e| e / sum).collect();
        let index = scores
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i)
            .unwrap_or(0);
        Ok(DecisionOutput {
            decision_type: req.decision_type,
            scores,
            probabilities,
            index,
            option: req.options[index].clone(),
        })
    }

    fn forward_encoder(
        &self,
        blk: &EncoderBlock,
        x: &Tensor,
        positions: &Tensor,
        mask_bias: &Tensor,
    ) -> Result<Tensor> {
        let (b, seq, _h) = x.dims3()?;
        let hn = match blk.attn_norm.as_ref() {
            Some(n) => n.forward(x)?,
            None => x.clone(), // layer 0: identity
        };
        let qkv = linear(&hn, &blk.qkv_w)?;
        let qkv = qkv.reshape((b, seq, 3, self.n_head, self.head_dim))?;
        let q = qkv.narrow(2, 0, 1)?.squeeze(2)?;
        let k = qkv.narrow(2, 1, 1)?.squeeze(2)?;
        let v = qkv.narrow(2, 2, 1)?.squeeze(2)?;
        let q = apply_rope(&q, positions, self.head_dim, self.rope_base, &self.device)?;
        let k = apply_rope(&k, positions, self.head_dim, self.rope_base, &self.device)?;
        let q = q.transpose(1, 2)?.contiguous()?;
        let k = k.transpose(1, 2)?.contiguous()?;
        let v = v.transpose(1, 2)?.contiguous()?;
        let scale = 1.0f64 / (self.head_dim as f64).sqrt();
        let mut qk = q.matmul(&k.t()?)?.affine(scale, 0.0)?;
        if blk.is_sliding {
            let sw = sliding_window_mask(seq, self.sliding_window, &self.device)?;
            qk = qk.broadcast_add(&sw)?;
        }
        let qk = qk.broadcast_add(mask_bias)?;
        let probs = ops::softmax_last_dim(&qk)?;
        let attn = probs.matmul(&v.contiguous()?)?;
        let attn = attn.transpose(1, 2)?.reshape((b, seq, self.hidden))?;
        let attn = linear(&attn, &blk.out_w)?;
        let x = (x + &attn)?;
        let hn = blk.ffn_norm.forward(&x)?;
        let up = linear(&hn, &blk.ffn_up_w)?;
        let ffn = up.dim(2)?;
        // HF ModernBertMLP: first half = input (activated), second = gate.
        // Activation comes from the GGUF metadata (julia = gelu, granite = silu).
        let input = up.narrow(2, 0, ffn / 2)?;
        let input = if self.hidden_activation == "gelu" {
            gelu(&input)?
        } else {
            input.silu()?
        };
        let gate = up.narrow(2, ffn / 2, ffn / 2)?;
        let mid = input.broadcast_mul(&gate)?;
        let ffn_out = linear(&mid, &blk.ffn_down_w)?;
        Ok((x + &ffn_out)?)
    }

    /// PyTorch `TransformerEncoderLayer` (norm_first=True, ReLU MLP):
    /// x = x + self_attn(LN1(x)); x = x + linear2(relu(linear1(LN2(x)))).
    fn forward_head(&self, blk: &HeadBlock, x: &Tensor, mask_bias: &Tensor) -> Result<Tensor> {
        let (b, seq, _h) = x.dims3()?;
        let ln1 = LayerNorm::new(blk.norm1_w.clone(), blk.norm1_b.clone(), 1e-5).forward(x)?;
        let qkv = linear(&ln1, &blk.in_proj_w)?; // [b, seq, 3*hidden]
        let qkv = qkv.broadcast_add(&blk.in_proj_b.reshape((1, 1, 3 * self.hidden))?)?;
        let qkv = qkv.reshape((b, seq, 3, self.n_head, self.head_dim))?;
        let q = qkv
            .narrow(2, 0, 1)?
            .squeeze(2)?
            .transpose(1, 2)?
            .contiguous()?;
        let k = qkv
            .narrow(2, 1, 1)?
            .squeeze(2)?
            .transpose(1, 2)?
            .contiguous()?;
        let v = qkv
            .narrow(2, 2, 1)?
            .squeeze(2)?
            .transpose(1, 2)?
            .contiguous()?;
        let scale = 1.0f64 / (self.head_dim as f64).sqrt();
        let qk = q.matmul(&k.t()?)?.affine(scale, 0.0)?;
        let qk = qk.broadcast_add(mask_bias)?;
        let probs = ops::softmax_last_dim(&qk)?;
        let attn = probs.matmul(&v.contiguous()?)?;
        let attn = attn.transpose(1, 2)?.reshape((b, seq, self.hidden))?;
        let attn = linear(&attn, &blk.out_proj_w)?;
        let attn = attn.broadcast_add(&blk.out_proj_b.reshape((1, 1, self.hidden))?)?;
        let x = (x + &attn)?;
        let ln2 = LayerNorm::new(blk.norm2_w.clone(), blk.norm2_b.clone(), 1e-5).forward(&x)?;
        let l1 = linear(&ln2, &blk.linear1_w)?;
        let l1 = l1.broadcast_add(&blk.linear1_b.reshape((1, 1, 4 * self.hidden))?)?;
        let l1 = l1.relu()?;
        let l2 = linear(&l1, &blk.linear2_w)?;
        let l2 = l2.broadcast_add(&blk.linear2_b.reshape((1, 1, self.hidden))?)?;
        Ok((x + &l2)?)
    }
}

/// Exact (erf) GELU — `nn.GELU()` default in the official scorer:
/// `x * 0.5 * (1 + erf(x / sqrt(2)))`.
fn gelu(x: &Tensor) -> Result<Tensor> {
    let t = x.affine(1.0 / std::f64::consts::SQRT_2, 0.0)?.erf()?;
    let t = (t + 1.0)?.affine(0.5, 0.0)?;
    Ok((x * t)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Numerical probe against the official Julia-1 reference (python,
    /// official `JuliaDecisionModel` code): the GGUF weights through the
    /// official pipeline give
    ///   0 choice [5.466, 2.745] / 1 choice [-4.024, 7.221]
    ///   2 noul   [-1.439, 2.233] / 3 noul [-4.596, -3.847]
    ///   4 score  [1.896, -4.582]
    /// (case 2 is a genuine model miss — official fp32 weights miss it too).
    /// `cargo test --lib mv::decision::tests::julia_reference_probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn julia_reference_probe() {
        let gguf = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("runtimes/rag/model/decision/julia/1/model.gguf");
        if !gguf.is_file() {
            eprintln!("SKIP: julia_1 model.gguf not present");
            return;
        }
        let device = Device::Cpu;
        let mut file = std::fs::File::open(&gguf).unwrap();
        let size_dir = gguf.parent().unwrap().to_path_buf();
        let m = DecisionModel::load(&mut file, &size_dir, &device, None).expect("load");
        let cases = [
            (DecisionType::Choice, "user asks in Chinese how to check their public IP; tools available: getPublicIp (returns public IP), listFiles (lists local files)", "Which tool should be used?", vec!["getPublicIp: returns the machine public IP address", "listFiles: lists files in a local directory"], 0usize),
            (DecisionType::Choice, "user asks to list files in /tmp", "Which tool should be used?", vec!["getPublicIp: returns the machine public IP address", "listFiles: lists files in a local directory"], 1),
            (DecisionType::Noul, "user message: \"hi there\"", "Does this message require a tool call?", vec!["false", "true"], 0),
            (DecisionType::Noul, "user message: \"帮我查一下公网ip\"", "Does this message require a tool call?", vec!["false", "true"], 1),
            (DecisionType::Score, "candidate tools for the query \"translate this document to English\"", "Rate tool relevance", vec!["translate_text: translates text between languages", "get_weather: returns weather forecast"], 0),
        ];
        for (i, (t, state, q, opts, target)) in cases.iter().enumerate() {
            let out = m
                .decide(&DecisionRequest {
                    decision_type: *t,
                    state: state.to_string(),
                    question: q.to_string(),
                    options: opts.iter().map(|s| s.to_string()).collect(),
                })
                .expect("decide");
            eprintln!(
                "case {i} {:?} scores {:?} probs {:?} -> {} (target {target}) {}",
                out.decision_type,
                out.scores.iter().map(|v| format!("{v:.3}")).collect::<Vec<_>>(),
                out.probabilities.iter().map(|v| format!("{v:.3}")).collect::<Vec<_>>(),
                out.option,
                if out.index == *target { "✅" } else { "❌" }
            );
            // Case 2 is a known model miss (official fp32 weights also pick
            // "true" for "hi there"); every other case must match the target.
            if i != 2 {
                assert_eq!(out.index, *target, "case {i} argmax mismatch");
            }
        }
    }

    /// Decisive probe for the "user reports odd decision output" ticket:
    /// the SAME user input run through (A) the shipped tokenizer.json and
    /// (B) the GGUF-embedded tokenizer reconstruction. If (B) reproduces the
    /// user's exact scores, the running binary took the GGUF-tokenizer path
    /// (missing tokenizer.json) — proving the shipped tokenizer.json is
    /// REQUIRED, not optional.
    /// `cargo test --lib mv::decision::tests::julia_user_case_probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn julia_user_case_probe() {
        let src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("runtimes/rag/model/decision/julia/1");
        let gguf = src.join("model.gguf");
        if !gguf.is_file() {
            eprintln!("SKIP: julia_1 model.gguf not present");
            return;
        }
        // Print the GGUF-embedded tokenizer family for the record.
        {
            let mut f = std::fs::File::open(&gguf).unwrap();
            let content = Content::read(&mut f).unwrap();
            let model = content
                .metadata
                .get("tokenizer.ggml.model")
                .and_then(|v| v.to_string().ok());
            eprintln!("GGUF-embedded tokenizer.ggml.model = {model:?}");
        }
        let req = DecisionRequest {
            decision_type: DecisionType::Score,
            state: "今天是11月16号，今天的白天温度是20~30度，天气预报说天气很高".into(),
            question: "请问今天10~13点有可能下雨么".into(),
            options: vec!["晴天".into(), "下雨".into()],
        };
        // Math case reported by the user (score type, 3 options).
        let math = DecisionRequest {
            decision_type: DecisionType::Score,
            state: "在义务教育的数学场景下".into(),
            question: "1+1等于几".into(),
            options: vec!["4".into(), "3".into(), "2".into()],
        };
        // Order-swapped control: score semantics should be order-invariant.
        let math_swapped = DecisionRequest {
            options: vec!["2".into(), "3".into(), "4".into()],
            ..math.clone()
        };
        // English-state control.
        let math_en = DecisionRequest {
            state: "in a compulsory-education math context".into(),
            question: "What is 1+1 equal to?".into(),
            options: vec!["4".into(), "3".into(), "2".into()],
            decision_type: DecisionType::Score,
        };
        // Choice-type control for the same math question.
        let math_choice = DecisionRequest {
            decision_type: DecisionType::Choice,
            ..math.clone()
        };
        // A) shipped tokenizer.json fast path.
        let mut f = std::fs::File::open(&gguf).unwrap();
        let m = DecisionModel::load(&mut f, &src, &Device::Cpu, None).expect("load shipped");
        // Tokenization round-trip diagnostics for the math inputs.
        for text in ["score question: 1+1等于几", "1+1等于几", " 4", " 3", " 2", "今天天气怎么样"] {
            let enc = m.tokenizer.encode(text, false).expect("encode");
            let ids: Vec<u32> = enc.get_ids().to_vec();
            let toks: Vec<String> = ids
                .iter()
                .map(|&i| m.tokenizer.id_to_token(i).unwrap_or_default())
                .collect();
            let dec = m.tokenizer.decode(&ids, false).map(|d| d.to_string()).unwrap_or_default();
            eprintln!("tok {text:?} -> ids {ids:?}\n   tokens {toks:?}\n   decode {dec:?}");
        }
        let out = m.decide(&req).expect("decide A");
        let math_mid = DecisionRequest { options: vec!["4".into(), "2".into(), "3".into()], ..math.clone() };
        let math_first = DecisionRequest { options: vec!["2".into(), "4".into(), "3".into()], ..math.clone() };
        let math_two = DecisionRequest { options: vec!["4".into(), "2".into()], ..math.clone() };
        for (tag, r) in [
            ("[math score]", &math),
            ("[math score swapped]", &math_swapped),
            ("[math mid-answer]", &math_mid),
            ("[math first-answer]", &math_first),
            ("[math 2-options]", &math_two),
            ("[math score EN]", &math_en),
            ("[math choice]", &math_choice),
        ] {
            let o = m.decide(r).expect("decide math");
            eprintln!(
                "{tag:<22} scores {:?} probs {:?} -> {}",
                o.scores.iter().map(|v| format!("{v:.4}")).collect::<Vec<_>>(),
                o.probabilities.iter().map(|v| format!("{v:.4}")).collect::<Vec<_>>(),
                o.option
            );
        }
        // Choice-type control: the same weather input is semantically a
        // pick-one question, not a rating task.
        let req_choice = DecisionRequest {
            decision_type: DecisionType::Choice,
            ..req.clone()
        };
        let oc = m.decide(&req_choice).expect("decide A-choice");
        eprintln!(
            "[A choice-type control]     scores {:?} probs {:?} -> {}",
            oc.scores.iter().map(|v| format!("{v:.7}")).collect::<Vec<_>>(),
            oc.probabilities.iter().map(|v| format!("{v:.7}")).collect::<Vec<_>>(),
            oc.option
        );
        // Score-type with cleaner state (standard phrasing).
        let req2 = DecisionRequest {
            decision_type: DecisionType::Score,
            state: "今天白天温度20到30度，天气晴朗，无降水迹象".into(),
            ..req.clone()
        };
        let o2 = m.decide(&req2).expect("decide A2");
        eprintln!(
            "[A score, cleaner state]    scores {:?} probs {:?} -> {}",
            o2.scores.iter().map(|v| format!("{v:.7}")).collect::<Vec<_>>(),
            o2.probabilities.iter().map(|v| format!("{v:.7}")).collect::<Vec<_>>(),
            o2.option
        );
        eprintln!(
            "[A shipped tokenizer.json]  scores {:?} probs {:?} -> {}",
            out.scores.iter().map(|v| format!("{v:.7}")).collect::<Vec<_>>(),
            out.probabilities.iter().map(|v| format!("{v:.7}")).collect::<Vec<_>>(),
            out.option
        );
        // B) GGUF-reconstructed tokenizer (size dir WITHOUT tokenizer.json).
        let tmp = std::env::temp_dir().join("julia_probe_gguf_tok");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::hard_link(&gguf, tmp.join("model.gguf")).unwrap();
        let mut f = std::fs::File::open(&gguf).unwrap();
        let m2 = DecisionModel::load(&mut f, &tmp, &Device::Cpu, None).expect("load gguf-tok");
        let out2 = m2.decide(&req).expect("decide B");
        eprintln!(
            "[B GGUF tokenizer]          scores {:?} probs {:?} -> {}   (user reported: [1.2778271, 1.6409860] -> 下雨)",
            out2.scores.iter().map(|v| format!("{v:.7}")).collect::<Vec<_>>(),
            out2.probabilities.iter().map(|v| format!("{v:.7}")).collect::<Vec<_>>(),
            out2.option
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

