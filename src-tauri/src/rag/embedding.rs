//! ONNX embedding backend (ort + tokenizers), wrapped as an `Embedder`.
//!
//! Loads the sentence-embedding model from a self-contained size dir
//! (`model.onnx` + its weight-data sibling + `tokenizer.json` + `config.json`)
//! and embeds text into a model-determined-dim f32 vector (read from the
//! session output shape at load - NOT hardcoded - so swapping to a different
//! model / dim just works). The ONNX graph already includes mean-pooling +
//! L2-normalize, so the `sentence_embedding` output is the final embedding.
//!
//! NOTE on external data: `model.onnx` may store its weights in a sibling file
//! whose name is fixed by the protobuf's internal external-data reference
//! (e.g. `model_q4f16.onnx_data`). The code loads `model.onnx`; ort resolves
//! the external-data file by the name embedded in the protobuf, so quantized
//! (q4/q4f16) AND non-quantized (fp16/f32) models all load without code
//! changes - just swap the files in the size dir.
//!
//! Execution-provider strategy is driven PURELY by the size dir's `deploy.json`
//! `platform` (AUTO/GPU/CPU; `RAG_GGUF_DEVICE` env overrides):
//!   - AUTO (default): register the platform GPU EP (CoreML/DirectML/CUDA),
//!     CPU appended last as the per-op fallback. ort assigns each op to the
//!     first EP that supports it - "GPU-first with CPU fallback, maximized".
//!   - GPU: same as AUTO (the user insists on GPU).
//!   - CPU: CPU-only (no GPU EP).
//! NOTE: a contrib-quantized model (MatMul4Bits etc.) on a GPU EP per-shape-
//! recompiles + thrashes (GPU EPs can't map those ops, ~8x slower); for such a
//! model set `deploy.json` `"platform":"CPU"`. The format is no longer
//! auto-detected - the user controls it via deploy.json.
//!
//! I/O names (verified from the bundled Gemma3 model):
//!   inputs:  `input_ids` (i64[1, L]), `attention_mask` (i64[1, L])
//!   output:  `sentence_embedding` (f32[1, <embed_dim>])

use std::path::Path;
use anyhow::{anyhow, Result};
use ort::{
    inputs,
    session::{builder::GraphOptimizationLevel, Session},
    value::Tensor,
};
use tokenizers::tokenizer::Tokenizer;

use crate::rag::embedder::{Embedder, Platform};

/// Maximum chunks embedded in a single `session.run`. Larger = fewer model
/// invocations and better EP utilization, bounded by peak activation memory.
/// The service chunks its per-file progress reporting on this same boundary so
/// each progress tick aligns with exactly one `session.run` completing.
pub const EMBED_BATCH_SIZE: usize = 128;

/// A loaded ONNX embedding model + tokenizer. `embed()` needs `&mut self`
/// because ort's `Session::run` takes `&mut self`.
pub struct OrtEmbedder {
    session: Session,
    tokenizer: Tokenizer,
    /// Model context window in tokens, read from `config.json`
    /// (`max_position_embeddings`). Used to cap chunk_size in the UI.
    max_context: u32,
    /// The embedding (output) dimension, read from the loaded session's output
    /// tensor shape - NOT hardcoded. Drives the lancedb FixedSizeList<f32, N>
    /// schema and row slicing in embed/embed_batch.
    embed_dim: usize,
    /// Human-readable execution-provider config that took effect, e.g.
    /// "CoreML+CPU", "DirectML+CPU", "CUDA+CPU", or "CPU" (deploy CPU / no GPU).
    /// Concise (no quant diagnostics); matches the GGUF backend's style. Surfaced
    /// via `ep_label()` in the service enable-time log.
    ep_label: String,
}

impl OrtEmbedder {
    /// Load the model + tokenizer from a self-contained `size_dir` (holds
    /// `model.onnx` + `tokenizer.json` + `config.json`). Reads the embedding
    /// dim from the session output and picks the execution-provider strategy
    /// from the model's op set (see module docs).
    pub fn load(size_dir: &Path) -> Result<Self> {
        let model_path = size_dir.join("model.onnx");
        let tokenizer_path = size_dir.join("tokenizer.json");

        let mut builder = Session::builder()
            .map_err(|e| anyhow!("ort session builder: {}", e))?
            .with_optimization_level(GraphOptimizationLevel::All)
            .map_err(|e| anyhow!("set optimization level: {}", e))?
            // Use ALL logical cores for intra-op parallelism. The previous cap
            // of 4 left performance on the table on 8/10/12-core machines - a
            // leading cause of slow imports (each batched session.run only used
            // 4 threads). More threads = proportionally faster matmuls.
            .with_intra_threads(num_cpus_or_1())
            .map_err(|e| anyhow!("set intra threads: {}", e))?;

        // Execution providers: driven purely by the size dir's `deploy.json`
        // platform (AUTO/GPU/CPU; `RAG_GGUF_DEVICE` env overrides). The CPU EP
        // is always appended last as the per-op fallback; ort assigns each op
        // to the first EP that supports it. ort EP registration is best-effort
        // - if the GPU EP can't init (no GPU / EP not in the ort build) its
        // `register()` errors and we run CPU-only (session creation continues).
        //   - AUTO (default): GPU-first (register the platform GPU EP), CPU fallback.
        //   - GPU: register the GPU EP (user insists on GPU).
        //   - CPU: CPU-only (no GPU EP).
        // NOTE: a contrib-quantized ONNX model (MatMul4Bits etc.) on a GPU EP
        // per-shape-recompiles + thrashes (GPU EPs can't map those ops) - for
        // such a model set deploy.json `"platform":"CPU"`.
        let platform = crate::rag::embedder::resolve_platform(size_dir);
        let gpu_ep_name = match platform {
            Platform::Cpu => None,
            Platform::Auto | Platform::Gpu => register_gpu_ep(&mut builder),
        };
        let mut providers: Vec<ort::ep::ExecutionProviderDispatch> = Vec::new();
        providers.push(ort::ep::CPU::default().with_arena_allocator(true).build());
        let session = builder
            .with_execution_providers(&providers)
            .map_err(|e| anyhow!("execution providers: {}", e))?
            .commit_from_file(&model_path)
            .map_err(|e| anyhow!("load ONNX model {}: {}", model_path.display(), e))?;

        // Read the embedding dim from the session's first output tensor shape.
        // The last dimension is concrete (the model's output dim); leading dims
        // (batch) are dynamic. Fall back to 768 only if the shape is missing.
        let embed_dim = session
            .outputs()
            .first()
            .and_then(|o| o.dtype().tensor_shape())
            .and_then(|s| s.last().copied())
            .filter(|&d| d > 0)
            .unwrap_or(768) as usize;

        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow!("load tokenizer {}: {}", tokenizer_path.display(), e))?;

        let max_context = crate::rag::embedder::read_max_context(size_dir);
        // Concise ep_label (no quant diagnostics) - matches the GGUF backend's
        // style; the service enable log prints `backend + ep_label`.
        let ep_label = match platform {
            Platform::Cpu => "CPU".to_string(),
            Platform::Auto | Platform::Gpu => match gpu_ep_name {
                Some(n) => format!("{}+CPU", n),
                None => "CPU".to_string(),
            },
        };
        Ok(Self {
            session,
            tokenizer,
            max_context,
            embed_dim,
            ep_label,
        })
    }
}

impl Embedder for OrtEmbedder {
    fn embed(&mut self, text: &str) -> Result<Vec<f32>> {
        let enc = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| anyhow!("tokenize text: {}", e))?;
        let ids: Vec<i64> = enc.get_ids().iter().map(|&v| v as i64).collect();
        let mask: Vec<i64> = enc.get_attention_mask().iter().map(|&v| v as i64).collect();
        if ids.is_empty() {
            return Ok(vec![0.0; self.embed_dim]);
        }
        let seq_len = ids.len() as i64;
        let shape = vec![1_i64, seq_len];
        let ids_tensor =
            Tensor::from_array((shape.clone(), ids)).map_err(|e| anyhow!("build input_ids tensor: {}", e))?;
        let mask_tensor =
            Tensor::from_array((shape, mask)).map_err(|e| anyhow!("build attention_mask tensor: {}", e))?;

        let outputs = self
            .session
            .run(inputs!["input_ids" => ids_tensor, "attention_mask" => mask_tensor])
            .map_err(|e| anyhow!("run embedding session: {}", e))?;

        let emb = &outputs["sentence_embedding"];
        let (_shape, data) = emb
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("extract sentence_embedding: {}", e))?;
        Ok(data.to_vec())
    }

    fn embed_batch(&mut self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        // Larger batch = fewer session.run calls and better CPU/CoreML
        // utilization. 128 chunks per run keeps peak activation memory bounded
        // for the typical chunk (~200 tokens from chunk_size=512 chars); the
        // arena allocator reuses tensors across the many sub-batches during a
        // single import. Exposed as `EMBED_BATCH_SIZE` so the service can chunk
        // its per-file progress reporting on the same boundary.
        let cap = self.max_context.max(1) as usize;
        let mut out = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(EMBED_BATCH_SIZE) {
            // Parallel tokenization of the whole sub-batch at once
            // (encode_batch uses rayon internally). The tokenizer config has
            // no padding/truncation set (verified), so each Encoding keeps its
            // own length and we pad to the longest in-batch below.
            let batch = chunk.len();
            let encodings = self
                .tokenizer
                .encode_batch(chunk.to_vec(), true)
                .map_err(|e| anyhow!("tokenize batch: {}", e))?;
            let mut max_len = 0usize;
            let mut lens: Vec<usize> = Vec::with_capacity(batch);
            for enc in &encodings {
                // Truncate to the model's context window. Without this a chunk
                // whose token count exceeds max_position_embeddings makes
                // session.run error out (input shape too large) - the main
                // cause of failed uploads when chunk_size is set high.
                let n = enc.get_ids().len().min(cap);
                max_len = max_len.max(n);
                lens.push(n);
            }
            max_len = max_len.max(1);
            // Flatten to [batch * max_len] with right-padding (mask=0 on pad).
            // Each row is truncated to its own `n` then padded to max_len.
            // NOTE: ids/mask must be i64 - the ONNX graph expects int64. The
            // tokenizer returns u32; if we feed a uint32 tensor ort errors with
            // "Unexpected input data type. Actual: (tensor(uint32)), expected:
            // (tensor(int64))" and the whole upload fails. Cast here, matching
            // the single-chunk `embed()` path.
            let mut ids: Vec<i64> = Vec::with_capacity(batch * max_len);
            let mut mask: Vec<i64> = Vec::with_capacity(batch * max_len);
            for (enc, &n) in encodings.iter().zip(lens.iter()) {
                ids.extend(enc.get_ids()[..n].iter().map(|&v| v as i64));
                mask.extend(enc.get_attention_mask()[..n].iter().map(|&v| v as i64));
                for _ in n..max_len {
                    ids.push(0);
                    mask.push(0);
                }
            }
            let shape = vec![batch as i64, max_len as i64];
            let ids_tensor = Tensor::from_array((shape.clone(), ids))
                .map_err(|e| anyhow!("build input_ids tensor: {}", e))?;
            let mask_tensor = Tensor::from_array((shape, mask))
                .map_err(|e| anyhow!("build attention_mask tensor: {}", e))?;
            let outputs = self
                .session
                .run(inputs!["input_ids" => ids_tensor, "attention_mask" => mask_tensor])
                .map_err(|e| anyhow!("run embedding batch: {}", e))?;
            let emb = &outputs["sentence_embedding"];
            let (_shape, data) = emb
                .try_extract_tensor::<f32>()
                .map_err(|e| anyhow!("extract sentence_embedding: {}", e))?;
            // Output is [batch, embed_dim] row-major; slice one row per input.
            for i in 0..batch {
                let start = i * self.embed_dim;
                out.push(data[start..start + self.embed_dim].to_vec());
            }
        }
        Ok(out)
    }

    fn embed_dim(&self) -> usize {
        self.embed_dim
    }

    fn max_context(&self) -> u32 {
        self.max_context
    }

    fn tokenize_offsets(&self, text: &str) -> Vec<(usize, usize)> {
        match self.tokenizer.encode(text, false) {
            Ok(enc) => enc.get_offsets().to_vec(),
            Err(_) => Vec::new(),
        }
    }

    fn ep_label(&self) -> &str {
        &self.ep_label
    }

    fn backend(&self) -> &str {
        "onnx"
    }
}

/// Number of logical CPU cores, or 1 if unavailable. Used to cap intra-op
/// threads so we don't oversubscribe on small machines.
fn num_cpus_or_1() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// Register the platform's GPU execution provider on `builder` (best-effort),
/// to be called BEFORE the CPU EP is appended so ort prefers GPU. Returns the
/// EP name on success, or `None` if registration failed (no GPU / EP not in
/// the ort build / platform has no GPU EP) - caller then runs CPU-only.
///
/// Each EP is registered via the per-EP `register()` trait method rather than
/// `with_execution_providers([gpu, cpu])` because the latter errors wholesale
/// if ANY EP fails, which would abort session creation. With per-EP register,
/// a GPU EP that can't init is simply skipped and the CPU EP (appended by the
/// caller) handles everything - true "GPU-first, CPU-fallback".
///
/// Platform EPs (gated by Cargo features in Cargo.toml):
///   - macOS  -> CoreML MLProgram, All compute units (ANE > GPU > CPU).
///   - Windows-> DirectML (DirectX 12; NVIDIA/AMD/Intel).
///   - Linux  -> CUDA (NVIDIA; fails->CPU on non-NVIDIA boxes).
fn register_gpu_ep(builder: &mut ort::session::builder::SessionBuilder) -> Option<&'static str> {
    use ort::ep::ExecutionProvider;

    #[cfg(target_os = "macos")]
    {
        let ep = ort::ep::CoreML::default()
            .with_model_format(ort::ep::coreml::ModelFormat::MLProgram)
            .with_compute_units(ort::ep::coreml::ComputeUnits::All);
        match ep.register(builder) {
            Ok(()) => return Some("CoreML"),
            Err(e) => log::warn!("[RAG] CoreML EP register failed, CPU fallback: {e}"),
        }
    }
    #[cfg(target_os = "windows")]
    {
        let ep = ort::ep::DirectML::default();
        match ep.register(builder) {
            Ok(()) => return Some("DirectML"),
            Err(e) => log::warn!("[RAG] DirectML EP register failed, CPU fallback: {e}"),
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let ep = ort::ep::CUDA::default();
        match ep.register(builder) {
            Ok(()) => return Some("CUDA"),
            Err(e) => log::warn!("[RAG] CUDA EP register failed, CPU fallback: {e}"),
        }
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "windows",
        all(unix, not(target_os = "macos"))
    )))]
    {
        let _ = builder;
    }
    None
}
