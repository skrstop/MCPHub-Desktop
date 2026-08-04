//! RAG (Retrieval-Augmented Generation) subsystem.
//!
//! Provides document upload, embedding (ort + tokenizers for ONNX, candle for
//! GGUF), vector storage (lancedb), and hybrid search for the `/mcp`
//! `rag_search` / `rag_get` tools.
//!
//! Lifecycle is driven by the RAG switch on the page:
//!   off → `start()` → initializing → ready ; ready → `stop()` → off
//! `enabled` = ready. `start()` loads the embedding model (format-detected via
//! `embedder::load_embedder` - ONNX→ort, GGUF→candle), opens the vector DB
//! connection, and mounts the MCP tools. `stop()` releases all of it.
//!
//! Modules:
//! - `embedder`: the `Embedder` strategy trait + format-detecting factory +
//!   shared memory/context helpers (backend-agnostic).
//! - `embedding`: the ONNX backend (`OrtEmbedder`, ort + tokenizers).
//! - `gguf`: the GGUF backend (`GgufEmbedder`, candle).
//! - `gguf_gemma`: the Gemma/Gemma3 GGUF architecture (bidirectional + mean-pool).
//! - `gguf_qwen3`: the Qwen3 GGUF architecture (causal/bidirectional + last/
//!   mean/cls pooling, driven by `pooling_type`).
//! - `vectordb`: lancedb connection + insert/query/delete.
//! - `service`: high-level lifecycle + document + search operations.

pub mod embedder;
pub mod embedding;
pub mod gguf;
pub mod gguf_gemma;
pub mod gguf_qwen3;
pub mod service;
pub mod vectordb;
