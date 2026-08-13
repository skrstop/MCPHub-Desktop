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
    /// final score below this are filtered out of the results. Default 0.65 —
    /// small embedding models (e.g. granite 97M) are anisotropic and give high
    /// cosine to unrelated text, so a non-trivial floor keeps results relevant.
    #[serde(default = "default_score_threshold")]
    pub score_threshold: f32,
    /// Chunk size in tokens, used at upload/reindex time. `0` means "auto" —
    /// use the loaded model's deploy.json-recommended `chunkSize` (falling back
    /// to 1024), capped by the model's max context. A positive value is an
    /// explicit override. Default 0 (auto) so users who don't tune it get a
    /// model-appropriate chunk size without touching the setting.
    #[serde(default = "default_chunk_size")]
    pub chunk_size: u32,
    /// Chunk overlap in tokens. `0` means "auto" — use the model's
    /// deploy.json-recommended `chunkOverlap` (falling back to 100). A positive
    /// value is an explicit override. Default 0 (auto).
    #[serde(default = "default_chunk_overlap")]
    pub chunk_overlap: u32,
}

fn default_vector_weight() -> f32 {
    // Vector (semantic) search dominates by default — it carries the meaning;
    // keyword is a recall backstop. 0.9 / 0.1 is the recommended split for
    // embedding models with CLS/mean pooling.
    0.9
}
fn default_keyword_weight() -> f32 {
    0.1
}
fn default_max_results() -> u32 {
    20
}
fn default_score_threshold() -> f32 {
    // 0.65 — small embedding models are anisotropic (unrelated text scores
    // 0.5–0.9), so a non-trivial floor keeps results relevant.
    0.65
}
fn default_chunk_size() -> u32 {
    // 0 = "auto" — resolved per loaded model at reindex time (deploy.json
    // `chunkSize`, else 1024), capped by max_context. See `reindex_doc`.
    0
}
fn default_chunk_overlap() -> u32 {
    // 0 = "auto" — resolved per loaded model (deploy.json `chunkOverlap`, else 100).
    0
}

/// Default content version (1) for legacy docs whose `.meta` predates the
/// `version` field (serde fills it for missing/corrupt values). Referenced by
/// `#[serde(default = "default_version_one")]` (the compiler can't see that
/// use, hence the allow).
#[allow(dead_code)]
fn default_version_one() -> u32 {
    1
}

impl Default for RagSettings {
    fn default() -> Self {
        Self {
            vector_weight: 0.9,
            keyword_weight: 0.1,
            max_results: 20,
            score_threshold: 0.65,
            chunk_size: 0,
            chunk_overlap: 0,
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
    /// Content version. 1 on first upload, +1 each update (update button /
    /// rag_file_update). Legacy docs without the field default to 1. Shown in
    /// the list as "vN" next to the file-type label.
    #[serde(default = "default_version_one")]
    pub version: u32,
    /// The actual on-disk filename (the file content_path_for resolves to) —
    /// `dir/{id}` (uuid, no extension) for uploads, or `dir/{meta.name}` for
    /// rag_file_create docs. Surfaced so the UI can show it under the display
    /// name: when the user opens the file's folder (reveal-in-file-manager),
    /// they can match the selected/visible file to this name even when it
    /// differs from the display name.
    #[serde(default)]
    pub file_name: String,
}
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

/// A single chunk of a document (for the "view chunks" dialog): its 0-based
/// index in the document + the text. No embedding is returned (view-only).
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RagChunk {
    pub chunk_index: i64,
    pub chunk_text: String,
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
