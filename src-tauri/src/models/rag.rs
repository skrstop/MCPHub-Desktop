//! Serde types for the RAG feature (camelCase to match the frontend in
//! `frontend/src/types/index.ts`).

use serde::{Deserialize, Serialize};

/// RAG search settings: weights applied to hybrid search scoring + max
/// number of results returned per search.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RagSettings {
    #[serde(default = "default_vector_weight")]
    pub vector_weight: f32,
    #[serde(default = "default_keyword_weight")]
    pub keyword_weight: f32,
    #[serde(default = "default_max_results")]
    pub max_results: u32,
    /// Minimum similarity score for a hit to be shown (0..1). Hits with a
    /// final score below this are filtered out of the results. Default 0
    /// (return everything).
    #[serde(default)]
    pub score_threshold: f32,
    /// Chunk size in tokens, used at upload/reindex time. Default 512.
    #[serde(default = "default_chunk_size")]
    pub chunk_size: u32,
    /// Chunk overlap in tokens. Default 100.
    #[serde(default = "default_chunk_overlap")]
    pub chunk_overlap: u32,
}

fn default_vector_weight() -> f32 {
    0.5
}
fn default_keyword_weight() -> f32 {
    0.5
}
fn default_max_results() -> u32 {
    20
}
fn default_chunk_size() -> u32 {
    // In tokens (the model's own unit). 512 tokens is well within the model's
    // 2048-token context, a sensible retrieval granularity.
    512
}
fn default_chunk_overlap() -> u32 {
    100
}

impl Default for RagSettings {
    fn default() -> Self {
        Self {
            vector_weight: 0.5,
            keyword_weight: 0.5,
            max_results: 20,
            score_threshold: 0.0,
            chunk_size: 512,
            chunk_overlap: 100,
        }
    }
}

/// Document metadata for the list view (no content).
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RagDocInfo {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub uploaded_at: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Number of chunks indexed for this doc (0 if indexed before this field
    /// existed / RAG was off at upload).
    #[serde(default)]
    pub chunk_count: u32,
    /// Display label from file_support.json (ext→name); empty if the extension
    /// isn't in the catalog. Display-only — validation is content-based.
    #[serde(default)]
    pub file_type: String,
}

/// Full document (with content) for the View dialog.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RagDoc {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub content: String,
    pub uploaded_at: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub chunk_count: u32,
    #[serde(default)]
    pub file_type: String,
}

/// A search result fragment.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RagSearchResult {
    pub doc_id: String,
    pub doc_name: String,
    pub title: String,
    pub snippet: String,
    pub score: f32,
}

/// A tag with the number of documents that carry it.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RagTagStat {
    pub tag: String,
    pub file_count: u32,
}

/// A file picked from the OS file dialog (by path) — the backend reads bytes
/// from `path` directly, so large files never go through JSON/base64.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RagPickedFile {
    pub path: String,
    pub name: String,
}

/// Result of an upload batch.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RagUploadResult {
    pub success_count: u32,
    pub failure_count: u32,
}

/// Runtime status reported to the frontend switch.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RagStatus {
    pub enabled: bool,
    pub initializing: bool,
    /// True iff the vector table was recreated on the last enable because the
    /// loaded model's embedding dim differs from the on-disk table (model
    /// swapped). The frontend, on seeing this, must trigger a re-index of all
    /// docs (re-embed with the new model) — old embeddings are gone. Stays
    /// true until `reindex_all` completes. Cleared on disable.
    #[serde(default)]
    pub needs_reindex: bool,
}
