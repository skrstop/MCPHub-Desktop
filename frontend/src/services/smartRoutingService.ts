import { apiGet, apiPost } from '../utils/fetchInterceptor';

export interface SmartRoutingPerformanceData {
  enabled: boolean;
  /** True while a reindex pass is running anywhere in this hub process. */
  reindexing: boolean;
  config: {
    provider: string;
    model: string | null;
    configuredDimensions: number | null;
  };
  database: {
    connected: boolean;
    healthy: boolean;
    lastError: string | null;
  };
  vectorStore: {
    available: boolean;
    totalRows: number;
    toolRows: number;
    serverRows: number;
    distinctServers: number;
    dimensions: number | null;
    byModel: Array<{ model: string; toolCount: number; serverCount: number }>;
    byServer: Array<{ serverName: string; toolCount: number }>;
    oldestUpdatedAt: string | null;
    newestUpdatedAt: string | null;
  };
  coverage: {
    totalServers: number;
    connectedServers: number;
    personalCredentialServers: number;
    indexedServers: number;
    missingIndexServerCount: number;
    missingIndexServers: string[];
  };
}

export interface SmartRoutingReindexResult {
  syncedServers: number;
  failedServers: number;
  skippedServers: number;
  totalTools: number;
  results: Array<{
    serverName: string;
    toolCount: number;
    ok: boolean;
    skipped?: boolean;
    error?: string;
    principals?: string[];
  }>;
}

export interface SmartRoutingApiResponse<T> {
  success: boolean;
  data: T;
  error?: string;
  // Dashboard error responses carry `message` (e.g. the 403 admin gate and the
  // 409 concurrency guard), so surface it when `error` is absent.
  message?: string;
}

export const fetchSmartRoutingPerformance = async (): Promise<SmartRoutingPerformanceData> => {
  const response = await apiGet<SmartRoutingApiResponse<SmartRoutingPerformanceData>>(
    '/smart-routing/performance',
  );
  if (!response.success) {
    throw new Error(response.error || response.message || 'Failed to load smart routing performance');
  }
  return response.data;
};

export const reindexSmartRouting = async (): Promise<SmartRoutingReindexResult> => {
  const response = await apiPost<SmartRoutingApiResponse<SmartRoutingReindexResult>>(
    '/smart-routing/reindex',
  );
  if (!response.success) {
    throw new Error(response.error || response.message || 'Failed to rebuild smart routing index');
  }
  return response.data;
};

/** Participants that a smart reindex will cover (enabled + connected-or-
 *  on-demand + non-empty tools + builtin). Read-only count used by the
 *  model-swap dialog's confirm-phase meta ("N 个服务器"). */
export interface SmartReindexParticipants {
  servers: number;
  tools: number;
}

/** Totals a smart reindex will cover — the model-swap dialog's confirm meta
 *  "N 个服务器 M 个工具" (both are totals needing reload). */
export const getSmartReindexParticipants = async (): Promise<SmartReindexParticipants> => {
  const response = await apiGet<SmartRoutingApiResponse<{ 0: number; 1: number }>>(
    '/smart-routing/reindex-participants',
  );
  if (!response.success) {
    throw new Error(response.error || response.message || 'Failed to load smart routing participants');
  }
  // Rust tuple arrives as a JSON array [servers, tools].
  const d = response.data as unknown;
  if (Array.isArray(d)) return { servers: Number(d[0]) || 0, tools: Number(d[1]) || 0 };
  const o = (d ?? {}) as { servers?: number; tools?: number };
  return { servers: o.servers ?? 0, tools: o.tools ?? 0 };
};
