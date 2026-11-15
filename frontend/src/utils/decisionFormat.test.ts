import { describe, expect, it } from 'vitest';
import {
  DECISION_TYPES,
  decisionStatusKey,
  decisionTypeLabelKey,
  formatProbability,
  formatScore,
  mergeHistoryPage,
  normalizeDecisionType,
} from './decisionFormat';

describe('normalizeDecisionType', () => {
  it('passes through known types', () => {
    expect(normalizeDecisionType('choice')).toBe('choice');
    expect(normalizeDecisionType('score')).toBe('score');
    expect(normalizeDecisionType('noul')).toBe('noul');
  });

  it('falls back to choice for unknown values', () => {
    expect(normalizeDecisionType('bogus')).toBe('choice');
    expect(normalizeDecisionType('')).toBe('choice');
    expect(normalizeDecisionType('CHOICE')).toBe('choice'); // case-sensitive enum
  });

  it('exposes exactly the official QTYPES', () => {
    expect([...DECISION_TYPES]).toEqual(['choice', 'score', 'noul']);
  });
});

describe('decisionTypeLabelKey', () => {
  it('builds the i18n key', () => {
    expect(decisionTypeLabelKey('score')).toBe('pages.decision.type_score');
  });
  it('never leaks a raw unknown enum into the key', () => {
    expect(decisionTypeLabelKey('evil')).toBe('pages.decision.type_choice');
  });
});

describe('formatProbability', () => {
  it('formats 0-1 as percentage with one decimal', () => {
    expect(formatProbability(0.756)).toBe('75.6%');
    expect(formatProbability(0)).toBe('0.0%');
    expect(formatProbability(1)).toBe('100.0%');
  });
});

describe('formatScore', () => {
  it('fixes to 4 decimals', () => {
    expect(formatScore(5.7654321)).toBe('5.7654');
    expect(formatScore(-0.5)).toBe('-0.5000');
  });
});

describe('mergeHistoryPage', () => {
  const row = (id: string, createdAt: string) => ({ id, createdAt });

  it('appends only unseen rows (dedup by id)', () => {
    const prev = [row('a', '2026-01-01 00:00:00'), row('b', '2026-01-02 00:00:00')];
    const list = [row('b', '2026-01-02 00:00:00'), row('c', '2026-01-03 00:00:00')];
    const merged = mergeHistoryPage(prev, list);
    expect(merged.map((r) => r.id)).toEqual(['c', 'b', 'a']);
  });

  it('restores time-desc order when inserts surface mid-list', () => {
    // Rows inserted between page fetches appear at the TOP of the refetched
    // prefix — the merge must re-sort, not append.
    const prev = [row('a', '2026-01-01 00:00:00')];
    const list = [row('x', '2026-01-05 00:00:00'), row('a', '2026-01-01 00:00:00'), row('b', '2026-01-02 00:00:00')];
    expect(mergeHistoryPage(prev, list).map((r) => r.id)).toEqual(['x', 'b', 'a']);
  });

  it('breaks created_at ties by id DESC (backend ORDER BY contract)', () => {
    const prev = [row('a', '2026-01-01 00:00:00')];
    const list = [row('c', '2026-01-01 00:00:00'), row('b', '2026-01-01 00:00:00'), row('a', '2026-01-01 00:00:00')];
    expect(mergeHistoryPage(prev, list).map((r) => r.id)).toEqual(['c', 'b', 'a']);
  });

  it('returns prev order untouched when the page adds nothing new', () => {
    const prev = [row('b', '2026-01-02 00:00:00'), row('a', '2026-01-01 00:00:00')];
    const list = [row('b', '2026-01-02 00:00:00'), row('a', '2026-01-01 00:00:00')];
    expect(mergeHistoryPage(prev, list).map((r) => r.id)).toEqual(['b', 'a']);
  });

  it('merges into an empty prev (initial fetch shape)', () => {
    const list = [row('b', '2026-01-02 00:00:00'), row('a', '2026-01-01 00:00:00')];
    expect(mergeHistoryPage([], list).map((r) => r.id)).toEqual(['b', 'a']);
  });

  it('dedups duplicates within the incoming list itself', () => {
    const prev = [row('a', '2026-01-01 00:00:00')];
    const list = [row('x', '2026-01-05 00:00:00'), row('x', '2026-01-05 00:00:00')];
    expect(mergeHistoryPage(prev, list).map((r) => r.id)).toEqual(['x', 'a']);
  });

  it('does not mutate the prev array', () => {
    const prev = [row('a', '2026-01-01 00:00:00')];
    const copy = [...prev];
    mergeHistoryPage(prev, [row('x', '2026-01-05 00:00:00')]);
    expect(prev).toEqual(copy);
  });
});

describe('decisionStatusKey', () => {
  it('maps the three runtime states to distinct i18n keys', () => {
    // Running: enabled + ready model → model name shown.
    expect(decisionStatusKey(true, true)).toBe('settings.decisionStatusRunning');
    // Enabled but model missing → actionable "not ready" copy.
    expect(decisionStatusKey(true, false)).toBe('settings.decisionStatusNoModel');
    // Disabled → stopped copy regardless of the model state.
    expect(decisionStatusKey(false, true)).toBe('settings.decisionStatusStopped');
    expect(decisionStatusKey(false, false)).toBe('settings.decisionStatusStopped');
  });
});
