import { apiGet, apiPost, apiPut } from '../utils/fetchInterceptor';
import {
  RagDoc,
  RagDocInfo,
  RagPickedFile,
  RagSettings,
  RagSearchResult,
  RagStatus,
  RagTagStat,
  RagModelLimits,
  RagModelInfo,
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
 * Upload a single plain-text document by disk path. The backend reads the
 * bytes from disk, detects + converts the encoding to UTF-8, then chunks +
 * embeds + indexes. The frontend loops over the picked paths, calling this
 * once per file for per-file progress.
 */
export const uploadRagDoc = async (filePath: string, tags: string[] = []): Promise<void> => {
  const response: ApiResponse = await apiPost('/rag/docs/upload', { filePath, tags });
  if (!response.success) throw new Error(response.message || 'Failed to upload RAG doc');
};

/** Delete a document: removes its files + vector DB records. */
export const deleteRagDoc = async (id: string): Promise<void> => {
  const response: ApiResponse = await apiPost('/rag/docs/delete', { id });
  if (!response.success) throw new Error(response.message || 'Failed to delete RAG doc');
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

/** List/search distinct tags in the RAG library. Empty `searchKey` returns all. */
export const ragTagSearch = async (searchKey: string[] = []): Promise<RagTagStat[]> => {
  const response: ApiResponse<RagTagStat[]> = await apiPost('/rag/tags/search', { searchKey });
  if (!response.success) throw new Error(response.message || 'Failed to search RAG tags');
  return response.data || [];
};

/** Get RAG search settings (weights + max results). */
export const getRagSettings = async (): Promise<RagSettings> => {
  const response: ApiResponse<RagSettings> = await apiGet('/rag/settings');
  if (!response.success) throw new Error(response.message || 'Failed to get RAG settings');
  return (
    response.data ?? { vectorWeight: 0.5, keywordWeight: 0.5, maxResults: 20, scoreThreshold: 0, chunkSize: 512, chunkOverlap: 100 }
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
