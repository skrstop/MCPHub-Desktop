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
