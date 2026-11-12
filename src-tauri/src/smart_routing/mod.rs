//! smart_routing — Smart Routing backend (Phase 3): tool/server vector index
//! over the shared mv runtime + hybrid retrieval.
//!
//! Mirrors origin's `vectorSearchService.ts` semantics, adapted to the desktop:
//! - one lancedb table `smart_tool` (shared mv connection) holds one row per
//!   tool plus one row per server (`content_type` = "tool" | "server"),
//!   mirroring origin's pgvector `vector_embeddings` rows.
//! - `toolSetHash` uses the same scrypt parameters as origin
//!   (N=2048/r=8/p=1, 32 bytes, key `mcphub:toolset-embedding-cache:v1`) over
//!   the same stable-serialized normalized tool list — so the skip-check
//!   semantics are identical even though embeddings come from the local model.
//! - search is desktop hybrid retrieval (vector_weight/keyword_weight +
//!   score_threshold + max_results) instead of origin's dynamic thresholds.
//!
//! Lifecycle hooks (B9):
//! - pool connect success → `index::on_server_connected` (background)
//! - server delete / rename / disable → `index::remove_server_embeddings`
//! - config update → `index::on_server_updated` (re-runs save; hash skip-check
//!   makes it a no-op when nothing changed)

pub mod index;
pub mod meta;
pub mod models;
pub mod search;
pub mod store;

pub use store::{store_index_summary, store_performance};
