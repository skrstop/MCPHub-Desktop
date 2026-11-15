/** Pure formatting/validation helpers for the Decision Support page
 *  (决策支持). Extracted so the rendering logic stays unit-testable without
 *  the Tauri runtime. */

/** The decision types the backend accepts (official `QTYPES`). */
export const DECISION_TYPES = ['choice', 'score', 'noul'] as const;

export type DecisionTypeKey = (typeof DECISION_TYPES)[number];

/** Normalize a raw decision type string to a known i18n key suffix. Unknown
 *  values fall back to 'choice' (the backend default) so the tag never
 *  renders a raw enum leak. */
export function normalizeDecisionType(ty: string): DecisionTypeKey {
  return (DECISION_TYPES as readonly string[]).includes(ty)
    ? (ty as DecisionTypeKey)
    : 'choice';
}

/** i18n key for a decision type tag. */
export function decisionTypeLabelKey(ty: string): string {
  return `pages.decision.type_${normalizeDecisionType(ty)}`;
}

/** Probability (0-1) as a percentage string, e.g. 0.756 -> "75.6%". */
export function formatProbability(p: number): string {
  return `${(p * 100).toFixed(1)}%`;
}

/** Raw logit score fixed to 4 decimals. */
export function formatScore(s: number): string {
  return s.toFixed(4);
}

/** i18n key for the decision-model runtime status row in 【本地模型】设置
 *  (mirrors the RAG runtime's mvStatus text). Three states: running with a
 *  ready model, enabled but the model file is missing, and not running. */
export type DecisionStatusKey =
  | 'settings.decisionStatusRunning'
  | 'settings.decisionStatusNoModel'
  | 'settings.decisionStatusStopped';

export function decisionStatusKey(enabled: boolean, modelReady: boolean): DecisionStatusKey {
  if (!enabled) return 'settings.decisionStatusStopped';
  return modelReady ? 'settings.decisionStatusRunning' : 'settings.decisionStatusNoModel';
}

/** Minimal row shape mergeHistoryPage operates on (structurally satisfied by
 *  the service layer's `DecisionHistoryEntry`). */
export interface HistoryRowKey {
  id: string;
  createdAt: string;
}

/** Merge a prefix-refetch page into the loaded entries and restore the
 *  backend's ordering contract (`created_at DESC, id DESC`).
 *
 *  The page refetches the newest `limit` rows from offset 0 (offset paging
 *  would permanently skip rows inserted between fetches); rows inserted in
 *  between surface mid-list, so the merged set is re-sorted. Dedup by id —
 *  overlap between the previously loaded rows and the refetched prefix is
 *  expected. */
export function mergeHistoryPage<T extends HistoryRowKey>(
  prev: T[],
  list: T[],
): T[] {
  const seen = new Set(prev.map((e) => e.id));
  const merged = [...prev];
  for (const e of list) {
    // Dedup against prev AND within list itself (a defensive guard — the
    // backend never returns duplicate ids in one page, but the merge must
    // not depend on that).
    if (seen.has(e.id)) continue;
    seen.add(e.id);
    merged.push(e);
  }
  merged.sort((a, b) =>
    a.createdAt < b.createdAt ? 1 : a.createdAt > b.createdAt ? -1 : a.id < b.id ? 1 : -1,
  );
  return merged;
}
