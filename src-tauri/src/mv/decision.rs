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
    pub(crate) fn id(self) -> u32 {
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

/// Public decision-model handle: dispatches between the Julia-style
/// `modern-bert` GGUF layout and the laya `ggmlc` fused-graph layout.
pub struct DecisionModel {
    pub tokenizer: Tokenizer,
    inner: Inner,
}

enum Inner {
    ModernBert(ModernBertModel),
    Ggmlc(super::ggmlc::GgmlcModel),
    Statim(super::gguf_statim::StatimModel),
    Jev(super::gguf_jev::JevModel),
}

impl DecisionModel {
    pub fn load<R: Read + Seek>(
        file: &mut R,
        size_dir: &Path,
        device: &Device,
        tokenizer_fallback: Option<&Path>,
    ) -> Result<Self> {
        use std::io::SeekFrom;
        // Peek the architecture, then rewind and let the backend re-read.
        let arch = {
            file.seek(SeekFrom::Start(0))?;
            let c = Content::read(file).map_err(|e| anyhow!("read gguf: {e}"))?;
            c.metadata
                .get("general.architecture")
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    _ => "modern-bert".to_string(),
                })
                .unwrap_or_else(|| "modern-bert".to_string())
        };
        file.seek(SeekFrom::Start(0))?;
        match arch.as_str() {
            "ggmlc" => {
                let m = super::ggmlc::GgmlcModel::load(file, device)?;
                let tokenizer = m.tokenizer.clone();
                Ok(Self {
                    tokenizer,
                    inner: Inner::Ggmlc(m),
                })
            }
            "laya" => {
                let m = super::gguf_statim::StatimModel::load(file, device)?;
                let tokenizer = m.tokenizer.clone();
                Ok(Self {
                    tokenizer,
                    inner: Inner::Statim(m),
                })
            }
            "qwen35" => {
                let m = super::gguf_jev::JevModel::load(file, device)?;
                let tokenizer = m.tokenizer.clone();
                Ok(Self {
                    tokenizer,
                    inner: Inner::Jev(m),
                })
            }
            _ => {
                let m = ModernBertModel::load(file, size_dir, device, tokenizer_fallback)?;
                let tokenizer = m.tokenizer.clone();
                Ok(Self {
                    tokenizer,
                    inner: Inner::ModernBert(m),
                })
            }
        }
    }

    /// Run the full decision forward and return the structured output.
    pub fn decide(&self, req: &DecisionRequest) -> Result<DecisionOutput> {
        match &self.inner {
            Inner::ModernBert(m) => m.decide(req),
            Inner::Ggmlc(m) => m.decide(req),
            Inner::Statim(m) => m.decide(req),
            Inner::Jev(m) => m.decide(req),
        }
    }

    /// TEST-ONLY: same forward but with the tokenizer's post-processor
    /// template applied (see `ModernBertModel::decide_with_special`).
    #[cfg(test)]
    fn decide_with_special(&self, req: &DecisionRequest) -> Result<DecisionOutput> {
        match &self.inner {
            Inner::ModernBert(m) => m.decide_with_special(req),
            Inner::Ggmlc(_) => anyhow::bail!("decide_with_special is modern-bert only"),
            Inner::Statim(_) => anyhow::bail!("decide_with_special is modern-bert only"),
            Inner::Jev(_) => anyhow::bail!("decide_with_special is modern-bert only"),
        }
    }

    /// Resolve the selected decision model GGUF via the shared kind-aware
    /// resolver (`decision/<family>/<size>/model.gguf`, downloaded copy
    /// preferred over bundled).
    pub fn resolve_model_path(app: &tauri::AppHandle, name: &str) -> Result<std::path::PathBuf> {
        // Accept both legacy bare sizes ("multilingual") and the unique
        // "<family>/<size>" selection keys ("statim/multilingual") — the
        // key-aware resolver validates each segment (no traversal, no ':').
        let key_ok = if name.contains('/') {
            super::models::split_model_key(name).is_some()
        } else {
            super::models::valid_model_size_name(name)
        };
        if !key_ok {
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
}

struct ModernBertModel {
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

impl ModernBertModel {
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
        // Tokenizer resolution order: `tokenizer.json` → `tokenizer.json.gz`
        // (the 34MB gemma vocab compresses to ~9MB; decompressed in memory) →
        // the caller-supplied bundled fallback (downloaded dirs contain only
        // model.gguf) → reconstruct from the GGUF metadata. The GGUF path is
        // verified token-exact for this family (`pre = mmbert`/`gemma` routes
        // to the Metaspace builder) but the shipped file stays the primary
        // source so a GGUF metadata regression cannot silently change
        // tokenization.
        let load_tokenizer_file = |p: &Path| -> Result<Tokenizer> {
            if p.extension().and_then(|e| e.to_str()) == Some("gz") {
                let f = std::fs::File::open(p).map_err(|e| anyhow!("open {}: {e}", p.display()))?;
                let mut s = String::new();
                flate2::read::GzDecoder::new(f)
                    .read_to_string(&mut s)
                    .map_err(|e| anyhow!("gunzip {}: {e}", p.display()))?;
                Tokenizer::from_bytes(&s).map_err(|e| anyhow!("parse {}: {e}", p.display()))
            } else {
                Tokenizer::from_file(p).map_err(|e| anyhow!("load {}: {e}", p.display()))
            }
        };
        let tokenizer_json = size_dir.join("tokenizer.json");
        let tokenizer_gz = size_dir.join("tokenizer.json.gz");
        let tokenizer = if tokenizer_json.exists() {
            load_tokenizer_file(&tokenizer_json)?
        } else if tokenizer_gz.exists() {
            load_tokenizer_file(&tokenizer_gz)?
        } else if let Some(fb) = tokenizer_fallback.filter(|p| p.exists()) {
            load_tokenizer_file(fb)?
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

        // Julia's encoder is HF `ModernBertModel` (model_type=modernbert, no
        // auto_map): HF modeling_modernbert.py uses BIAS-FREE LayerNorm
        // (mean+variance, norm_bias=false) for emb/final/attn/mlp norms and
        // nn.Identity for layer 0's attn_norm. Head + scorer keep standard
        // LayerNorm with bias. Verified against the official inference code
        // (julia/model.py + transformers AutoModel) — the GGUF ships
        // weight-only norm tensors.
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
        // Official order: encoder output (final_norm applied INSIDE the
        // encoder, i.e. BEFORE type_emb) → + type_emb → head → gather →
        // scorer. HF ModernBertModel.last_hidden_state already carries
        // final_norm, so apply output_norm here, before type_emb.
        let out_ln = LayerNorm::new(
            self.output_norm_w.clone(),
            Tensor::zeros(self.hidden, candle_core::DType::F32, device)?,
            1e-5,
        );
        x = out_ln.forward(&x)?;
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

    /// TEST-ONLY: same forward but with the tokenizer's post-processor
    /// template applied (`<bos>` + text + `<eos>`) — the HF python default
    /// (`encode(text)` with add_special_tokens=True). Used by the probe to
    /// test whether the official pipeline encodes segments with special
    /// tokens (our production `sequence()` uses bare encoding).
    #[cfg(test)]
    fn decide_with_special(&self, req: &DecisionRequest) -> Result<DecisionOutput> {
        let enc = |t: &str| -> Result<Vec<u32>> {
            Ok(self
                .tokenizer
                .encode(t, true)
                .map_err(|e| anyhow!("tokenize: {e}"))?
                .get_ids()
                .to_vec())
        };
        let head_ids = enc(&format!("{} question: {}", req.decision_type.as_str(), req.question))?;
        let mut options: Vec<Vec<u32>> = req
            .options
            .iter()
            .map(|o| -> Result<Vec<u32>> {
                let mut v = vec![self.mask_id];
                v.extend(enc(&format!(" {o}"))?.into_iter().take(48));
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
        let state_ids = enc(&req.state)?;
        let room = self.max_context.saturating_sub(ids.len() + 1);
        if room < 1 {
            return Err(anyhow!("question/options exceed the sequence budget"));
        }
        ids.extend(state_ids.into_iter().take(room));
        ids.push(self.sep_id);
        self.forward_ids(&ids, &markers, req)
    }

    /// Shared forward over pre-built ids/markers (refactor of decide's body).
    #[cfg(test)]
    fn forward_ids(&self, ids: &[u32], markers: &[usize], req: &DecisionRequest) -> Result<DecisionOutput> {
        let seq = ids.len();
        let device = &self.device;
        let input = Tensor::new(ids, device)?.reshape((1, seq))?;
        let attn = Tensor::ones((1, seq), candle_core::DType::F32, device)?;
        let ids_flat = input.flatten_all()?;
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
        let out_ln = LayerNorm::new(
            self.output_norm_w.clone(),
            Tensor::zeros(self.hidden, candle_core::DType::F32, device)?,
            1e-5,
        );
        x = out_ln.forward(&x)?;
        let te = self.type_emb.narrow(0, req.decision_type.id() as usize, 1)?;
        x = x.broadcast_add(&te.reshape((1, 1, self.hidden))?)?;
        for blk in self.blocks.iter().skip(self.encoder_layers) {
            match blk {
                Block::Head(b) => x = self.forward_head(b, &x, &mask_bias)?,
                Block::Encoder(_) => {}
            }
        }
        let n_opt = markers.len();
        let mk_u32: Vec<u32> = markers.iter().map(|&m| m as u32).collect();
        let mk_idx = Tensor::new(&mk_u32[..], device)?.reshape((1, n_opt, 1))?;
        let mk = x.gather(&mk_idx.expand((1, n_opt, self.hidden))?.contiguous()?, 1)?;
        let s = LayerNorm::new(
            self.scorer_norm_w.clone(),
            self.scorer_norm_b.clone(),
            1e-5,
        )
        .forward(&mk)?;
        let s = linear(&s, &self.scorer_w)?;
        let s = s.broadcast_add(&self.scorer_b.reshape((1, 1, self.hidden))?)?;
        let s = gelu(&s)?;
        let s = linear(&s, &self.output_w)?;
        let s = s.broadcast_add(&self.output_b.reshape((1, 1, 1))?)?
            .squeeze(2)?
            .squeeze(0)?;
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
            .join("runtimes/decision/model/julia/1/model.gguf");
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

    /// Run every decision model (julia, laya, statim) across all three
    /// decision types and dump raw logits for comparison against the official
    /// references:
    ///   - julia:  official `JuliaDecisionModel` python (fp32) — values in
    ///             `julia_reference_probe`
    ///   - laya:   official laya python golden file
    ///             (statim repo tests/data/golden_laya-multilingual.jsonl)
    ///   - statim: official statim C++ engine (github.com/BEKO2210/statim)
    /// `cargo test --lib mv::decision::tests::all_decision_models_all_types_probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn all_decision_models_all_types_probe() {
        let device = Device::Cpu;
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("runtimes/decision/model");
        let models = [
            ("julia/1", "julia-1"),
            ("laya/multilingual", "laya-multilingual"),
            ("statim/multilingual-base", "statim"),
            ("jev-style/0.8b", "jev-style"),
        ];
        // (type, state, question, options) — covers choice / score / noul.
        let common = [
            (DecisionType::Choice, "对话进行中", "用户想退出程序", vec!["继续运行", "退出程序"]),
            (DecisionType::Choice, "对话进行中", "1+1", vec!["2", "3"]),
            (DecisionType::Noul, "对话进行中", "今天天气很好", vec!["false", "true"]),
            (DecisionType::Score, "对话进行中", "这段文字的情感倾向", vec!["负面", "正面"]),
        ];
        // Official laya golden inputs (state 0 of golden_laya-multilingual.jsonl).
        let golden = [
            (DecisionType::Choice, "I was charged twice for March, please refund the duplicate today or we cancel.", "Which department should handle this request?", vec!["billing: invoices, payments, refunds", "technical: bugs, outages, system errors", "sales: pricing, new contracts", "other: everything else"]),
            (DecisionType::Choice, "I was charged twice for March, please refund the duplicate today or we cancel.", "What is the sentiment of the message?", vec!["positive", "neutral", "negative"]),
            (DecisionType::Score, "I was charged twice for March, please refund the duplicate today or we cancel.", "How urgent is this request?", vec!["not urgent", "soon", "critical deadline or blocking issue"]),
            (DecisionType::Noul, "I was charged twice for March, please refund the duplicate today or we cancel.", "Does the user threaten to cancel or leave?", vec!["false", "true"]),
        ];
        for (rel, name) in models {
            let path = if rel.ends_with(".gguf") {
                base.join(rel)
            } else {
                base.join(rel).join("model.gguf")
            };
            if !path.is_file() {
                eprintln!("SKIP {name}: {} not present", path.display());
                continue;
            }
            let mut file = std::fs::File::open(&path).unwrap();
            let size_dir = path.parent().unwrap().to_path_buf();
            let m = DecisionModel::load(&mut file, &size_dir, &device, None)
                .unwrap_or_else(|e| panic!("load {name}: {e}"));
            eprintln!("=== {name} ({}) ===", path.display());
            // warm-up pass (first call includes lazy init), then timed runs
            for (t, st, q, opts) in common.iter().take(1) {
                let _ = m.decide(&DecisionRequest {
                    decision_type: *t,
                    state: st.to_string(),
                    question: q.to_string(),
                    options: opts.iter().map(|s| s.to_string()).collect(),
                });
            }
            for (t, st, q, opts) in common.iter().chain(golden.iter()) {
                let req = DecisionRequest {
                    decision_type: *t,
                    state: st.to_string(),
                    question: q.to_string(),
                    options: opts.iter().map(|s| s.to_string()).collect(),
                };
                // median of 5 runs
                let mut times = Vec::new();
                let mut out = None;
                for _ in 0..5 {
                    let t0 = std::time::Instant::now();
                    let o = m
                        .decide(&req)
                        .unwrap_or_else(|e| panic!("{name} decide {q}: {e}"));
                    times.push(t0.elapsed().as_secs_f64() * 1000.0);
                    out = Some(o);
                }
                times.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let out = out.unwrap();
                eprintln!(
                    "{:?} state={:?} q={:?} opts={:?}\n  scores={:?}\n  probs={:?} idx={} opt={}  latency median={:.1}ms (min {:.1} max {:.1})",
                    out.decision_type,
                    st,
                    q,
                    opts,
                    out.scores.iter().map(|v| format!("{v:.4}")).collect::<Vec<_>>(),
                    out.probabilities.iter().map(|v| format!("{v:.4}")).collect::<Vec<_>>(),
                    out.index,
                    out.option,
                    times[2], times[0], times[times.len() - 1]
                );
            }
        }
    }

    /// Dump the scorer-head tensor shapes as candle sees them (GGUF dims are
    /// stored reversed; this shows the LOGICAL shapes the loader works with).

    /// `cargo test --lib mv::decision::tests::julia_shape_dump -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn julia_shape_dump() {
        let gguf = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("runtimes/decision/model/julia/1/model.gguf");
        if !gguf.is_file() {
            eprintln!("SKIP: julia_1 model.gguf not present");
            return;
        }
        let mut f = std::fs::File::open(&gguf).unwrap();
        let mut content = Content::read(&mut f).unwrap();
        let md = &content.metadata;
        for key in [
            "modern-bert.rope.freq_base",
            "modern-bert.rope.freq_base_swa",
            "modern-bert.norm_eps",
            "modern-bert.hidden_activation",
        ] {
            match md.get(key) {
                Some(v) => eprintln!("  {key}: {:?}", v),
                None => eprintln!("  {key}: <absent>"),
            }
        }
        for name in [
            "cls.output.weight",
            "cls.output.bias",
            "cls.weight",
            "cls.bias",
            "cls.norm.weight",
            "token_types.weight",
        ] {
            match content.tensor(&mut f, name, &Device::Cpu) {
                Ok(t) => {
                    let d = t.dequantize(&Device::Cpu).expect("deq");
                    eprintln!("  {name}: rank {} dims {:?}", d.rank(), d.dims());
                }
                Err(e) => eprintln!("  {name}: ERR {e}"),
            }
        }
    }

    /// Interactive demo of the Julia-1 decision model's capability boundary.
    /// One command:
    /// `cargo test --lib mv::decision::tests::julia_demo_probe -- --ignored --nocapture`
    ///
    /// Section A proves the serving pipeline is lossless (tokenizer round
    /// trip + dual-tokenizer contrast). Section B runs a labeled case matrix:
    /// IN-DOMAIN cases (tool routing / option picking / relevance scoring —
    /// what the model was trained for) must match their target; OUT-OF-DOMAIN
    /// cases (math, causal weather inference, factual knowledge) print the
    /// raw scores to show they are noise — do not trust the model there.


    #[test]
    #[ignore]
    fn julia_demo_probe() {
        let src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("runtimes/decision/model/julia/1");
        let gguf = src.join("model.gguf");
        if !gguf.is_file() {
            eprintln!("SKIP: julia_1 model.gguf not present");
            return;
        }
        let mut f = std::fs::File::open(&gguf).unwrap();
        let m = DecisionModel::load(&mut f, &src, &Device::Cpu, None).expect("load");
        eprintln!("\n=== Julia-1 decision demo (bundled tokenizer.json path) ===\n");

        // ── Section A: pipeline losslessness ────────────────────────────
        eprintln!("── A. Tokenizer round trip (must decode back exactly) ──");
        for text in ["1+1等于几", "今天天气怎么样", "translate this document"] {
            let enc = m.tokenizer.encode(text, false).expect("encode");
            let ids: Vec<u32> = enc.get_ids().to_vec();
            let dec = m.tokenizer.decode(&ids, false).map(|d| d.to_string()).unwrap_or_default();
            eprintln!("  {text:?} -> decode {dec:?} {}", if dec.trim() == text.trim() { "✅" } else { "❌" });
        }
        {
            let mut f2 = std::fs::File::open(&gguf).unwrap();
            let content = Content::read(&mut f2).unwrap();
            let family = content
                .metadata
                .get("tokenizer.ggml.model")
                .and_then(|v| v.to_string().ok());
            eprintln!(
                "  GGUF-embedded tokenizer family = {family:?} (wrong vocab — the shipped tokenizer.json is REQUIRED)\n"
            );
        }

        // ── Section B: labeled case matrix ──────────────────────────────
        eprintln!("── B. Case matrix (✅ expected pick, ❌ out-of-domain noise) ──");
        let case = |t: DecisionType, state: &str, q: &str, opts: &[&str], expect: Option<usize>, tag: &str| {
            let r = DecisionRequest {
                decision_type: t,
                state: state.to_string(),
                question: q.to_string(),
                options: opts.iter().map(|s| s.to_string()).collect(),
            };
            let o = m.decide(&r).expect("decide");
            let verdict = match expect {
                Some(i) if o.index == i => "✅",
                Some(_) => "❌",
                None => "⚠️ (OOD — noise expected)",
            };
            eprintln!(
                "  [{tag}] {verdict} scores [{}] probs [{}] -> {}",
                o.scores.iter().map(|v| format!("{v:.3}")).collect::<Vec<_>>().join(", "),
                o.probabilities.iter().map(|v| format!("{v:.3}")).collect::<Vec<_>>().join(", "),
                o.option
            );
            (tag.to_string(), expect, o.index)
        };
        // Special-tokens variant: does applying the tokenizer template
        // (<bos>+text+<eos>, the HF python default) change the verdict?
        eprintln!("── B0. Special-tokens encoding variant (HF python default) ──");
        let sp_math = DecisionRequest {
            decision_type: DecisionType::Score,
            state: "在义务教育的数学场景下".into(),
            question: "1+1等于几".into(),
            options: vec!["4".into(), "3".into(), "2".into()],
        };
        let sp_weather = DecisionRequest {
            decision_type: DecisionType::Score,
            state: "今天是11月16号，今天的白天温度是20~30度，天气预报说天气很高".into(),
            question: "请问今天10~13点有可能下雨么".into(),
            options: vec!["晴天".into(), "下雨".into()],
        };
        for (tag, r) in [("[sp·math]", &sp_math), ("[sp·weather]", &sp_weather)] {
            let o = m.decide_with_special(r).expect("decide sp");
            eprintln!(
                "  {tag:<14} scores [{}] probs [{}] -> {}",
                o.scores.iter().map(|v| format!("{v:.3}")).collect::<Vec<_>>().join(", "),
                o.probabilities.iter().map(|v| format!("{v:.3}")).collect::<Vec<_>>().join(", "),
                o.option
            );
        }

        let mut results = Vec::new();
        // IN-DOMAIN: tool routing (choice) — officially anchored cases only
        // (verified against the official fp32 reference; self-authored
        // scenarios stay informational below and are not asserted).
        results.push(case(DecisionType::Choice, "user asks in Chinese how to check their public IP; tools available: getPublicIp (returns public IP), listFiles (lists local files)", "Which tool should be used?", &["getPublicIp: returns the machine public IP address", "listFiles: lists files in a local directory"], Some(0), "choice·route"));
        // Informational (self-authored, no official anchor): the model picks
        // the single-file tool here — plausible scoring, not an assertion.
        results.push(case(DecisionType::Choice, "user wants to rename hundreds of files in one folder at once; tools: rename_files (batch rename), rename_file (rename a single file)", "Which tool should be used?", &["rename_files: batch rename files in a directory", "rename_file: rename one file at a time"], None, "choice·route*"));
        // IN-DOMAIN: tool-or-chat (noul)
        results.push(case(DecisionType::Noul, "user message: \"帮我查一下公网ip\"", "Does this message require a tool call?", &["false", "true"], Some(1), "noul·route"));
        // IN-DOMAIN: relevance scoring (score)
        results.push(case(DecisionType::Score, "candidate tools for the query \"translate this document to English\"", "Rate tool relevance", &["translate_text: translates text between languages", "get_weather: returns weather forecast"], Some(0), "score·relevance"));
        results.push(case(DecisionType::Score, "candidate tools for the query \"will it rain tomorrow in Beijing\"", "Rate tool relevance", &["get_weather: returns weather forecast for a city", "translate_text: translates text between languages"], Some(0), "score·relevance"));
        // OUT-OF-DOMAIN: arithmetic / causal inference / factual knowledge
        results.push(case(DecisionType::Score, "在义务教育的数学场景下", "1+1等于几", &["4", "3", "2"], None, "score·math"));
        results.push(case(DecisionType::Score, "今天是11月16号，今天的白天温度是20~30度，天气预报说天气很高", "请问今天10~13点有可能下雨么", &["晴天", "下雨"], None, "score·weather"));
        results.push(case(DecisionType::Choice, "general knowledge quiz", "What is the capital of France?", &["Paris", "London"], None, "choice·knowledge"));

        let in_domain: Vec<_> = results.iter().filter(|(_, e, _)| e.is_some()).collect();
        let passed = in_domain.iter().filter(|(_, e, got)| Some(*got) == *e).count();
        eprintln!(
            "\nSummary: in-domain {passed}/{} correct · out-of-domain rows are model noise by design\n",
            in_domain.len()
        );
        for (tag, e, got) in &in_domain {
            assert_eq!(*e, Some(*got), "in-domain case [{tag}] must match its target");
        }
    }
}


#[cfg(test)]
mod suite_tests {
    use super::*;
    use std::path::PathBuf;

    /// Full interface suite: 61 cases (choice/score/noul across routing,
    /// intent, sentiment, urgency, severity, fact-check, moderation, long
    /// context, mixed-language, edge inputs) run through the PUBLIC
    /// `DecisionModel::decide` API for every decision model, with a
    /// cross-model agreement summary.
    ///
    /// `cargo test --release --lib mv::decision::suite_tests::all_models_61cases_suite -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn all_models_61cases_suite() {
        let case_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("runtimes/decision/testdata/decision_suite.json");
        let raw = std::fs::read_to_string(&case_path).unwrap();
        #[derive(serde::Deserialize)]
        struct Case {
            id: String,
            #[serde(rename = "type")]
            typ: String,
            state: String,
            question: String,
            options: Vec<String>,
        }
        let cases: Vec<Case> = serde_json::from_str(&raw).unwrap();
        assert!(cases.len() >= 50, "suite must have 50+ cases, got {}", cases.len());

        let device = Device::Cpu;
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtimes/decision/model");
        let models = [
            ("julia/1", "julia"),
            ("laya/multilingual", "laya"),
            ("statim/multilingual-base", "statim"),
            ("jev-style/0.8b", "jev-q8"),
        ];
        let mut results: Vec<(String, Vec<Option<(usize, f32, f64)>>)> = Vec::new();
        let mut json_out: std::collections::HashMap<String, Vec<serde_json::Value>> = std::collections::HashMap::new();
        for (rel, name) in models {
            let path = if rel.ends_with(".gguf") { base.join(rel) } else { base.join(rel).join("model.gguf") };
            let mut file = std::fs::File::open(&path).unwrap();
            let size_dir = path.parent().unwrap().to_path_buf();
            let m = DecisionModel::load(&mut file, &size_dir, &device, None)
                .unwrap_or_else(|e| panic!("load {name}: {e}"));
            eprintln!("=== {name} ===");
            let mut per_model = Vec::new();
            for c in &cases {
                let req = DecisionRequest {
                    decision_type: match c.typ.as_str() {
                        "choice" => DecisionType::Choice,
                        "score" => DecisionType::Score,
                        _ => DecisionType::Noul,
                    },
                    state: c.state.clone(),
                    question: c.question.clone(),
                    options: c.options.clone(),
                };
                let t0 = std::time::Instant::now();
                let out = m
                    .decide(&req)
                    .unwrap_or_else(|e| panic!("{name} {} decide: {e}", c.id));
                let ms = t0.elapsed().as_secs_f64() * 1000.0;
                per_model.push(Some((out.index, out.probabilities[out.index], ms)));
                json_out.entry(name.to_string()).or_insert_with(Vec::new).push(serde_json::json!({
                    "id": c.id, "index": out.index, "option": out.option,
                    "prob": out.probabilities[out.index], "probabilities": out.probabilities, "scores": out.scores, "ms": ms
                }));
                eprintln!(
                    "{} {:?} q={:?} -> {} (p={:.4}, {:.0}ms)",
                    c.id, out.decision_type, c.question, out.option, out.probabilities[out.index], ms
                );
            }
            results.push((name.to_string(), per_model));
        }

        // cross-model agreement summary
        eprintln!("\n===== CROSS-MODEL AGREEMENT (61 cases) =====");
        let n = cases.len();
        for i in 0..results.len() {
            for j in i + 1..results.len() {
                let agree = results[i]
                    .1
                    .iter()
                    .zip(results[j].1.iter())
                    .filter(|(a, b)| a.map(|x| x.0) == b.map(|x| x.0))
                    .count();
                eprintln!("{} vs {}: {}/{} same argmax", results[i].0, results[j].0, agree, n);
            }
        }
        // per-type agreement of all 4 models
        for typ in ["choice", "score", "noul"] {
            let same = cases
                .iter()
                .enumerate()
                .filter(|(_, c)| c.typ == typ)
                .filter(|(i, _)| {
                    let first = results[0].1[*i].map(|x| x.0);
                    results.iter().all(|(_, r)| r[*i].map(|x| x.0) == first)
                })
                .count();
            let total = cases.iter().filter(|c| c.typ == typ).count();
            eprintln!("all-4-agree on {typ}: {same}/{total}");
        }
        let lat: Vec<String> = results
            .iter()
            .map(|(n, r)| {
                let mut ts: Vec<f64> = r.iter().filter_map(|x| x.map(|y| y.2)).collect();
                ts.sort_by(|a, b| a.partial_cmp(b).unwrap());
                format!("{n} median {:.0}ms", ts[ts.len() / 2])
            })
            .collect();
        eprintln!("latency: {}", lat.join(", "));
        let out_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("runtimes/decision/testdata/suite_results.json");
        std::fs::write(&out_path, serde_json::to_string_pretty(&json_out).unwrap()).unwrap();
        eprintln!("results written to {}", out_path.display());
    }

    /// Cold model-load timing (disk read + dequantize + tokenizer build).
    /// `cargo test --release --lib mv::decision::suite_tests::load_time_probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn load_time_probe() {
        let device = Device::Cpu;
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtimes/decision/model");
        let models = [
            ("julia/1", "julia"),
            ("laya/multilingual", "laya"),
            ("statim/multilingual-base", "statim"),
            ("jev-style/0.8b", "jev-q8"),
        ];
        for (rel, name) in models {
            let path = if rel.ends_with(".gguf") { base.join(rel) } else { base.join(rel).join("model.gguf") };
            let t0 = std::time::Instant::now();
            let mut file = std::fs::File::open(&path).unwrap();
            let size_dir = path.parent().unwrap().to_path_buf();
            let m = DecisionModel::load(&mut file, &size_dir, &device, None).unwrap();
            let load_ms = t0.elapsed().as_secs_f64() * 1000.0;
            // first decide (includes any lazy init)
            let req = DecisionRequest {
                decision_type: DecisionType::Choice,
                state: "s".into(),
                question: "q".into(),
                options: vec!["a".into(), "b".into()],
            };
            let t1 = std::time::Instant::now();
            let _ = m.decide(&req);
            let first_ms = t1.elapsed().as_secs_f64() * 1000.0;
            eprintln!("{name:8} load={load_ms:7.0}ms  first-decide={first_ms:7.0}ms  size={:.0}MB",
                std::fs::metadata(&path).unwrap().len() as f64 / 1048576.0);
        }
    }
}

#[cfg(test)]
mod resource_tests {
    use super::*;
    use std::path::PathBuf;

    /// Resource footprint probe: loads ONE model (selected by env
    /// RESOURCE_MODEL = julia|laya|statim|jev-q8|jev-q4), then runs 10
    /// decide calls. Pair with `/usr/bin/time -l` for max-RSS.
    /// `RESOURCE_MODEL=julia cargo test --release --lib mv::decision::resource_tests::resource_probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn resource_probe() {
        let which = std::env::var("RESOURCE_MODEL").unwrap_or_else(|_| "julia".into());
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtimes/decision/model");
        let rel = match which.as_str() {
            "julia" => "julia/1",
            "laya" => "laya/multilingual",
            "statim" => "temp/statim-decide-multilingual-base-q8_0.gguf",
            "jev-q8" => "temp/Jev-Style-0.8B-Decision-v3-Q8_0.gguf",
            "jev-q4" => "temp/Jev-Style-0.8B-Decision-v3-Q4_K_M.gguf",
            other => panic!("unknown RESOURCE_MODEL {other}"),
        };
        let path = if rel.ends_with(".gguf") { base.join(rel) } else { base.join(rel).join("model.gguf") };
        eprintln!("PHASE baseline (pid={})", std::process::id());
        std::thread::sleep(std::time::Duration::from_millis(500));
        let t0 = std::time::Instant::now();
        let mut file = std::fs::File::open(&path).unwrap();
        let size_dir = path.parent().unwrap().to_path_buf();
        let m = DecisionModel::load(&mut file, &size_dir, &Device::Cpu, None).unwrap();
        eprintln!("PHASE loaded {} in {:.0}ms", which, t0.elapsed().as_secs_f64() * 1000.0);
        std::thread::sleep(std::time::Duration::from_millis(500));
        let req = DecisionRequest {
            decision_type: DecisionType::Choice,
            state: "I was charged twice for March, please refund the duplicate today or we cancel.".into(),
            question: "Which department should handle this request?".into(),
            options: vec![
                "billing: invoices, payments, refunds".into(),
                "technical: bugs, outages, system errors".into(),
                "sales: pricing, new contracts".into(),
                "other: everything else".into(),
            ],
        };
        let mut times = Vec::new();
        for i in 0..10 {
            let t = std::time::Instant::now();
            let _ = m.decide(&req).unwrap();
            times.push(t.elapsed().as_secs_f64() * 1000.0);
            if i == 0 {
                eprintln!("PHASE first-decide-done");
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        eprintln!(
            "PHASE done: decide median={:.0}ms min={:.0}ms max={:.0}ms",
            times[5], times[0], times[9]
        );
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}
