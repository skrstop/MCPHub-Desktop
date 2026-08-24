//! Tauri command wrappers for the RAG service. Each maps 1:1 to a service
//! function and returns `Result<T, String>` (Tauri's convention).

use tauri::AppHandle;

use crate::models::rag::{
    BatchPreview, RagDoc, RagDocInfo, RagPickedFile, RagSearchResult, RagSettings, RagStatus,
    RagTagPage, RagTagStat, RagUpdateCheck,
};
use crate::rag::service;

#[tauri::command]
pub async fn rag_toggle(app: AppHandle, enabled: bool) -> Result<RagStatus, String> {
    service::toggle(&app, enabled).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rag_status() -> Result<RagStatus, String> {
    Ok(service::status())
}

#[tauri::command]
pub async fn list_rag_docs(app: AppHandle) -> Result<Vec<RagDocInfo>, String> {
    service::list_docs(&app).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_rag_doc(app: AppHandle, id: String) -> Result<Option<RagDoc>, String> {
    service::get_doc(&app, &id).await.map_err(|e| e.to_string())
}

/// Read a document's chunks (index + text, no embeddings) for the "view
/// chunks" dialog. Requires RAG enabled (chunks live in lancedb).
#[tauri::command]
pub async fn get_rag_chunks(id: String) -> Result<Vec<crate::models::rag::RagChunk>, String> {
    service::get_doc_chunks(&id).await.map_err(|e| e.to_string())
}

/// Open the OS multi-file picker (no extension filter — validation is
/// content-based) and return the chosen paths + display names. No file bytes
/// cross the IPC boundary — the backend reads from disk at upload time.
/// Async so the blocking dialog doesn't freeze the UI (runs off the main
/// thread; the dialog itself is dispatched to the main thread by the plugin).
#[tauri::command]
pub async fn pick_rag_files(app: AppHandle) -> Result<Vec<RagPickedFile>, String> {
    Ok(service::pick_files(&app))
}

/// Open the OS folder picker, scan the folder's immediate (non-recursive) file
/// children, and return them as import candidates. Hidden files + sub-directories
/// are skipped. Same return shape as `pick_rag_files` so the upload pipeline is
/// identical to multi-file import.
#[tauri::command]
pub async fn pick_rag_folder(app: AppHandle) -> Result<Vec<RagPickedFile>, String> {
    Ok(service::pick_folder(&app))
}

/// Read + decode-to-UTF-8 + chunk + embed + index a single file (by disk
/// path). The frontend loops over the picked paths, calling this once per
/// file so it can show per-file upload progress. `method` selects the import
/// method: "symlink" (default — record original_path, no copy) or "copy"
/// (copy bytes into rag/files). `None` defaults to "symlink" to match the
/// UI default.
#[tauri::command]
pub async fn upload_rag_doc(
    app: AppHandle,
    file_path: String,
    tags: Vec<String>,
    method: Option<String>,
) -> Result<(), String> {
    service::upload_one_path(&app, &file_path, tags, method)
        .await
        .map_err(|e| e.to_string())
}

/// Update an existing document in place. `mode`:
/// - `"original"`: re-read the recorded `original_path` and re-index its
///   current content (the "from original" button). `file_path` is ignored.
/// - `"file"`: read a freshly-picked `file_path` and overwrite (the "manual
///   upload" button). The new file becomes the recorded original_path + its
///   md5 is stored, so future update detection works (this is the legacy-compat
///   path for old docs that had no original_path).
/// Returns the new chunk count. Requires RAG enabled.
#[tauri::command]
pub async fn update_rag_doc(
    app: AppHandle,
    id: String,
    mode: String,
    file_path: Option<String>,
) -> Result<u32, String> {
    if mode == "original" {
        service::update_doc_from_original(&app, &id)
            .await
            .map_err(|e| e.to_string())
    } else {
        let fp = file_path.unwrap_or_default();
        if fp.is_empty() {
            return Err("file_path is required for manual-upload update".to_string());
        }
        service::update_doc_from_file(&app, &id, &fp)
            .await
            .map_err(|e| e.to_string())
    }
}

/// Single-doc update check: classify the recorded original's state so the
/// per-row UpdateDialog can render the right branch (lost / changed /
/// no-change / legacy-no-path). Cheap (one md5 of the source, no embedding).
#[tauri::command]
pub async fn check_rag_update(app: AppHandle, id: String) -> Result<RagUpdateCheck, String> {
    service::check_rag_update(&app, &id)
        .await
        .map_err(|e| e.to_string())
}

/// Batch-update preview: aggregate counts (total / to_update / skipped / lost)
/// over all docs, shown in the confirm dialog before the expensive re-index
/// pass runs. No embedding, no progress events.
#[tauri::command]
pub async fn preview_batch_update(app: AppHandle) -> Result<BatchPreview, String> {
    service::preview_batch_update(&app)
        .await
        .map_err(|e| e.to_string())
}

/// Run the batch update in the background: re-index every doc whose source
/// changed (md5 differs, or legacy no-md5 treated as changed). Emits
/// `rag://batch-update-progress` per doc. Lost/legacy docs are skipped.
/// Returns immediately; work runs on a spawned task. Guarded against double
/// triggers (a second call while one is running is a no-op).
#[tauri::command]
pub async fn batch_update_rag_docs(app: AppHandle) -> Result<(), String> {
    service::batch_update_rag_docs(app);
    Ok(())
}

#[tauri::command]
pub async fn delete_rag_doc(app: AppHandle, id: String) -> Result<(), String> {
    service::delete_doc(&app, &id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rag_search_command(query: String, tags: Vec<String>) -> Result<Vec<RagSearchResult>, String> {
    service::search(query, tags).await.map_err(|e| e.to_string())
}

/// List/search distinct tags in the RAG library. `search_key` filters by
/// case-insensitive substring (any match); empty returns all tags. Each tag
/// is returned with its file count.
#[tauri::command]
pub async fn rag_tag_search(search_key: Vec<String>) -> Result<Vec<RagTagStat>, String> {
    service::list_tags(search_key).await.map_err(|e| e.to_string())
}

/// Paginated tag search for the frontend's searchable dropdowns. Returns one
/// page of tags (SQL-level LIKE + LIMIT/OFFSET) + the total matching count,
/// so the UI can fetch more pages on demand. `page` is 0-based.
#[tauri::command]
pub async fn rag_tag_search_paged(
    search_key: String,
    page: u32,
    page_size: u32,
) -> Result<RagTagPage, String> {
    service::list_tags_paged(search_key, page, page_size)
        .await
        .map_err(|e| e.to_string())
}

/// Paginated RAG document search for the file list's toolbar: `search_key` is
/// a case-insensitive substring on the doc name (empty = all), `tags` is an
/// ANY-match tag filter. Returns one page of `RagDocInfo` (enriched from the
/// `.meta` files) + the total matching count. `page` is 0-based.
#[tauri::command]
pub async fn rag_doc_search_paged(
    app: AppHandle,
    search_key: String,
    tags: Vec<String>,
    page: u32,
    page_size: u32,
) -> Result<crate::models::rag::RagDocPage, String> {
    service::search_docs_paged(&app, search_key, tags, page, page_size)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_rag_tags(app: AppHandle, id: String, tags: Vec<String>) -> Result<(), String> {
    service::set_doc_tags(&app, &id, tags).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_rag_settings() -> Result<RagSettings, String> {
    service::get_settings().await.map_err(|e| e.to_string())
}

/// The model's context window in tokens, read from the model's `config.json`
/// (`max_position_embeddings`). Used by the frontend to cap the chunk_size
/// input so a chunk can't exceed what the model can encode. Always derived
/// from the actual model - never hardcoded - so swapping models just works.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RagModelLimits {
    pub max_context: u32,
    /// Model-author-recommended chunk size (tokens), from the loaded model's
    /// deploy.json `chunkSize`. `None` if unset (the service falls back to
    /// 1024). Shown by the frontend's Auto mode + used to seed manual sliders.
    pub chunk_size: Option<u32>,
    /// Model-author-recommended chunk overlap (tokens), from deploy.json
    /// `chunkOverlap`. `None` if unset (falls back to 100).
    pub chunk_overlap: Option<u32>,
}

#[tauri::command]
pub async fn rag_model_limits(app: AppHandle) -> Result<RagModelLimits, String> {
    let (max_context, chunk_size, chunk_overlap) =
        service::model_chunk_recommendation(&app).await;
    Ok(RagModelLimits {
        max_context,
        chunk_size,
        chunk_overlap,
    })
}

/// The app-level RAG tools (rag_search / rag_get / rag_tag_search) as MCP
/// tool definitions (name / description / inputSchema). Returns an empty list
/// when RAG is disabled. Powers the "view tools" dialog in the RAG page and is
/// the same source the HTTP MCP layer advertises in tools/list.
#[tauri::command]
pub async fn rag_tools() -> Result<Vec<serde_json::Value>, String> {
    if !service::is_enabled() {
        return Ok(Vec::new());
    }
    Ok(service::tool_definitions())
}

#[tauri::command]
pub async fn save_rag_settings(settings: RagSettings) -> Result<(), String> {
    service::save_settings(settings).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn open_rag_file_location(app: AppHandle, id: String) -> Result<(), String> {
    service::open_file_location(&app, &id).await.map_err(|e| e.to_string())
}

/// Re-embed every uploaded doc with the currently-loaded model, after a model
/// swap recreated the vector table (embedding dim changed). Drives the same
/// upload overlay the frontend uses for imports: emits `rag://reindex-progress`
/// per doc (file-level bar) and reuses `reindex_doc`'s `rag://upload-progress`
/// (char-level bar). Clears `needs_reindex` when done. No-op (returns 0) if no
/// docs exist.
#[tauri::command]
pub async fn rag_reindex_all(app: AppHandle) -> Result<usize, String> {
    service::reindex_all(&app).await.map_err(|e| e.to_string())
}

/// List available model sizes (scanned from `runtimes/rag/model/<family>/<size>/`).
/// Each entry's status is "ready" (selectable) or "downloadable" (has a
/// download.url, fetch via `rag_download_model` first).
#[tauri::command]
pub async fn rag_list_models(app: AppHandle) -> Result<Vec<service::RagModelInfo>, String> {
    service::list_models(&app).map_err(|e| e.to_string())
}

/// The currently-selected model size (`config_json.rag.model`), or null.
#[tauri::command]
pub async fn rag_current_model() -> Result<Option<String>, String> {
    Ok(service::current_model().await)
}

/// Select a model size: persist it and auto-restart RAG if enabled (so the new
/// model loads). Errors if the size isn't ready. Returns the post-restart
/// status (with `needs_reindex` if the dim changed).
#[tauri::command]
pub async fn rag_select_model(app: AppHandle, size: String) -> Result<crate::models::rag::RagStatus, String> {
    service::select_model(&app, &size).await.map_err(|e| e.to_string())
}

/// Download a model size via its `download.url` (a GGUF model file). Streams
/// with progress on `rag://model-download` into
/// `<app_data>/rag/models/<family>/<size>/`. After success the size is ready.
#[tauri::command]
pub async fn rag_download_model(app: AppHandle, size: String) -> Result<(), String> {
    service::download_model(&app, &size).await.map_err(|e| e.to_string())
}
