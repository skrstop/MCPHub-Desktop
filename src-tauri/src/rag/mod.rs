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
//! - `embedder` / `gguf*`: **moved to `crate::mv`** in the Phase 2 shared
//!   runtime extraction — the model stack is owned by the mv module now
//!   (`crate::mv::embedder` / `crate::mv::gguf*`).
//! - `vectordb`: lancedb table handle for `rag_chunk` (built on the shared
//!   mv connection) + insert/query/delete.
//! - `extract`: content extraction strategies for binary imports (PDF /
//!   Office / image-OCR) + the plain-text fallback, behind a
//!   `ContentExtractor` trait (see `chunker.rs` for the pattern).
//! - `git`: Git data source support (gix shallow clone / refresh / per-source
//!   local credential file; two-staged storage temp→app-data).
//! - `service`: high-level lifecycle + document + search operations.

pub mod chunker;
pub mod extract;
pub mod git;
pub mod service;
pub mod vectordb;
