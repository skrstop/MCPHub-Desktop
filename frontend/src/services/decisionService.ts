import { apiDelete, apiGet, apiPost, apiPut } from '../utils/fetchInterceptor';
import type { ModelDownloadProgress } from '@/components/ui/ModelSelector';
import type { ApiResponse, RagModelInfo } from '@/types';

/** Decision-support status (enable flag + selected model). `selectionLost` is
 *  the persisted selection when its files no longer resolve (the decide path
 *  falls back to a ready default, so the UI must surface it for re-download). */
export interface DecisionStatus {
  enabled: boolean;
  model: string | null;
  modelReady: boolean;
  selectionLost?: string | null;
}

/** One recorded decision request (row of decision_history). */
export interface DecisionHistoryEntry {
  id: string;
  createdAt: string;
  decisionType: string;
  state: string | null;
  question: string;
  options: string[];
  scores: number[];
  probabilities: number[];
  selectedIndex: number;
  selectedOption: string;
  durationMs: number | null;
  source: string | null;
  /** Selection key ("<family>/<size>") of the model that produced this row. */
  model: string | null;
}

export const decisionStatus = async (): Promise<DecisionStatus> => {
  const response: ApiResponse<DecisionStatus> = await apiGet('/decision/status');
  if (!response.success) throw new Error(response.message || 'Failed to get decision status');
  return response.data ?? { enabled: false, model: null, modelReady: false };
};

export const decisionSetEnabled = async (enabled: boolean): Promise<void> => {
  const response: ApiResponse<unknown> = await apiPut('/decision/enabled', { enabled });
  if (!response.success) throw new Error(response.message || 'Failed to toggle decision support');
};

/** Run one structured decision from the UI (recorded with source "ui"). */
export const decideDecision = async (
  request: { decisionType?: string; state: string; question: string; options: string[] },
): Promise<unknown> => {
  const response: ApiResponse<unknown> = await apiPost('/decision/decide', request);
  if (!response.success) throw new Error(response.message || 'Failed to run decision');
  return response.data;
};

/** The app-level decision MCP tool (decision_decide) — name/description/
 *  inputSchema. Always returns the catalog: the "view tools" dialog
 *  documents the tool even while decision support is off (enable gating
 *  applies to the MCP surface only). Powers the "view tools" dialog in the
 *  decision page. */
export const getDecisionTools = async (): Promise<Record<string, unknown>[]> => {
  const response: ApiResponse<Record<string, unknown>[]> = await apiGet('/decision/tools');
  if (!response.success) throw new Error(response.message || 'Failed to get decision tools');
  return response.data ?? [];
};

export const listDecisionModels = async (): Promise<RagModelInfo[]> => {
  const response: ApiResponse<RagModelInfo[]> = await apiGet('/decision/models');
  if (!response.success) throw new Error(response.message || 'Failed to list decision models');
  return response.data || [];
};

export const selectDecisionModel = async (model: string): Promise<void> => {
  const response: ApiResponse<unknown> = await apiPost('/decision/select-model', { model });
  if (!response.success) throw new Error(response.message || 'Failed to select decision model');
};

export const downloadDecisionModel = async (model: string, source?: string): Promise<void> => {
  const response: ApiResponse<unknown> = await apiPost('/decision/download-model', {
    model,
    source,
  });
  if (!response.success) throw new Error(response.message || 'Failed to start download');
};

export const decisionModelDownloadStatus = async (
  model: string,
): Promise<ModelDownloadProgress | null> => {
  const response: ApiResponse<ModelDownloadProgress | null> = await apiGet(
    `/decision/download-status?model=${encodeURIComponent(model)}`,
  );
  if (!response.success) throw new Error(response.message || 'Failed to get download status');
  return response.data ?? null;
};

export const cancelDecisionModelDownload = async (model: string): Promise<boolean> => {
  const response: ApiResponse<boolean> = await apiPost('/decision/download-cancel', { model });
  if (!response.success) throw new Error(response.message || 'Failed to cancel download');
  return response.data ?? false;
};

export const listDecisionHistory = async (
  limit = 50,
  offset = 0,
): Promise<DecisionHistoryEntry[]> => {
  const response: ApiResponse<DecisionHistoryEntry[]> = await apiGet(
    `/decision/history?limit=${limit}&offset=${offset}`,
  );
  if (!response.success) throw new Error(response.message || 'Failed to list decision history');
  return response.data || [];
};

export const getDecisionHistory = async (id: string): Promise<DecisionHistoryEntry | null> => {
  const response: ApiResponse<DecisionHistoryEntry | null> = await apiGet(
    `/decision/history/${encodeURIComponent(id)}`,
  );
  if (!response.success) throw new Error(response.message || 'Failed to get decision history');
  return response.data ?? null;
};

export const clearDecisionHistory = async (): Promise<number> => {
  const response: ApiResponse<number> = await apiDelete('/decision/history');
  if (!response.success) throw new Error(response.message || 'Failed to clear decision history');
  return response.data ?? 0;
};
