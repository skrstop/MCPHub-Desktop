import { apiGet, apiPost, apiPut } from '../utils/fetchInterceptor';
import {
  RagDoc,
  RagDocInfo,
  RagChunk,
  RagPickedFile,
  RagSettings,
  RagSearchResult,
  RagStatus,
  RagTagStat,
  RagTagPage,
  RagDocPage,
  RagModelLimits,
  RagModelInfo,
  RagUpdateCheck,
  BatchPreview,
  ApiResponse,
} from '@/types';

/** RAG runtime status (switch state). */
export const ragStatus = async (): Promise<RagStatus> => {
  const response: ApiResponse<RagStatus> = await apiGet('/rag/status');
  if (!response.success) throw new Error(response.message || 'Failed to get RAG status');
  return response.data ?? { enabled: false, initializing: false };
};

/** Enable/disable RAG. Enabling blocks until the model + vector DB are ready. */
export const ragToggle = async (enabled: boolean): Promise<RagStatus> => {
  const response: ApiResponse<RagStatus> = await apiPost('/rag/toggle', { enabled });
  if (!response.success) throw new Error(response.message || 'Failed to toggle RAG');
  return response.data ?? { enabled: false, initializing: false };
};

/** List all documents (metadata only). Works with RAG off. */
export const listRagDocs = async (): Promise<RagDocInfo[]> => {
  const response: ApiResponse<RagDocInfo[]> = await apiGet('/rag/docs');
  if (!response.success) throw new Error(response.message || 'Failed to list RAG docs');
  return response.data || [];
};

/** Get the full content of a document (for the View dialog). */
export const getRagDoc = async (id: string): Promise<RagDoc | null> => {
  const response: ApiResponse<RagDoc | null> = await apiGet(`/rag/docs/${encodeURIComponent(id)}`);
  if (!response.success) throw new Error(response.message || 'Failed to get RAG doc');
  return response.data ?? null;
};

/** Get a document's chunks (index + text, no embeddings) for the "view
 *  chunks" dialog. Requires RAG enabled (chunks live in lancedb). */
export const getRagChunks = async (id: string): Promise<RagChunk[]> => {
  const response: ApiResponse<RagChunk[]> = await apiPost('/rag/docs/chunks', { id });
  if (!response.success) throw new Error(response.message || 'Failed to get RAG chunks');
  return response.data ?? [];
};

/**
 * Open the OS multi-file picker (plain-text filter). Returns the chosen paths
 * + display names — no file bytes cross the IPC boundary; the backend reads
 * from disk at upload time (handles large files without OOM).
 */
export const pickRagFiles = async (): Promise<RagPickedFile[]> => {
  const response: ApiResponse<RagPickedFile[]> = await apiPost('/rag/docs/pick', {});
  if (!response.success) throw new Error(response.message || 'Failed to pick files');
  return response.data || [];
};

/**
 * Open the OS folder picker, scan the folder's immediate (non-recursive) file
 * children, and return them as import candidates — same shape as pickRagFiles
 * so the rest of the upload pipeline (per-file upload + progress) is identical.
 * Hidden files + sub-directories are skipped by the backend.
 */
export const pickRagFolder = async (): Promise<RagPickedFile[]> => {
  const response: ApiResponse<RagPickedFile[]> = await apiPost('/rag/docs/pick-folder', {});
  if (!response.success) throw new Error(response.message || 'Failed to pick folder');
  return response.data || [];
};

/**
 * Upload a single plain-text document by disk path. The backend reads the
 * bytes from disk, detects + converts the encoding to UTF-8, then chunks +
 * embeds + indexes. The frontend loops over the picked paths, calling this
 * once per file for per-file progress. `method` selects the import method:
 * "symlink" (default — record original_path, no copy) or "copy".
 */
export const uploadRagDoc = async (
  filePath: string,
  tags: string[] = [],
  method: 'symlink' | 'copy' = 'symlink',
): Promise<void> => {
  const response: ApiResponse = await apiPost('/rag/docs/upload', { filePath, tags, method });
  if (!response.success) throw new Error(response.message || 'Failed to upload RAG doc');
};

/** Delete a document: removes its files + vector DB records. For "symlink"
 *  docs the original file is left untouched (only meta + vectors removed). */
export const deleteRagDoc = async (id: string): Promise<void> => {
  const response: ApiResponse = await apiPost('/rag/docs/delete', { id });
  if (!response.success) throw new Error(response.message || 'Failed to delete RAG doc');
};

/** Update an existing document in place.
 *  - mode='original': re-read the recorded original_path + re-index (the "from
 *    original" button). filePath is ignored.
 *  - mode='file': read a freshly-picked filePath + overwrite (the "manual
 *    upload" button). The new file becomes the recorded original_path.
 *  Returns the new chunk count. Requires RAG enabled (re-embeds). */
export const updateRagDoc = async (
  id: string,
  opts: { mode: 'original' | 'file'; filePath?: string },
): Promise<number> => {
  const response: ApiResponse<number> = await apiPost('/rag/docs/update', {
    id,
    mode: opts.mode,
    filePath: opts.filePath ?? '',
  });
  if (!response.success) throw new Error(response.message || 'Failed to update RAG doc');
  return response.data ?? 0;
};

/** Single-doc update check: classify the recorded original's state (lost /
 *  changed / no-change / legacy-no-path) so the UpdateDialog renders the right
 *  branch. Cheap (one md5 of the source, no embedding). */
export const checkRagUpdate = async (id: string): Promise<RagUpdateCheck> => {
  const response: ApiResponse<RagUpdateCheck> = await apiPost('/rag/docs/check-update', { id });
  if (!response.success) throw new Error(response.message || 'Failed to check RAG update');
  return (
    response.data ?? {
      method: '',
      hasOriginalPath: false,
      originalExists: false,
      hasMd5: false,
      originalChanged: false,
      lostOriginal: false,
    }
  );
};

/** Batch-update preview: aggregate counts (total / toUpdate / skipped / lost)
 *  over all docs, shown in the confirm dialog before the expensive re-index
 *  pass runs. No embedding. */
export const previewBatchUpdate = async (): Promise<BatchPreview> => {
  const response: ApiResponse<BatchPreview> = await apiPost('/rag/docs/batch-preview', {});
  if (!response.success) throw new Error(response.message || 'Failed to preview batch update');
  return response.data ?? { total: 0, toUpdate: 0, skipped: 0, lost: 0 };
};

/** Run the batch update in the background: re-index every doc whose source
 *  changed. Returns immediately; progress arrives via the
 *  `rag://batch-update-progress` event. Guarded against double triggers. */
export const batchUpdateRagDocs = async (): Promise<void> => {
  const response: ApiResponse = await apiPost('/rag/docs/batch-update', {});
  if (!response.success) throw new Error(response.message || 'Failed to start batch update');
};

/** Set the absolute tag list for a document (re-indexes its chunks). */
export const setRagTags = async (id: string, tags: string[]): Promise<void> => {
  const response: ApiResponse = await apiPost('/rag/docs/set-tags', { id, tags });
  if (!response.success) throw new Error(response.message || 'Failed to set RAG tags');
};

/** Run a similarity search. Returns fragments + scores. Optional tag filter. */
export const searchRagDocs = async (query: string, tags: string[] = []): Promise<RagSearchResult[]> => {
  const response: ApiResponse<RagSearchResult[]> = await apiPost('/rag/search', { query, tags });
  if (!response.success) throw new Error(response.message || 'Failed to search RAG docs');
  return response.data || [];
};

/** List/search distinct tags in the RAG library. Empty `searchKey` returns all.
 *  Unbounded — kept for the MCP tool / legacy callers; the UI dropdowns use
 *  `ragTagSearchPaged` instead. */
export const ragTagSearch = async (searchKey: string[] = []): Promise<RagTagStat[]> => {
  const response: ApiResponse<RagTagStat[]> = await apiPost('/rag/tags/search', { searchKey });
  if (!response.success) throw new Error(response.message || 'Failed to search RAG tags');
  return response.data || [];
};

/** Paginated tag search backing the searchable dropdowns. `page` is 0-based.
 *  Returns one page of items + the total matching count so the UI can load
 *  more pages / stop fetching. */
export const ragTagSearchPaged = async (
  searchKey: string,
  page: number,
  pageSize: number,
): Promise<RagTagPage> => {
  const response: ApiResponse<RagTagPage> = await apiPost('/rag/tags/search-paged', {
    searchKey,
    page,
    pageSize,
  });
  if (!response.success) throw new Error(response.message || 'Failed to search RAG tags');
  return response.data ?? { items: [], total: 0, page, pageSize };
};

/** Paginated document search backing the file list's toolbar (name substring +
 *  ANY-match tag filter), executed in SQL over the rag_docs mirror table.
 *  `page` is 0-based; returns one page of enriched RagDocInfo + the total
 *  matching count so the UI can load more pages / stop fetching. */
export const ragDocSearchPaged = async (
  searchKey: string,
  tags: string[],
  page: number,
  pageSize: number,
): Promise<RagDocPage> => {
  const response: ApiResponse<RagDocPage> = await apiPost('/rag/docs/search-paged', {
    searchKey,
    tags,
    page,
    pageSize,
  });
  if (!response.success) throw new Error(response.message || 'Failed to search RAG docs');
  return response.data ?? { items: [], total: 0, page, pageSize };
};

/** Get RAG search settings (weights + max results). */
export const getRagSettings = async (): Promise<RagSettings> => {
  const response: ApiResponse<RagSettings> = await apiGet('/rag/settings');
  if (!response.success) throw new Error(response.message || 'Failed to get RAG settings');
  return (
    response.data ?? { vectorWeight: 0.9, keywordWeight: 0.1, maxResults: 20, scoreThreshold: 0.65, chunkSize: 0, chunkOverlap: 0 }
  );
};

/** Persist RAG search settings. */
export const saveRagSettings = async (settings: RagSettings): Promise<void> => {
  const response: ApiResponse = await apiPut('/rag/settings', settings);
  if (!response.success) throw new Error(response.message || 'Failed to save RAG settings');
};

/** Model context window (tokens), to cap chunk_size in the UI. */
export const getRagModelLimits = async (): Promise<RagModelLimits> => {
  const response: ApiResponse<RagModelLimits> = await apiGet('/rag/model-limits');
  if (!response.success) throw new Error(response.message || 'Failed to get RAG model limits');
  return response.data ?? { maxContext: 2048 };
};

/** The app-level RAG MCP tools (name/description/inputSchema). Empty if RAG off. */
export const getRagTools = async (): Promise<Record<string, unknown>[]> => {
  const response: ApiResponse<Record<string, unknown>[]> = await apiGet('/rag/tools');
  if (!response.success) throw new Error(response.message || 'Failed to get RAG tools');
  return response.data ?? [];
};

/** Reveal a document's file location in the OS file manager. */
export const openRagFileLocation = async (id: string): Promise<void> => {
  const response: ApiResponse = await apiPost('/rag/open-location', { id });
  if (!response.success) throw new Error(response.message || 'Failed to open file location');
};

/**
 * Re-embed every uploaded doc with the currently-loaded model, after a model
 * swap recreated the vector table (embedding dim changed). One-shot: the
 * backend emits `rag://reindex-progress` (file-level bar) and reuses
 * `rag://upload-progress` (char-level bar) per doc. Returns the count of docs
 * successfully re-embedded.
 */
export const reindexAllRag = async (): Promise<number> => {
  const response: ApiResponse<number> = await apiPost('/rag/reindex-all', {});
  if (!response.success) throw new Error(response.message || 'Failed to reindex RAG docs');
  return response.data ?? 0;
};

/** List available model sizes (ready / downloadable) for the dropdown. */
export const listRagModels = async (): Promise<RagModelInfo[]> => {
  const response: ApiResponse<RagModelInfo[]> = await apiGet('/rag/models');
  if (!response.success) throw new Error(response.message || 'Failed to list RAG models');
  return response.data || [];
};

/** The currently-selected model size (or null if none chosen yet). */
export const currentRagModel = async (): Promise<string | null> => {
  const response: ApiResponse<string | null> = await apiGet('/rag/model');
  if (!response.success) throw new Error(response.message || 'Failed to get current RAG model');
  return response.data ?? null;
};

/** Select a model size: persist + auto-restart RAG with the new model. Returns
 *  the post-restart status (with needsReindex if the dim changed). */
export const selectRagModel = async (size: string): Promise<RagStatus> => {
  const response: ApiResponse<RagStatus> = await apiPost('/rag/select-model', { size });
  if (!response.success) throw new Error(response.message || 'Failed to select RAG model');
  return response.data ?? { enabled: false, initializing: false };
};

/** Stream-download a model .zip (from its download.url) + extract. Emits
 *  `rag://model-download` progress events. Resolves when extracted + ready. */
export const downloadRagModel = async (size: string): Promise<void> => {
  const response: ApiResponse = await apiPost('/rag/download-model', { size });
  if (!response.success) throw new Error(response.message || 'Failed to download RAG model');
};
