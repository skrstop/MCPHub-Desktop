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

/** Slice a fetched history page into display rows + a next-page flag.
 *  The page fetch requests `pageSize + 1` rows; an extra row is the only
 *  reliable "more pages" signal short of a count query, and must be sliced
 *  off before display. */
export function pageSlice<T>(raw: T[], pageSize: number): { rows: T[]; hasMore: boolean } {
  return { rows: raw.slice(0, pageSize), hasMore: raw.length > pageSize };
}
