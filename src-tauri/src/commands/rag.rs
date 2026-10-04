//! Tauri command wrappers for the RAG service. Each maps 1:1 to a service
//! function and returns `Result<T, String>` (Tauri's convention).

use tauri::State;

use crate::commands::auth::SessionState;

use anyhow::Result;
use tauri::AppHandle;

use crate::models::rag::{
    BatchPreview, RagChunkPage, RagDoc, RagDocInfo, RagFolderScan, RagPickedFile, RagSearchResult,
    RagSettings, RagStatus, RagTagPage, RagTagStat, RagUpdateCheck,
};
use crate::rag::service;

#[tauri::command]
pub async fn rag_toggle(
    session: State<'_, SessionState>,
app: AppHandle, enabled: bool) -> Result<RagStatus, String> {
    // RAG write: upload accepts arbitrary paths (read->index->search exfil channel) — admin-gated in multi-user mode.
    crate::commands::config::require_admin(&session).await?;
    service::toggle(&app, enabled).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rag_status() -> Result<RagStatus, String> {
    Ok(service::status())
}

/// OCR capability probe for the upload dialog's pre-flight + the failure
/// dialog (image OCR unavailable on Linux without tesseract). Cheap — the
/// Linux probe is cached after the first call.
#[tauri::command]
pub fn get_ocr_status() -> crate::rag::extract::ocr::OcrStatus {
    crate::rag::extract::ocr::status()
}

#[tauri::command]
pub async fn list_rag_docs(session: State<'_, SessionState>, app: AppHandle) -> Result<Vec<RagDocInfo>, String> {
    // Doc names/original_paths leak the index inventory — gate like
    // rag_doc_search_paged (same data, already gated).
    crate::commands::config::require_admin(&session).await?;
    service::list_docs(&app).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_rag_doc(
    session: State<'_, SessionState>,
    app: AppHandle,
    id: String,
) -> Result<Option<RagDoc>, String> {
    // Doc content is a read-exfil channel (upload accepts arbitrary paths) — admin-gated in multi-user mode.
    crate::commands::config::require_admin(&session).await?;
        service::get_doc(&app, &id).await.map_err(|e| e.to_string())
}

/// Paged doc-detail read for the View dialog: skip `offset` UTF-8 bytes of the
/// decoded content, return up to `limit_bytes` more (char-boundary aligned).
/// `limit_bytes = 0` uses the user's configured `doc_load_chunk_kb` page size.
/// The result's `truncated` / `nextOffset` / `contentTotalBytes` drive the
/// "load more" button. Prevents loading a huge doc's whole content at once.
#[tauri::command]
pub async fn get_rag_doc_paged(
    session: State<'_, SessionState>,
    app: AppHandle,
    id: String,
    offset_bytes: u64,
    limit_bytes: u64,
) -> Result<Option<RagDoc>, String> {
    crate::commands::config::require_admin(&session).await?;
        service::get_doc_paged(&app, &id, offset_bytes, limit_bytes)
        .await
        .map_err(|e| e.to_string())
}

/// Read a document's chunks (index + text, no embeddings) for the "view
/// chunks" dialog. Requires RAG enabled (chunks live in lancedb).
#[tauri::command]
pub async fn get_rag_chunks(
    session: State<'_, SessionState>,
    id: String,
) -> Result<Vec<crate::models::rag::RagChunk>, String> {
    crate::commands::config::require_admin(&session).await?;
        service::get_doc_chunks(&id).await.map_err(|e| e.to_string())
}

/// Paginated chunks for the "view chunks" dialog: `offset` chunks skipped,
/// then up to `page_size` returned (clamped [1,200]) + the total count so the
/// UI can auto-load the next page on scroll-to-bottom. Requires RAG enabled.
#[tauri::command]
pub async fn get_rag_chunks_paged(
    session: State<'_, SessionState>,
    id: String,
    offset: u32,
    page_size: u32,
) -> Result<RagChunkPage, String> {
    crate::commands::config::require_admin(&session).await?;
        service::get_doc_chunks_paged(&id, offset, page_size)
        .await
        .map_err(|e| e.to_string())
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

/// Open the OS folder picker, scan the folder, and return import candidates
/// grouped per sub-directory. `recursive=false` (the pre-existing behavior)
/// scans only immediate file children as a single root group; `recursive=true`
/// walks all descendant directories (skipping folder_ignore.json dev dirs,
/// hidden entries, and symlinks) with a 500-file candidate cap. Same per-file
/// filtering as `pick_rag_files`. No file bytes cross the IPC boundary.
#[tauri::command]
pub async fn pick_rag_folder(app: AppHandle, recursive: bool) -> Result<RagFolderScan, String> {
    Ok(service::pick_folder(&app, recursive))
}

/// Git data source: clone (shallow, into the OS temp dir) + scan the clone
/// with the same candidate rules as the folder picker. Optional
/// username/password (private repos; public repos pull anonymously). On
/// success the credential (if given) is stored in the app's local credential
/// file so later updates pull without re-entry. Auth failures surface as
/// `GIT_AUTH_REQUIRED:<detail>` — the frontend shows the credential form.
#[tauri::command]
pub async fn pick_rag_git_repo(
    session: State<'_, SessionState>,
    app: AppHandle,
    url: String,
    branch: Option<String>,
    username: Option<String>,
    password: Option<String>,
    depth: Option<u32>,
) -> Result<RagFolderScan, String> {
    // Admin-gated: a successful pick persists the supplied credential into
    // the app-global `rag/git-credentials.json` — shared mutable state that
    // later admin-gated refreshes will consume. Ungated, any non-admin
    // session could overwrite the admin's stored credential for the same
    // repo (credential downgrade / auth breakage on subsequent refreshes).
    crate::commands::config::require_admin(&session).await?;
    let scan = git_pick_inner(&app, &url, branch.as_deref(), username.as_deref(), password.as_deref(), depth.unwrap_or(1))
        .await
        .map_err(|e| e.to_string())?;
    Ok(scan)
}

/// Cancel an in-flight git pick/clone for `url` (the UI "取消" button).
/// The backend abandons the clone wait and cleans the temp dir; the pending
/// `pick_rag_git_repo` promise resolves with a `PICK_CANCELLED` error.
#[tauri::command]
pub async fn cancel_rag_git_pick(session: State<'_, SessionState>, url: String) -> Result<bool, String> {
    // Aborting a clone is a write to shared clone state (temp-dir cleanup can
    // race the admin's subsequent upload mapping) — admin-gate to match
    // pick_rag_git_repo/refresh_rag_source.
    crate::commands::config::require_admin(&session).await?;
    Ok(crate::rag::git::signal_abort_canonical(&url).await)
}

/// Shared body of `pick_rag_git_repo` (also called by the credential-retry
/// command). Clone into temp + scan + persist the credential locally.
async fn git_pick_inner(
    app: &AppHandle,
    url: &str,
    branch: Option<&str>,
    username: Option<&str>,
    password: Option<&str>,
    depth: u32,
) -> Result<RagFolderScan> {
    let result = crate::rag::git::clone_to_temp(Some(app), url, branch, username, password, depth).await?;
    // Persist the credential in the app's local credential file so future
    // refreshes re-use it without re-prompting.
    if let (Some(u), Some(p)) = (username, password) {
        if let Err(e) = crate::rag::git::store_credential_for(app, url, u, p).await {
            crate::rag::service::rag_log("warn", format!("git: store credential failed: {e}"));
        }
    }
    // A successful clone proves the source is reachable with the given
    // credentials — drop any previously recorded refresh failure so the UI
    // doesn't show a stale warning after the user fixes the source.
    service::clear_git_source_error_for(url).await;
    let mut scan = service::scan_folder_public(&result.dir, true);
    // Stamp the scan with the head commit so uploads can record it in the
    // doc source (display only).
    scan.commit = Some(result.commit);
    // Exclusion-registry match paths: the dialog checks/sets paths in the
    // global registry, and the update pipeline compares against the doc's
    // recorded original_path — which for git docs is the PERSISTENT clone
    // path (temp->persistent mapping at upload), not the temp path the scan
    // just returned. Re-stamp each file's match_path accordingly; files that
    // don't map (unexpected) keep the temp path (harmless: they'd just never
    // match the registry).
    for g in &mut scan.groups {
        for f in &mut g.files {
            if let Ok(Some((_, persisted))) =
                crate::rag::git::map_temp_to_persistent(app, &f.match_path)
            {
                f.match_path = persisted;
            }
        }
    }
    Ok(scan)
}

/// Read + decode-to-UTF-8 + chunk + embed + index a single file (by disk
/// path). The frontend loops over the picked paths, calling this once per
/// file so it can show per-file upload progress. `method` selects the import
/// method: "symlink" (default — record original_path, no copy) or "copy"
/// (copy bytes into rag/files). `None` defaults to "symlink" to match the
/// UI default.
#[tauri::command]
pub async fn upload_rag_doc(
    session: State<'_, SessionState>,
    app: AppHandle,
    file_path: String,
    tags: Vec<String>,
    method: Option<String>,
    // Data-source provenance (camelCase from the frontend): kind/label/root/
    // relPath/git{url,branch,commit}. None on legacy callers -> classified as
    // "file" at read time.
    source: Option<crate::models::rag::DocSource>,
) -> Result<(), String> {
    // Arbitrary-path read channel: the file is ingested into the index and
    // readable back via rag_search — admin-gated in multi-user mode
    // (skipAuth short-circuits, so the default desktop flow is unaffected).
    crate::commands::config::require_admin(&session).await?;
    // Git source: the scan/selection happened in the OS temp clone. Before
    // uploading, persist the repo into app-data (idempotent) and rewrite the
    // file path prefix temp->persistent so the doc's original_path (md5
    // update source + open-location target) survives temp cleanup.
    let file_path = match source.as_ref().filter(|s| s.kind == "git") {
        Some(src) => match crate::rag::git::map_temp_to_persistent(&app, &file_path) {
            Ok(Some((hash, persisted))) => {
                // First import: copy temp -> persistent (needs the temp dir
                // still present). The hash comes from the path prefix under
                // the temp root — the SAME hash clone_to_temp used (it hashes
                // the canonicalized URL, which can differ from the raw
                // frontend url, e.g. after an http->https redirect probe).
                let temp_dir = crate::rag::git::temp_dir_for(&hash);
                crate::rag::git::ensure_persisted(&app, &hash, &temp_dir).map_err(|e| e.to_string())?;
                let _ = src;
                persisted
            }
            Ok(None) => file_path,
            Err(e) => return Err(e.to_string()),
        },
        _ => file_path,
    };
    // Git imports always land as "copy" (the dialog hides the symlink/copy
    // toggle for the git data source and documents forced-copy semantics —
    // enforce it here so a stale 'symlink' UI state can't leak through).
    let method = match source.as_ref().filter(|s| s.kind == "git") {
        Some(_) => Some("copy".to_string()),
        None => method,
    };
    service::upload_one_path(&app, &file_path, tags, method, source)
        .await
        .map_err(|e| e.to_string())
}

/// Bracket an import session around the frontend's upload loop: `begin`
/// before the first `upload_rag_doc`, `end` when the loop finishes (or is
/// cancelled — the frontend's finally block always ends it). While a session
/// is active the backend defers git-source refreshes + the auto-update tick
/// and suppresses source-sync add imports (see service::IMPORT_SESSION).
#[tauri::command]
pub async fn begin_rag_import_session(
    session: State<'_, SessionState>,
) -> Result<(), String> {
    // RAG write: upload accepts arbitrary paths (read->index->search exfil channel) — admin-gated in multi-user mode.
    crate::commands::config::require_admin(&session).await?;
    service::begin_import_session();
    Ok(())
}

#[tauri::command]
pub async fn end_rag_import_session(
    session: State<'_, SessionState>,
) -> Result<(), String> {
    // RAG write: upload accepts arbitrary paths (read->index->search exfil channel) — admin-gated in multi-user mode.
    crate::commands::config::require_admin(&session).await?;
    service::end_import_session();
    Ok(())
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
    session: State<'_, SessionState>,

    app: AppHandle,
    id: String,
    mode: String,
    file_path: Option<String>,
) -> Result<u32, String> {
    // RAG write: upload accepts arbitrary paths (read->index->search exfil channel) — admin-gated in multi-user mode.
    crate::commands::config::require_admin(&session).await?;
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
pub async fn check_rag_update(
    session: State<'_, SessionState>,
    app: AppHandle,
    id: String,
) -> Result<RagUpdateCheck, String> {
    // Read of source-path state (original_path existence / md5) — gated like
    // list_rag_docs (review round 8, 2026-10-04).
    crate::commands::config::require_admin(&session).await?;
    service::check_rag_update(&app, &id)
        .await
        .map_err(|e| e.to_string())
}

/// Batch-update preview: aggregate counts (total / to_update / skipped / lost)
/// over all docs, shown in the confirm dialog before the expensive re-index
/// pass runs. No embedding, no progress events.
#[tauri::command]
pub async fn preview_batch_update(
    session: State<'_, SessionState>,
    app: AppHandle,
) -> Result<BatchPreview, String> {
    crate::commands::config::require_admin(&session).await?;
    service::preview_batch_update(&app)
        .await
        .map_err(|e| e.to_string())
}

/// Query the last recorded git-source refresh failures (no network I/O).
/// Backs the batch-update progress dialog's warning icon: the user clicks it
/// to see WHICH repos are failing and WHY (auth vs address/network).
#[tauri::command]
pub async fn get_git_source_errors(
    session: State<'_, SessionState>,
    app: AppHandle,
) -> Result<Vec<crate::models::rag::GitSourceError>, String> {
    crate::commands::config::require_admin(&session).await?;
    Ok(service::get_git_source_errors(&app).await)
}

/// Run the batch update in the background: re-index every doc whose source
/// changed (md5 differs, or legacy no-md5 treated as changed). Emits
/// `rag://batch-update-progress` per doc. Lost/legacy docs are skipped.
/// Returns immediately; work runs on a spawned task. Guarded against double
/// triggers (a second call while one is running is a no-op).
#[tauri::command]
pub async fn batch_update_rag_docs(
    session: State<'_, SessionState>,
app: AppHandle) -> Result<(), String> {
    // RAG write: upload accepts arbitrary paths (read->index->search exfil channel) — admin-gated in multi-user mode.
    crate::commands::config::require_admin(&session).await?;
    service::batch_update_rag_docs(app);
    Ok(())
}

/// Source-level manual update (the tree view's per-source refresh button):
/// git — force-refresh the repo's persistent clone; then (both kinds) run the
/// source-level sync scoped to this source (import added files / remove docs
/// whose file vanished, same gates as the batch flow) and re-index the docs
/// of this source whose md5 changed. Guarded by the same BATCH_UPDATE_RUNNING
/// CAS as the batch flow — the frontend's shared progress dialog works for
/// both. Returns (added, removed, updated) counts.
#[tauri::command]
pub async fn refresh_rag_source(
    session: State<'_, SessionState>,
    app: AppHandle,
    kind: String,
    url: Option<String>,
    root: Option<String>,
) -> Result<(u32, u32, u32), String> {
    // Admin-gated: this is a WRITE operation of the same class as
    // batch_update_rag_docs — for git sources it force-refreshes the
    // persistent clone using the admin-stored credentials, then imports/removes
    // docs and re-indexes. Ungated, any non-admin session could trigger an
    // authenticated pull of a private repo and read the indexed content back
    // via search/get (the exfil channel the upload gating exists to block).
    crate::commands::config::require_admin(&session).await?;
    service::refresh_source_update(
        &app,
        service::SourceUpdateTarget {
            kind,
            url: url.unwrap_or_default(),
            root: root.unwrap_or_default(),
        },
    )
    .await
    .map_err(|e| e.to_string())
}

/// Source-scoped batch preview: the confirm dialog counts for the tree view's
/// per-source refresh button (same shape as preview_batch_update, but scoped
/// to one source; git targets are force-refreshed first so the counts reflect
/// the remote state).
#[tauri::command]
pub async fn preview_rag_source_update(
    session: State<'_, SessionState>,
    app: AppHandle,
    kind: String,
    url: Option<String>,
    root: Option<String>,
) -> Result<BatchPreview, String> {
    // Same gate as refresh_rag_source: the git path force-refreshes with
    // stored credentials before counting.
    crate::commands::config::require_admin(&session).await?;
    service::preview_source_update(
        &app,
        service::SourceUpdateTarget {
            kind,
            url: url.unwrap_or_default(),
            root: root.unwrap_or_default(),
        },
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_rag_doc(
    session: State<'_, SessionState>,
app: AppHandle, id: String) -> Result<(), String> {
    // RAG write: upload accepts arbitrary paths (read->index->search exfil channel) — admin-gated in multi-user mode.
    crate::commands::config::require_admin(&session).await?;
    service::delete_doc(&app, &id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rag_search_command(
    session: State<'_, SessionState>,
    query: String,
    tags: Vec<String>,
) -> Result<Vec<RagSearchResult>, String> {
    // Search results carry chunk TEXT — the same read-exfil channel the
    // get_rag_doc/upload gates protect. Ungated here defeated all of them in
    // multi-user mode (any non-admin could read every indexed doc).
    crate::commands::config::require_admin(&session).await?;
    service::search(query, tags).await.map_err(|e| e.to_string())
}

/// List/search distinct tags in the RAG library. `search_key` filters by
/// case-insensitive substring (any match); empty returns all tags. Each tag
/// is returned with its file count.
#[tauri::command]
pub async fn rag_tag_search(
    session: State<'_, SessionState>,
    search_key: Vec<String>,
) -> Result<Vec<RagTagStat>, String> {
    // Tag inventory leaks what's indexed — align with the other rag reads
    // (review round 9).
    crate::commands::config::require_admin(&session).await?;
    service::list_tags(search_key).await.map_err(|e| e.to_string())
}

/// Paginated tag search for the frontend's searchable dropdowns. Returns one
/// page of tags (SQL-level LIKE + LIMIT/OFFSET) + the total matching count,
/// so the UI can fetch more pages on demand. `page` is 0-based.
#[tauri::command]
pub async fn rag_tag_search_paged(
    session: State<'_, SessionState>,
    search_key: String,
    page: u32,
    page_size: u32,
) -> Result<RagTagPage, String> {
    crate::commands::config::require_admin(&session).await?;
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
    session: State<'_, SessionState>,
    app: AppHandle,
    search_key: String,
    tags: Vec<String>,
    page: u32,
    page_size: u32,
) -> Result<crate::models::rag::RagDocPage, String> {
    crate::commands::config::require_admin(&session).await?;
        service::search_docs_paged(&app, search_key, tags, page, page_size)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_rag_tags(
    session: State<'_, SessionState>,
app: AppHandle, id: String, tags: Vec<String>) -> Result<(), String> {
    // RAG write: upload accepts arbitrary paths (read->index->search exfil channel) — admin-gated in multi-user mode.
    crate::commands::config::require_admin(&session).await?;
    service::set_doc_tags(&app, &id, tags).await.map_err(|e| e.to_string())
}

/// Read the exclusion registry (`config_json.rag.excludedPaths`) — absolute
/// paths (or directories) the update pipeline must ignore. Backs the import
/// dialog's per-file ban toggles (initial state) and the doc-list badge.
#[tauri::command]
pub async fn list_rag_excluded_paths(session: State<'_, SessionState>) -> Result<Vec<String>, String> {
    // Leaks admin-configured absolute filesystem paths — admin-gated in
    // multi-user mode (same face as get_rag_git_clone_dir; review round 9).
    crate::commands::config::require_admin(&session).await?;
    Ok(service::list_excluded_paths().await)
}

/// Replace the exclusion registry wholesale. The import dialog persists its
/// dialog-local toggles as a full diff (removed + added paths) in one call;
/// the doc-list toggle sends the current list plus/minus one path. Returns
/// the stored list (deduped/trimmed).
#[tauri::command]
pub async fn set_rag_excluded_paths(
    session: State<'_, SessionState>,
paths: Vec<String>) -> Result<Vec<String>, String> {
    // RAG write: upload accepts arbitrary paths (read->index->search exfil channel) — admin-gated in multi-user mode.
    crate::commands::config::require_admin(&session).await?;
    service::set_excluded_paths(paths).await.map_err(|e| e.to_string())
}

/// The persistent clone directory for a git source URL. The tree view's
/// per-source exclude button uses it as the registry entry (a dir entry
/// prefix-covers every doc of that repo), keeping git-source exclusion on the
/// same path-based mechanism as folder exclusion.
#[tauri::command]
pub async fn get_rag_git_clone_dir(
    session: State<'_, SessionState>,
    app: AppHandle,
    url: String,
) -> Result<String, String> {
    // Leaks the persistent clone's absolute path — admin-only (review round 8).
    crate::commands::config::require_admin(&session).await?;
    let dir = crate::rag::git::persistent_repo_dir_for_url(&app, &url)
        .await
        .ok_or_else(|| "git source has no persistent clone yet".to_string())?;
    Ok(dir.to_string_lossy().into_owned())
}

/// Set (alias non-empty) or clear (alias empty) a folder/git source's display
/// alias. `identity` = folder absolute root path / git canonical URL — the
/// same values the doc list already carries as sourceRoot / gitUrl.
#[tauri::command]
pub async fn set_rag_source_alias(
    session: State<'_, SessionState>,

    kind: String,
    identity: String,
    alias: String,
) -> Result<(), String> {
    // RAG write: upload accepts arbitrary paths (read->index->search exfil channel) — admin-gated in multi-user mode.
    crate::commands::config::require_admin(&session).await?;
    service::set_source_alias(kind, identity, alias)
        .await
        .map_err(|e| e.to_string())
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
pub async fn save_rag_settings(
    session: State<'_, SessionState>,
app: AppHandle, settings: RagSettings) -> Result<(), String> {
    // RAG write: upload accepts arbitrary paths (read->index->search exfil channel) — admin-gated in multi-user mode.
    crate::commands::config::require_admin(&session).await?;
    // Persist + re-arm the auto-update timer so interval/toggle changes take
    // effect immediately (the running loop wakes on the generation bump).
    service::save_settings_and_rearm(&app, settings)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn open_rag_file_location(
    session: State<'_, SessionState>,
    app: AppHandle,
    id: String,
    target: Option<String>,
) -> Result<(), String> {
    // Arbitrary-path open (opens the recorded original_path) — gated like
    // skills' open_path_in_explorer (review round 8, 2026-10-04).
    crate::commands::config::require_admin(&session).await?;
    service::open_file_location(&app, &id, target)
        .await
        .map_err(|e| e.to_string())
}

/// Open a doc's ORIGINAL source file with the OS default application
/// ("view source file" for PDF/Office/image imports).
#[tauri::command]
pub async fn open_rag_doc_source_file(
    session: State<'_, SessionState>,
    app: AppHandle,
    id: String,
) -> Result<(), String> {
    // Same arbitrary-path open surface as open_rag_file_location.
    crate::commands::config::require_admin(&session).await?;
    service::open_doc_source(&app, &id).await.map_err(|e| e.to_string())
}

/// Re-embed every uploaded doc with the currently-loaded model, after a model
/// swap recreated the vector table (embedding dim changed). Drives the same
/// upload overlay the frontend uses for imports: emits `rag://reindex-progress`
/// per doc (file-level bar) and reuses `reindex_doc`'s `rag://upload-progress`
/// (char-level bar). Clears `needs_reindex` when done. No-op (returns 0) if no
/// docs exist.
#[tauri::command]
pub async fn rag_reindex_all(
    session: State<'_, SessionState>,
app: AppHandle) -> Result<usize, String> {
    // RAG write: upload accepts arbitrary paths (read->index->search exfil channel) — admin-gated in multi-user mode.
    crate::commands::config::require_admin(&session).await?;
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
pub async fn rag_select_model(
    session: State<'_, SessionState>,
    app: AppHandle,
    size: String,
) -> Result<crate::models::rag::RagStatus, String> {
    // Admin-gated: persists a GLOBAL config key (config_json.rag.model) and
    // restarts the RAG runtime for every user.
    crate::commands::config::require_admin(&session).await?;
    service::select_model(&app, &size).await.map_err(|e| e.to_string())
}

/// Download a model size via its `download.url` (a GGUF model file). Streams
/// with progress on `rag://model-download` into
/// `<app_data>/rag/models/<family>/<size>/`. After success the size is ready.
#[tauri::command]
pub async fn rag_download_model(
    session: State<'_, SessionState>,
    app: AppHandle,
    size: String,
) -> Result<(), String> {
    // Admin-gated: triggers an external network download into the app data dir.
    crate::commands::config::require_admin(&session).await?;
    service::download_model(&app, &size).await.map_err(|e| e.to_string())
}
