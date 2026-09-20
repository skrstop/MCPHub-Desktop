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
    /// Auto doc update: periodically (every `auto_update_interval_secs`) check
    /// every doc's recorded original for md5 changes and re-index the changed
    /// ones in the background — the same pass as the manual "批量更新" button.
    /// Default true (on). Only runs while RAG is enabled (re-index needs the
    /// embedding runtime).
    #[serde(default = "default_auto_update_enabled")]
    pub auto_update_enabled: bool,
    /// Auto-update check interval in seconds. Default 300 (5 minutes). Clamped
    /// to [60, 86400] at read time so a bad persisted value can't busy-loop.
    #[serde(default = "default_auto_update_interval_secs")]
    pub auto_update_interval_secs: u64,
    /// Doc-detail page size in KiB: how much content `get_rag_doc_paged`
    /// returns per call (the View dialog loads this much up front, then the
    /// "load more" button fetches the next page). Default 200. Clamped to
    /// [10, 65536] at read time (the upload cap is 64 MiB).
    #[serde(default = "default_doc_load_chunk_kb")]
    pub doc_load_chunk_kb: u32,
    /// Auto-sync NEW files appearing in folder/git data sources during update
    /// checks (batch/auto). Off (default) = only re-index the files that were
    /// selected at import time; on = newly added source files are imported
    /// automatically too (removal sync always runs).
    #[serde(default)]
    pub source_sync_add_enabled: bool,
    /// Auto-remove docs whose source file vanished from the data source
    /// during update checks. On (default) = vanished docs are deleted
    /// automatically; off = docs are kept and only flagged "lost original"
    /// in the UI.
    #[serde(default = "default_source_sync_remove_enabled")]
    pub source_sync_remove_enabled: bool,
}

fn default_source_sync_remove_enabled() -> bool {
    true
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
fn default_auto_update_enabled() -> bool {
    // Auto doc update on by default — docs linked to originals stay fresh
    // without the user clicking "批量更新".
    true
}
fn default_auto_update_interval_secs() -> u64 {
    // 5 minutes between auto-update scans.
    300
}
fn default_doc_load_chunk_kb() -> u32 {
    // 200 KiB per doc-detail page — keeps the View dialog cheap on huge docs.
    200
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
            auto_update_enabled: true,
            auto_update_interval_secs: 300,
            doc_load_chunk_kb: 200,
            source_sync_add_enabled: false,
            source_sync_remove_enabled: true,
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
    /// differs from the display name. Empty string for "symlink" docs (no
    /// copied file — the content lives at `original_path`).
    #[serde(default)]
    pub file_name: String,
    /// Import method: "symlink" | "copy". `None`/empty for legacy docs (pre-
    /// feature) — treated as "copy" by the frontend.
    #[serde(default)]
    pub method: String,
    /// Absolute path of the original imported file. Surfaced so the list can
    /// show the method badge tooltip + so "open original location" works.
    #[serde(default)]
    pub original_path: String,
    /// MD5 (hex) of the source content captured at import time. Surfaced for
    /// completeness / debugging.
    #[serde(default)]
    pub md5: String,
    /// True iff the doc has a recorded `original_path` that does NOT exist on
    /// disk now (the source was moved/deleted). Applies to both "symlink"
    /// (no content copy — content unreadable) and "copy" (copy still in
    /// rag/files — content readable, but update-detection source is gone).
    /// The UI uses this for the ⚠️ badge + to disable the auto-update path;
    /// view/open-location disabling is gated on `content_available` (copy
    /// docs with a missing source still have their copy, so view works).
    #[serde(default)]
    pub lost_original: bool,
    /// True iff the doc's content is readable right now. "copy" docs are
    /// always content-available (the copy lives in rag/files). "symlink"
    /// docs are content-available iff `original_path` exists. The UI greys
    /// "view" + "open location" only when this is false (not merely on
    /// `lost_original`), so a copy doc whose original vanished still lets
    /// the user view its imported copy.
    #[serde(default)]
    pub content_available: bool,
    /// Data-source display fields (kind + label + root + rel dir chain +
    /// git url/branch), filled by `classify_source` at read time. Legacy
    /// docs (no `source` in meta) classify as kind "file" — the tree view
    /// groups them under the "file pick" node. `source_kind` is one of
    /// "file" | "folder" | "git" | "tool".
    #[serde(default)]
    pub source_kind: String,
    #[serde(default)]
    pub source_label: String,
    #[serde(default)]
    pub source_root: String,
    #[serde(default)]
    pub rel_path: String,
    #[serde(default)]
    pub git_url: String,
    #[serde(default)]
    pub git_branch: String,
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
    /// Import method ("symlink"|"copy"|"" legacy) — same semantics as
    /// `RagDocInfo.method`.
    #[serde(default)]
    pub method: String,
    /// Original imported file path (empty for legacy copy docs).
    #[serde(default)]
    pub original_path: String,
    /// True iff the recorded original_path is missing on disk (both symlink +
    /// copy count; see `RagDocInfo.lost_original`).
    #[serde(default)]
    pub lost_original: bool,
    /// True iff the doc's content is readable right now (symlink = original
    /// exists; copy = the rag/files copy exists). View/open-location are gated
    /// on this, NOT on `lost_original` — a copy doc whose original vanished
    /// still has its copy, so view works.
    #[serde(default)]
    pub content_available: bool,
    /// True iff `content` is only a PREFIX of the full document (paged read,
    /// `get_rag_doc_paged`). False for the unpaged `get_rag`. When true the UI
    /// shows the "load more" button; each click fetches the next
    /// `doc_load_chunk_kb` slice.
    #[serde(default)]
    pub truncated: bool,
    /// UTF-8 byte offset of the END of the returned `content` within the full
    /// decoded document. For the next page the frontend passes this back as
    /// the read offset. Equals the full length when `truncated` is false.
    #[serde(default)]
    pub next_offset: u64,
    /// Total size of the full decoded content in UTF-8 bytes (independent of
    /// how much was returned this call) — lets the UI show
    /// "已加载 X / 全部 Y KB" without an extra round-trip.
    #[serde(default)]
    pub content_total_bytes: u64,
    /// Data-source display fields — same semantics as `RagDocInfo`.
    #[serde(default)]
    pub source_kind: String,
    #[serde(default)]
    pub source_label: String,
    #[serde(default)]
    pub source_root: String,
    #[serde(default)]
    pub rel_path: String,
    #[serde(default)]
    pub git_url: String,
    #[serde(default)]
    pub git_branch: String,
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

/// A page of a document's chunks (for the paginated "view chunks" dialog):
/// the chunks on this page + the total chunk count so the UI knows when all
/// pages are loaded. `offset` is the 0-based index of the first returned
/// chunk; `page_size` is the per-page cap actually applied.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RagChunkPage {
    pub items: Vec<RagChunk>,
    /// Total chunks across all pages (NOT just this page).
    pub total: u64,
    /// Index of the first returned chunk (0-based).
    pub offset: u32,
    pub page_size: u32,
}

/// A tag with the number of documents that carry it.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RagTagStat {
    pub tag: String,
    pub file_count: u32,
}

/// A page of tag-search results: the items on the current page + the total
/// number of matching tags (so the frontend knows whether more pages exist).
/// `page` is 0-based; `page_size` is the per-page cap actually applied.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RagTagPage {
    pub items: Vec<RagTagStat>,
    /// Total matching tags across all pages (NOT just this page).
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
}

/// A page of doc-search results (mirrors `RagTagPage` for the file list).
/// `page` is 0-based; `page_size` is the per-page cap actually applied.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RagDocPage {
    pub items: Vec<RagDocInfo>,
    /// Total matching docs across all pages (NOT just this page).
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
}

/// A file picked from the OS file dialog (by path) — the backend reads bytes
/// from `path` directly, so large files never go through JSON/base64.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RagPickedFile {
    pub path: String,
    pub name: String,
}

/// A picked/scan candidate file with its size, for the grouped folder-scan
/// result (the Upload dialog shows sizes + a total in its summary bar).
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RagScanFile {
    pub path: String,
    pub name: String,
    /// File size in bytes (0 if metadata failed — the file is still listed).
    pub size: u64,
    /// Path relative to the scan root ("" for root-level files). Carried into
    /// each imported doc's `DocSource.rel_path` so the tree view can place it
    /// under the right directory chain. Empty for multi-file picks.
    #[serde(default)]
    pub rel_path: String,
}

/// Git source remote info attached to a doc's `DocSource` (kind = "git").
/// Auth credentials are deliberately NOT stored here — they live in the
/// app's local credential file.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct GitSource {
    pub url: String,
    pub branch: Option<String>,
    /// Commit sha at the time of the import/sync that produced this doc
    /// (display only; update detection re-fetches and re-hashes files).
    pub commit: Option<String>,
    /// Sub-directory inside the repo the doc came from (reserved; not
    /// exposed in the UI yet — the whole repo is scanned).
    pub subdir: Option<String>,
}

/// Where a document came from ("data source"). Serialized into the doc's
/// `{id}.meta` JSON (no DB column — RAG docs don't live in DB tables). Legacy
/// metas without a `source` field deserialize as `None` and are classified as
/// kind "file" at read time (`classify_source`), so the tree view groups all
/// pre-existing docs under the "file pick" source node.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct DocSource {
    /// "file" | "folder" | "git" | "tool"
    pub kind: String,
    /// Display label for the source node in the tree view: file = parent
    /// dir (or "文件选择" fallback), folder = chosen folder path, git =
    /// `url @ branch`, tool = "工具创建".
    #[serde(default)]
    pub label: String,
    /// Source root: absolute dir for file/folder, clone dir for git.
    #[serde(default)]
    pub root: Option<String>,
    /// Directory chain relative to `root` ("/"-separated, "" = root level).
    #[serde(default)]
    pub rel_path: Option<String>,
    /// Present only when kind = "git".
    #[serde(default)]
    pub git: Option<GitSource>,
}

/// One folder group in a recursive folder-scan result: the folder's path
/// relative to the scanned root ("" for the root itself) + the import
/// candidates it contains (already filtered by the same rules as the flat
/// scan: extension catalog + content sniff + not hidden).
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RagScanGroup {
    /// Folder path relative to the scan root; "" for the root's own files.
    pub rel_path: String,
    pub files: Vec<RagScanFile>,
}

/// Result of a folder scan (recursive or flat) for the Upload dialog's grouped
/// tree view. Flat scans return a single group (the root). The frontend folds
/// multi-file picks into a single pseudo-group too, so all three entry points
/// share one rendering.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RagFolderScan {
    /// Absolute path of the scanned folder ("" for multi-file picks).
    pub root: String,
    /// HEAD commit sha of the scanned git clone (git data source only; None
    /// for file/folder scans). Display-only — update detection is md5-based.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// Number of sub-directories that were skipped by folder_ignore.json
    /// (recursive scans only; 0 for flat scans).
    pub skipped_dirs: u32,
    /// Number of candidate files dropped because they exceeded the scan cap.
    /// 0 unless `truncated` is true.
    pub skipped_files: u32,
    pub groups: Vec<RagScanGroup>,
    /// True iff the scan stopped early at the candidate cap (`SCAN_FILE_CAP`);
    /// the summary shows a warning and suggests picking a smaller folder.
    pub truncated: bool,
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

/// Single-doc update check result (drives the per-row UpdateDialog branches):
/// does the original file exist, and if so has it changed vs the stored md5?
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RagUpdateCheck {
    /// "symlink" | "copy" | "" (legacy)
    pub method: String,
    /// False for legacy docs that predate `original_path` (no recorded source).
    pub has_original_path: bool,
    /// True iff the recorded original_path exists on disk right now.
    pub original_exists: bool,
    /// False for legacy docs that predate `md5` (no stored hash).
    pub has_md5: bool,
    /// True iff the source exists AND its current md5 differs from the stored
    /// md5 (or there's no stored md5 -> treat as "has update" for legacy).
    pub original_changed: bool,
    /// True iff symlink method + original_path missing (UI: only manual-upload
    /// is offered).
    pub lost_original: bool,
    /// Git source refresh failure for this doc's repo (address changed /
    /// credentials revoked). None = repo refreshed fine or doc isn't git.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_error: Option<GitSourceError>,
}

/// Batch-update preview: classification counts over all docs, shown in the
/// confirm dialog before the user starts the (expensive) async re-index pass.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BatchPreview {
    /// Total docs scanned.
    pub total: u32,
    /// Docs whose source exists + md5 changed (or legacy no-md5) -> will be
    /// re-indexed.
    pub to_update: u32,
    /// Docs with no change, or legacy docs with no recorded original_path
    /// (can't auto-update, left to single-doc manual upload).
    pub skipped: u32,
    /// Docs whose original_path is missing on disk (skipped; user can manually
    /// upload to overwrite).
    pub lost: u32,
    /// Files detected in folder/git sources that no doc references yet ->
    /// will be imported by the batch (source-level sync).
    pub added: u32,
    /// Docs whose recorded original_path is under a KNOWN source root
    /// (folder/git) but no longer present in a fresh scan of that source ->
    /// will be deleted by the batch (source-level sync).
    pub removed: u32,
    /// Per-repo refresh failures (address changed / credentials revoked /
    /// network down). `label` = repo url @ branch, `auth` = true means the
    /// fix is credentials (re-pick in the import dialog), false = network /
    /// not-found (address may have moved). Empty when every repo refreshed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub git_errors: Vec<GitSourceError>,
}

/// One git repo's refresh failure, surfaced to the UI so the user can fix the
/// source (re-enter credentials / update the URL) instead of silently getting
/// stale md5 checks.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GitSourceError {
    /// Repo URL (no credentials).
    pub url: String,
    /// Branch as recorded on the source (may be empty = remote default).
    pub branch: String,
    /// True = auth problem (401/403/credentials) -> re-enter credentials;
    /// false = other (not found / network) -> URL may have changed or be
    /// unreachable.
    pub auth: bool,
    /// Short error detail (already stripped of the GIT_AUTH_REQUIRED prefix).
    pub message: String,
}
