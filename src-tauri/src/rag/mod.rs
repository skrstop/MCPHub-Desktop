//! RAG (Retrieval-Augmented Generation) subsystem.
//!
//! Provides document upload, embedding (candle for GGUF), vector storage
//! (lancedb), and hybrid search for the `/mcp` `rag_search` / `rag_get` tools.
//!
//! Lifecycle is driven by the RAG switch on the page:
//!   off → `start()` → initializing → ready ; ready → `stop()` → off
//! `enabled` = ready. `start()` loads the embedding model (format-detected via
//! `embedder::load_embedder`), opens the vector DB
//! connection, and mounts the MCP tools. `stop()` releases all of it.
//!
//! Modules:
//! - `embedder`: the `Embedder` strategy trait + format-detecting factory +
//!   shared memory/context helpers (backend-agnostic).
//! - `chunker`: document chunking strategy (text/markdown/code) via the
//!   `text-splitter` crate; token-sized via the loaded `Embedder`.
//! - `gguf`: the GGUF backend (`GgufEmbedder`, candle).
//! - `gguf_gemma`: the Gemma/Gemma3 GGUF architecture (bidirectional + mean-pool).
//! - `gguf_qwen3`: the Qwen3 GGUF architecture (causal/bidirectional + last/
//!   mean/cls pooling, driven by `pooling_type`).
//! - `gguf_nomic`: the nomic-bert-moe GGUF architecture (BERT-MoE encoder +
//!   mean-pool, for nomic-embed-text-v2-moe).
//! - `gguf_lfm2`: the lfm2 GGUF architecture (hybrid ShortConv + attention
//!   encoder + CLS-pool, for LFM2.5-Embedding).
//! - `gguf_modernbert`: the modern-bert GGUF architecture (ModernBert encoder +
//!   CLS-pool, for Granite Embedding 97M Multilingual R2).
//! - `vectordb`: lancedb connection + insert/query/delete.
//! - `service`: high-level lifecycle + document + search operations.

pub mod chunker;
pub mod embedder;
pub mod gguf;
pub mod gguf_gemma;
pub mod gguf_lfm2;
pub mod gguf_modernbert;
pub mod gguf_nomic;
pub mod gguf_qwen3;
pub mod service;
pub mod vectordb;
