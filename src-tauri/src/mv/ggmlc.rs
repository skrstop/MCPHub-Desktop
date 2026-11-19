//! mv::ggmlc — interpreter for the `ggmlc` fused-graph GGUF format.
//!
//! The laya decision checkpoints (`convaiinnovations/laya-multilingual`, Q8_0)
//! are converted with the `ggmlc` converter: instead of the `modern-bert.*`
//! metadata + `blk.N` tensor naming the Julia GGUFs use, the file ships a
//! `ggmlc.graph_spec` JSON describing a fully-fused computation graph
//! (parameters, constants, inputs, and ~1100 op nodes). Executing that graph
//! faithfully reproduces the official `laya` python forward
//! (`laya/common.py::DecisionModel.forward`), including the folded LayerNorm
//! affines, the precomputed RoPE tables, the sliding-window bias and the
//! act/escalate head.
//!
//! Conventions (validated against the official `laya` package on the HF
//! safetensors — choice/noul probabilities match to ~1e-4, score within Q8
//! quantization error):
//! - `ne` arrays in the spec are ggml-order; torch shape = reversed. Parameters
//!   are pre-transposed to `[in, out]` / `[dim, vocab]` / `[head_dim, max_pos]`.
//! - `slice`/`getitem` address dims via `ggml_dim` (0 = fastest/torch-last);
//!   `end: -1` means "trim to the dynamic sequence length" when `start == 0`
//!   and the axis is the `max_len + 1` RoPE/bias table axis, else "open end".
//! - `unsqueeze`/`view`/`reshape`/`cat`/`sum` use torch dims.
//! - `transpose` nodes are markers for the fused SDPA (`fused_transpose: 1`);
//!   activations stay in torch `[B, S, H, D]` layout and the SDPA op moves the
//!   head axis internally.
//! - `layer_norm` with a single input is a pure normalize — the affine is
//!   folded into the following linear's weights (see the `*_baked_bias`
//!   tensors); with a weight input it applies it after normalizing.
//! - opcode 200 is `gelu(x)` on an already-biased linear output.

use super::gguf::{build_gguf_tokenizer, meta_u32};
use crate::mv::decision::{DecisionOutput, DecisionRequest, DecisionType};
use anyhow::{anyhow, bail, Result};
use candle_core::{DType, Device, Tensor};
use std::collections::HashMap;
use tokenizers::Tokenizer;

/// Resolved runtime shape entry from the graph spec.
#[derive(Debug, Clone)]
enum ShapeEntry {
    Static(i64),
    Symbol(String),
    Mul(Box<ShapeEntry>, Box<ShapeEntry>),
}

#[derive(Debug, Clone)]
struct Node {
    id: u32,
    opcode: u32,
    name: String,
    inputs: Vec<u32>,
    outputs: Vec<u32>,
    eps: f64,
    dim: i64,
    ggml_dim: i64,
    start: i64,
    end: i64,
    keepdim: bool,
    min: Option<f64>,
    max: Option<f64>,
    scale: f64,
    unary_op: i64,
    is_select: bool,
    shape: Option<Vec<ShapeEntry>>,
}

impl Node {
    fn from_json(v: &serde_json::Value) -> Result<Self> {
        let a = v.get("attributes").cloned().unwrap_or(serde_json::json!({}));
        let get_i = |k: &str| a.get(k).and_then(|x| x.as_i64()).unwrap_or(0);
        Ok(Node {
            id: v.get("id").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
            opcode: v
                .get("opcode")
                .and_then(|x| x.as_u64())
                .ok_or_else(|| anyhow!("node missing opcode"))? as u32,
            name: v
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            inputs: v
                .get("inputs")
                .and_then(|x| x.as_array())
                .map(|xs| xs.iter().filter_map(|x| x.as_u64()).map(|x| x as u32).collect())
                .unwrap_or_default(),
            outputs: v
                .get("outputs")
                .and_then(|x| x.as_array())
                .map(|xs| xs.iter().filter_map(|x| x.as_u64()).map(|x| x as u32).collect())
                .unwrap_or_default(),
            eps: a.get("eps").and_then(|x| x.as_f64()).unwrap_or(1e-5),
            dim: get_i("dim"),
            ggml_dim: get_i("ggml_dim"),
            start: get_i("start"),
            end: get_i("end"),
            keepdim: a.get("keepdim").and_then(|x| x.as_i64()).unwrap_or(0) != 0,
            min: a.get("min").and_then(|x| x.as_f64()),
            max: a.get("max").and_then(|x| x.as_f64()),
            scale: a.get("scale").and_then(|x| x.as_f64()).unwrap_or(1.0),
            unary_op: get_i("unary_op"),
            is_select: a.get("is_select").and_then(|x| x.as_i64()).unwrap_or(0) != 0,
            shape: a.get("shape").map(parse_shape).transpose()?,
        })
    }
}

fn parse_shape(v: &serde_json::Value) -> Result<Vec<ShapeEntry>> {
    // A bare entry (object/number/string) is a single-dim shape.
    if !v.is_array() {
        return parse_shape(&serde_json::Value::Array(vec![v.clone()]));
    }
    let arr = v.as_array().unwrap();
    arr.iter()
        .map(|e| {
            if let Some(s) = e.as_str() {
                // Symbolic dims arrive as plain strings ("s22", "-1").
                return Ok(ShapeEntry::Symbol(s.to_string()));
            }
            if let Some(i) = e.as_i64() {
                return Ok(ShapeEntry::Static(i));
            }
            let obj = e.as_object().ok_or_else(|| anyhow!("bad shape entry"))?;
            match obj.get("type").and_then(|t| t.as_str()) {
                Some("static") => Ok(ShapeEntry::Static(
                    obj.get("val")
                        .and_then(|x| x.as_i64())
                        .ok_or_else(|| anyhow!("static missing val"))?,
                )),
                Some("symbol") => Ok(ShapeEntry::Symbol(
                    obj.get("name")
                        .and_then(|x| x.as_str())
                        .ok_or_else(|| anyhow!("symbol missing name"))?
                        .to_string(),
                )),
                Some("mul") => Ok(ShapeEntry::Mul(
                    Box::new(
                        parse_shape(obj.get("left").ok_or_else(|| anyhow!("mul missing left"))?)?
                            .into_iter()
                            .next()
                            .ok_or_else(|| anyhow!("empty left"))?,
                    ),
                    Box::new(
                        parse_shape(obj.get("right").ok_or_else(|| anyhow!("mul missing right"))?)?
                            .into_iter()
                            .next()
                            .ok_or_else(|| anyhow!("empty right"))?,
                    ),
                )),
                other => bail!("unsupported shape entry type {other:?}"),
            }
        })
        .collect()
}

/// Input tensor ids from the graph spec (fixed by the converter).
const INPUT_IDS: u32 = 175;
const ATTN_MASK: u32 = 176;
const MARKER_POS: u32 = 177;
const MARKER_MASK: u32 = 178;
const QTYPE: u32 = 179;

/// One loaded ggmlc decision model (graph + tokenizer + sequence contract).
pub struct GgmlcModel {
    pub tokenizer: Tokenizer,
    params: HashMap<u32, Tensor>,
    nodes: Vec<Node>,
    /// Scores output tensor id (`add_95`: masked logits, `[max_opts, B]`).
    scores_out: u32,
    /// Act-head output tensor id (`linear_99`, `[n_act, B]`) — parsed for
    /// completeness; the decide path currently uses the scores only.
    #[allow(dead_code)]
    act_out: u32,
    cls_id: u32,
    sep_id: u32,
    mask_id: u32,
    max_len: usize,
    head_max_len: usize,
    max_opts: usize,
    device: Device,
}

impl GgmlcModel {
    /// Load from a ggmlc GGUF.
    pub fn load<R: std::io::Read + std::io::Seek>(file: &mut R, device: &Device) -> Result<Self> {
        let content = candle_core::quantized::gguf_file::Content::read(file)
            .map_err(|e| anyhow!("read gguf: {e}"))?;
        let md = &content.metadata;
        let spec_str = md
            .get("ggmlc.graph_spec")
            .and_then(|v| match v {
                candle_core::quantized::gguf_file::Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .ok_or_else(|| anyhow!("missing ggmlc.graph_spec metadata"))?;
        let spec: serde_json::Value =
            serde_json::from_str(&spec_str).map_err(|e| anyhow!("parse graph_spec: {e}"))?;

        // id -> name for every stored tensor (storage 1 = weight, 2 = constant).
        let mut id_to_name: HashMap<u32, String> = HashMap::new();
        if let Some(tensors) = spec.get("tensors").and_then(|t| t.as_object()) {
            for (tid, t) in tensors {
                if let (Some(name), Some(storage)) = (
                    t.get("name").and_then(|n| n.as_str()),
                    t.get("storage").and_then(|s| s.as_i64()),
                ) {
                    if storage == 1 || storage == 2 {
                        id_to_name.insert(tid.parse::<u32>()?, name.to_string());
                    }
                }
            }
        }

        // Load + dequantize every stored tensor. candle returns torch-logical
        // shapes (gguf dims reversed on disk): [out, in] linears, [vocab, dim]
        // embeddings, [max_pos, head_dim] RoPE tables — squeeze the trailing
        // 1-axes candle keeps from the 4-D ggml ne and use as-is.
        let mut params: HashMap<u32, Tensor> = HashMap::new();
        for (tid, name) in id_to_name.iter() {
            let t = content
                .tensor(file, name, device)
                .map_err(|e| anyhow!("tensor {name}: {e}"))?
                .dequantize(device)
                .map_err(|e| anyhow!("dequantize {name}: {e}"))?;
            let t = if t.rank() > 2 {
                // Strip only the LEADING 1-axes (ggml's trailing ne[.] == 1);
                // interior/unit dims (e.g. [1, 768] out-in linears) stay.
                let mut dims = t.dims().to_vec();
                while dims.len() > 2 && dims[0] == 1 {
                    dims.remove(0);
                }
                t.reshape(dims.as_slice())?
            } else {
                t
            };
            params.insert(*tid, t.contiguous()?);
        }

        let mut nodes: Vec<Node> = spec
            .get("nodes")
            .and_then(|n| n.as_array())
            .ok_or_else(|| anyhow!("graph_spec missing nodes"))?
            .iter()
            .map(Node::from_json)
            .collect::<Result<Vec<_>>>()?;
        // Execution order = node id (the converter emits a topological order).
        nodes.sort_by_key(|n| n.id);

        let outputs = spec
            .get("outputs")
            .and_then(|o| o.as_array())
            .ok_or_else(|| anyhow!("graph_spec missing outputs"))?;
        if outputs.len() < 2 {
            bail!(
                "expected 2 graph outputs (scores, act), got {}",
                outputs.len()
            );
        }
        let scores_out = outputs[0].as_u64().ok_or_else(|| anyhow!("bad output id"))? as u32;
        let act_out = outputs[1].as_u64().ok_or_else(|| anyhow!("bad output id"))? as u32;

        // laya.* metadata carries the sequence contract and special-token ids.
        let get_usize = |key: &str, default: usize| -> usize {
            meta_u32(md, key).map(|v| v as usize).unwrap_or(default)
        };
        let max_len = get_usize("laya.max_len", 1024);
        let head_max_len = get_usize("laya.head_max_len", 256);
        let max_opts = get_usize("laya.max_opts", 16);
        let cls_id = meta_u32(md, "laya.cls_token_id").unwrap_or(2);
        let sep_id = meta_u32(md, "laya.sep_token_id").unwrap_or(1);
        let mask_id = meta_u32(md, "laya.mask_token_id").unwrap_or(4);

        let tokenizer = build_gguf_tokenizer(md)?;

        Ok(GgmlcModel {
            tokenizer,
            params,
            nodes,
            scores_out,
            act_out,
            cls_id,
            sep_id,
            mask_id,
            max_len,
            head_max_len,
            max_opts,
            device: device.clone(),
        })
    }

    /// Official `build_sequence` (sequential option layout): `[CLS] <type>
    /// instructions [SEP] [MASK] opt0 [MASK] opt1 ... [SEP] state [SEP]`.
    /// Score options are rendered `level N: text`; noul uses the fixed
    /// false/true pair (the request's options stay the bare labels).
    fn sequence(&self, req: &DecisionRequest) -> Result<(Vec<u32>, Vec<usize>)> {
        if req.options.len() < 2 || req.options.len() > self.max_opts {
            bail!(
                "options must contain 2-{} entries (got {})",
                self.max_opts,
                req.options.len()
            );
        }
        if req.options.iter().any(|o| o.is_empty()) {
            bail!("options must be non-empty");
        }
        const MAX_FIELD_CHARS: usize = 64 * 1024;
        const MAX_TOTAL_CHARS: usize = 128 * 1024;
        if req.question.chars().count() > MAX_FIELD_CHARS
            || req.state.chars().count() > MAX_FIELD_CHARS
            || req.options.iter().any(|o| o.chars().count() > MAX_FIELD_CHARS)
        {
            bail!("question/state/options exceed the {MAX_FIELD_CHARS}-character limit");
        }
        let total_chars: usize = req.question.chars().count()
            + req.state.chars().count()
            + req.options.iter().map(|o| o.chars().count()).sum::<usize>();
        if total_chars > MAX_TOTAL_CHARS {
            bail!("question+state+options exceed the {MAX_TOTAL_CHARS}-character total limit");
        }

        let mask_str = self
            .tokenizer
            .id_to_token(self.mask_id)
            .ok_or_else(|| anyhow!("tokenizer has no token for mask id {}", self.mask_id))?
            .to_string();
        let clean = |t: &str| t.replace(&mask_str, " ");
        let enc = |t: &str| -> Result<Vec<u32>> {
            Ok(self
                .tokenizer
                .encode(t, false)
                .map_err(|e| anyhow!("tokenize: {e}"))?
                .get_ids()
                .to_vec())
        };
        // Official `render_options`: score questions get a `level N: ` prefix;
        // noul renders the fixed semantic pair; choice passes through.
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
            DecisionType::Choice => req.options.clone(),
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
            let per_option = std::cmp::max(4, self.head_max_len.saturating_sub(16) / options.len());
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
        let state_ids = enc(&clean(&req.state))?;
        let room = self.max_len.saturating_sub(ids.len() + 1);
        if room < 1 {
            bail!("question/options exceed the sequence budget");
        }
        ids.extend(state_ids.into_iter().take(room));
        ids.push(self.sep_id);
        Ok((ids, markers))
    }

    /// Run the graph and return the structured decision.
    pub fn decide(&self, req: &DecisionRequest) -> Result<DecisionOutput> {
        let (ids, markers) = self.sequence(req)?;
        let seq = ids.len();
        let device = &self.device;
        let n_opts = self.max_opts;

        let input_ids = Tensor::new(ids.as_slice(), device)?.reshape((1, seq))?;
        let attention_mask = Tensor::ones((1, seq), DType::F32, device)?;
        let mut pos = vec![0u32; n_opts];
        let mut mask = vec![0.0f32; n_opts];
        for (i, &m) in markers.iter().enumerate() {
            pos[i] = m as u32;
            mask[i] = 1.0;
        }
        let marker_pos = Tensor::new(pos.as_slice(), device)?.reshape((1, n_opts))?;
        let marker_mask = Tensor::new(mask.as_slice(), device)?.reshape((1, n_opts))?;
        let qtype = Tensor::new(&[req.decision_type.id()], device)?.reshape((1,))?;

        let mut env: HashMap<u32, Tensor> = HashMap::new();
        env.insert(INPUT_IDS, input_ids);
        env.insert(ATTN_MASK, attention_mask);
        env.insert(MARKER_POS, marker_pos);
        env.insert(MARKER_MASK, marker_mask);
        env.insert(QTYPE, qtype);

        let out = self.execute(&mut env, seq)?;

        // scores_out: [max_opts, B] torch layout -> take batch 0 -> [max_opts].
        let scores_t = out.reshape((n_opts, 1))?.t()?.squeeze(0)?;
        let scores: Vec<f32> = scores_t.to_vec1()?;
        let valid = &scores[..markers.len()];
        let max = valid.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let exps: Vec<f32> = valid.iter().map(|v| (v - max).exp()).collect();
        let sum: f32 = exps.iter().sum();
        let probabilities: Vec<f32> = exps.iter().map(|e| e / sum).collect();
        let index = valid
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i)
            .unwrap_or(0);
        Ok(DecisionOutput {
            decision_type: req.decision_type,
            scores: valid.to_vec(),
            probabilities,
            index,
            option: req.options[index].clone(),
        })
    }

    fn execute(&self, env: &mut HashMap<u32, Tensor>, seq: usize) -> Result<Tensor> {
        let dyn_shapes: HashMap<&str, usize> = HashMap::from([("s22", 1usize), ("s53", seq)]);
        let mut v: HashMap<u32, Tensor> = env.iter().map(|(k, t)| (*k, t.clone())).collect();
        for node in &self.nodes {
            let mut ins: Vec<Tensor> = Vec::with_capacity(node.inputs.len());
            for &i in &node.inputs {
                if let Some(t) = v.get(&i) {
                    ins.push(t.clone());
                } else if let Some(p) = self.params.get(&i) {
                    ins.push(p.clone());
                } else {
                    bail!("node {} ({}): missing input {i}", node.name, node.opcode);
                }
            }
            let out = self
                .run_node(node, &ins, seq, &dyn_shapes)
                .map_err(|e| anyhow!("node {} (op {}): {e}", node.name, node.opcode))?;
            for &o in &node.outputs {
                v.insert(o, out.clone());
            }
        }
        v.get(&self.scores_out)
            .cloned()
            .ok_or_else(|| anyhow!("graph produced no scores output"))
    }

    fn resolve_shape(
        &self,
        entries: &[ShapeEntry],
        dyn_shapes: &HashMap<&str, usize>,
    ) -> Result<Vec<usize>> {
        entries
            .iter()
            .map(|e| {
                Ok(match e {
                    ShapeEntry::Static(i) => *i as usize,
                    ShapeEntry::Symbol(s) => {
                        if let Some(n) = dyn_shapes.get(s.as_str()) {
                            *n
                        } else if s == "-1" {
                            usize::MAX // inferred from the element count
                        } else {
                            bail!("unknown shape symbol {s}");
                        }
                    }
                    ShapeEntry::Mul(a, b) => {
                        let av = self.resolve_shape(std::slice::from_ref(a), dyn_shapes)?[0];
                        let bv = self.resolve_shape(std::slice::from_ref(b), dyn_shapes)?[0];
                        av * bv
                    }
                })
            })
            .collect()
    }

    fn run_node(
        &self,
        n: &Node,
        x: &[Tensor],
        seq: usize,
        dyn_shapes: &HashMap<&str, usize>,
    ) -> Result<Tensor> {
        match n.opcode {
            40 => {
                // embedding: table [N, D] (torch order), ids -> [.., D]
                let (table, ids) = (&x[0], &x[1]);
                let flat = ids.flatten_all()?.to_dtype(DType::U32)?;
                let rows = table.index_select(&flat, 0)?;
                let mut shape = ids.dims().to_vec();
                shape.push(table.dim(1)?);
                Ok(rows.reshape(shape.as_slice())?)
            }
            201 => {
                // layer_norm: pure normalize; optional weight (the affine is
                // folded into the following linear when absent).
                let mean = x[0].mean_keepdim(candle_core::D::Minus1)?;
                let centered = x[0].broadcast_sub(&mean)?;
                let var = centered.sqr()?.mean_keepdim(candle_core::D::Minus1)?;
                let normed = centered.broadcast_div(&(var + n.eps)?.sqrt()?)?;
                if x.len() > 1 {
                    Ok(normed.broadcast_mul(&x[1])?)
                } else {
                    Ok(normed)
                }
            }
            29 => {
                // linear: w [out, in] (torch), optional bias. candle matmul
                // needs matching ranks — flatten the leading dims, matmul as
                // 2-D, restore.
                let w_t = x[0].t()?;
                let h = &x[1];
                let out = if h.rank() > 2 {
                    let hd = h.dims().to_vec();
                    let in_dim = w_t.dim(0)?;
                    let out_dim = w_t.dim(1)?;
                    let rows: usize = hd[..hd.len() - 1].iter().product();
                    let mut od = hd[..hd.len() - 1].to_vec();
                    od.push(out_dim);
                    h.reshape((rows, in_dim))?
                        .matmul(&w_t)?
                        .reshape(od.as_slice())?
                } else {
                    h.matmul(&w_t)?
                };
                if x.len() > 2 {
                    Ok(out.broadcast_add(&x[2])?)
                } else {
                    Ok(out)
                }
            }
            2 => Ok(x[0].broadcast_add(&x[1])?),
            6 => Ok(x[0].broadcast_sub(&x[1])?),
            7 => Ok(x[0].broadcast_mul(&x[1])?),
            8 => Ok(x[0].broadcast_div(&x[1])?),
            11 => Ok(x[0].log()?),
            15 => {
                let d = n.dim.rem_euclid(x[0].rank() as i64) as usize;
                if n.keepdim {
                    Ok(x[0].sum_keepdim(d)?)
                } else {
                    Ok(x[0].sum_keepdim(d)?.squeeze(d)?)
                }
            }
            22 => {
                let d = if n.dim >= 0 {
                    n.dim as usize
                } else {
                    (x[0].rank() as i64 + n.dim) as usize
                };
                let refs: Vec<&Tensor> = x.iter().collect();
                Ok(Tensor::cat(&refs, d)?)
            }
            36 => {
                if let Some(shape) = &n.shape {
                    let mut dims = self.resolve_shape(shape, dyn_shapes)?;
                    // Infer "-1" placeholder dims from the element count.
                    if dims.contains(&usize::MAX) {
                        let known: usize = dims.iter().filter(|&&d| d != usize::MAX).product();
                        let inferred = x[0].elem_count() / known.max(1);
                        for d in dims.iter_mut() {
                            if *d == usize::MAX {
                                *d = inferred;
                            }
                        }
                    }
                    Ok(x[0].reshape(dims.as_slice())?)
                } else if n.name.starts_with("unsqueeze") {
                    let d = n.dim.rem_euclid(x[0].rank() as i64 + 1) as usize;
                    Ok(x[0].unsqueeze(d)?)
                } else if n.name.starts_with("squeeze") {
                    let d = n.dim.rem_euclid(x[0].rank() as i64) as usize;
                    Ok(x[0].squeeze(d)?)
                } else {
                    bail!("unsupported reshape node {}", n.name)
                }
            }
            37 => {
                // slice/getitem/select: ggml_dim -> torch dim; end == -1 trims
                // the RoPE/bias tables to the dynamic sequence length.
                let nd = x[0].rank() as i64;
                let d = if n.ggml_dim >= 0 {
                    nd - 1 - n.ggml_dim
                } else {
                    n.dim
                };
                if d < 0 || d >= nd {
                    bail!("slice dim {} out of range for rank {nd}", n.dim);
                }
                let d = d as usize;
                let axis = x[0].dim(d)?;
                let end: usize = if n.end == -1 {
                    if n.start == 0 && axis == self.max_len + 1 {
                        seq
                    } else {
                        axis
                    }
                } else {
                    (n.end as usize).min(axis)
                };
                let start = n.start as usize;
                let len = end.saturating_sub(start);
                let mut out = x[0].narrow(d, start, len)?;
                if n.is_select {
                    out = out.squeeze(d)?;
                }
                Ok(out)
            }
            38 => Ok(x[0].clone()), // fused into the SDPA op
            46 => {
                let d = n.dim.rem_euclid(x[0].rank() as i64) as usize;
                Ok(candle_nn::ops::softmax(&x[0], d)?)
            }
            50 => {
                let mut t = x[0].clone();
                if let Some(min) = n.min {
                    t = t.clamp(min as f32, f32::MAX)?;
                }
                if let Some(max) = n.max {
                    t = t.clamp(f32::MIN, max as f32)?;
                }
                Ok(t)
            }
            74 => {
                // fused SDPA: q,k,v [B,S,H,D], additive mask broadcastable.
                let scale = n.scale as f64;
                let b = x[0].dim(0)?;
                let s = x[0].dim(1)?;
                let h = x[0].dim(2)?;
                let d = x[0].dim(3)?;
                let tr = |t: &Tensor| t.transpose(1, 2)?.contiguous(); // [B,H,S,D]
                let (q, k, v) = (tr(&x[0])?, tr(&x[1])?, tr(&x[2])?);
                let att = q.matmul(&k.t()?)?.affine(scale, 0.0)?;
                let att = att.broadcast_add(&x[3])?;
                let att = candle_nn::ops::softmax(&att, candle_core::D::Minus1)?;
                let ctx = att.matmul(&v)?; // [B,H,S,D]
                Ok(ctx.transpose(1, 2)?.reshape((b, s, h * d))?)
            }
            91 => match n.unary_op {
                2 => Ok(x[0].neg()?),
                6 => Ok(x[0].relu()?),
                8 => Ok(gelu(&x[0])?),
                other => bail!("unsupported unary_op {other}"),
            },
            200 => Ok(gelu(&x[0])?),
            other => bail!("unsupported opcode {other} ({})", n.name),
        }
    }
}

/// GELU (erf form, matching the julia path's `gelu` helper).
fn gelu(x: &Tensor) -> Result<Tensor> {
    let t = x.affine(1.0 / std::f64::consts::SQRT_2, 0.0)?.erf()?;
    let t = (t + 1.0)?.affine(0.5, 0.0)?;
    Ok((x * t)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden probe against the numpy reference interpreter
    /// (validated against the official `laya` package on the HF safetensors:
    /// choice/noul probabilities match to ~1e-4, score within Q8 error).
    /// `cargo test --lib mv::ggmlc::tests::laya_golden_probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn laya_golden_probe() {
        let gguf = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("runtimes/decision/model/laya/multilingual/model.gguf");
        if !gguf.is_file() {
            eprintln!("SKIP: laya model.gguf not present");
            return;
        }
        let mut f = std::fs::File::open(&gguf).unwrap();
        let m = GgmlcModel::load(&mut f, &Device::Cpu).expect("load");
        // (name, type, state, question, options, expected index, golden scores)
        let cases: &[(&str, DecisionType, &str, &str, Vec<&str>, usize, &[f32])] = &[
            (
                "choice_zh_billing",
                DecisionType::Choice,
                "用户说: 我被扣了两次款, 要求退款",
                "Which team should handle body?",
                vec![
                    "billing: invoices, payments, refunds",
                    "technical: bugs and outages",
                    "sales: pricing",
                ],
                0,
                &[4.5547, -4.5338, -3.1646],
            ),
            (
                "choice_en_technical",
                DecisionType::Choice,
                "The server is returning 500 errors since this morning, users cannot log in.",
                "Which team should handle this?",
                vec![
                    "billing: invoices, payments, refunds",
                    "technical: bugs and outages",
                    "sales: pricing",
                ],
                1,
                &[-4.55, 2.4895, -4.542],
            ),
            (
                "noul_paid",
                DecisionType::Noul,
                "The invoice total is 240 yuan and the payment was received in full.",
                "Does the sender ask for money back?",
                vec!["false", "true"],
                0,
                &[6.1435, -0.3414],
            ),
            (
                "score_urgency",
                DecisionType::Score,
                "Meeting starts at 9am tomorrow, please prepare the deck.",
                "How urgent is this message?",
                vec!["not urgent", "somewhat urgent", "very urgent"],
                1,
                &[-0.8512, 1.1906, -0.1732],
            ),
        ];
        for (name, t, state, q, opts, target, golden) in cases.iter() {
            let out = m
                .decide(&DecisionRequest {
                    decision_type: *t,
                    state: state.to_string(),
                    question: q.to_string(),
                    options: opts.iter().map(|s| s.to_string()).collect(),
                })
                .expect("decide");
            eprintln!(
                "{name}: scores {:?} probs {:?} -> {} (target {target})",
                out.scores.iter().map(|v| format!("{v:.4}")).collect::<Vec<_>>(),
                out.probabilities.iter().map(|v| format!("{v:.4}")).collect::<Vec<_>>(),
                out.option
            );
            assert_eq!(out.index, *target, "{name} argmax mismatch");
            for (got, want) in out.scores.iter().zip(golden.iter()) {
                assert!(
                    (got - want).abs() < 0.02,
                    "{name} score {got} vs golden {want}"
                );
            }
        }
    }
}

#[cfg(test)]
mod compare_tests {
    use super::*;
    use crate::mv::decision::DecisionModel;

    /// laya vs julia side-by-side on the arithmetic cases the user reported
    /// ("1+1", "10+11"). Both models are OUT-OF-DOMAIN for arithmetic (julia's
    /// README explicitly disclaims algebra; laya is a routing/classification
    /// model) — this probe dumps the RAW inputs/outputs so the behaviour can
    /// be compared honestly.
    /// `cargo test --lib mv::ggmlc::compare_tests::laya_julia_compare_probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn laya_julia_compare_probe() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtimes/decision/model");
        let device = Device::Cpu;

        // Six shuffled options per case; each case runs twice — once WITH the
        // correct answer among the options, once WITHOUT it.
        let cases: Vec<(&str, &str, Vec<&str>)> = vec![
            // 1+1 = 2
            ("1+1 含正确答案(2)", "1+1等于几", vec!["3", "7", "2", "9", "5", "8"]),
            ("1+1 不含正确答案", "1+1等于几", vec!["3", "7", "9", "5", "8", "6"]),
            // 10+11 = 21
            ("10+11 含正确答案(21)", "10+11等于几", vec!["22", "19", "21", "25", "18", "24"]),
            ("10+11 不含正确答案", "10+11等于几", vec!["22", "19", "20", "25", "18", "24"]),
        ];

        for (label, question, options) in &cases {
            let req = DecisionRequest {
                decision_type: DecisionType::Choice,
                state: question.to_string(),
                question: question.to_string(),
                options: options.iter().map(|s| s.to_string()).collect(),
            };
            eprintln!("\n================ case [{label}] ================");
            eprintln!(
                "input: {}",
                serde_json::json!({
                    "decisionType": "choice",
                    "state": req.state,
                    "question": req.question,
                    "options": req.options,
                })
            );

            // julia (modern-bert layout)
            let jp = root.join("julia/1/model.gguf");
            if jp.is_file() {
                let mut f = std::fs::File::open(&jp).unwrap();
                match DecisionModel::load(&mut f, jp.parent().unwrap(), &device, None) {
                    Ok(m) => match m.decide(&req) {
                        Ok(out) => eprintln!(
                            "julia output: {}",
                            serde_json::json!({
                                "scores": out.scores.iter().map(|v| (v * 10000.0).round() / 10000.0).collect::<Vec<_>>(),
                                "probabilities": out.probabilities.iter().map(|v| (v * 10000.0).round() / 10000.0).collect::<Vec<_>>(),
                                "index": out.index,
                                "option": out.option,
                            })
                        ),
                        Err(e) => eprintln!("julia decide ERR: {e}"),
                    },
                    Err(e) => eprintln!("julia load ERR: {e}"),
                }
            } else {
                eprintln!("julia model not present, skipped");
            }

            // laya (ggmlc layout)
            let lp = root.join("laya/multilingual/model.gguf");
            if lp.is_file() {
                let mut f = std::fs::File::open(&lp).unwrap();
                match DecisionModel::load(&mut f, lp.parent().unwrap(), &device, None) {
                    Ok(m) => match m.decide(&req) {
                        Ok(out) => eprintln!(
                            "laya  output: {}",
                            serde_json::json!({
                                "scores": out.scores.iter().map(|v| (v * 10000.0).round() / 10000.0).collect::<Vec<_>>(),
                                "probabilities": out.probabilities.iter().map(|v| (v * 10000.0).round() / 10000.0).collect::<Vec<_>>(),
                                "index": out.index,
                                "option": out.option,
                            })
                        ),
                        Err(e) => eprintln!("laya decide ERR: {e}"),
                    },
                    Err(e) => eprintln!("laya load ERR: {e}"),
                }
            } else {
                eprintln!("laya model not present, skipped");
            }
        }
    }
}
