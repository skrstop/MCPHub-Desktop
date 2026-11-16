import { describe, expect, it } from 'vitest';
import {
  DECISION_TYPES,
  decisionStatusKey,
  decisionTypeLabelKey,
  formatProbability,
  formatScore,
  pageSlice,
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

describe('pageSlice', () => {
  const row = (i: number) => ({ id: `id-${i}`, createdAt: '' });

  it('slices the extra probe row off and reports hasMore', () => {
    const raw = Array.from({ length: 4 }, (_, i) => row(i));
    const { rows, hasMore } = pageSlice(raw, 3);
    expect(rows.map((r) => r.id)).toEqual(['id-0', 'id-1', 'id-2']);
    expect(hasMore).toBe(true);
  });

  it('reports no next page when the fetch is shorter than pageSize+1', () => {
    const raw = Array.from({ length: 3 }, (_, i) => row(i));
    const { rows, hasMore } = pageSlice(raw, 3);
    expect(rows).toHaveLength(3);
    expect(hasMore).toBe(false);
  });

  it('returns empty rows and no next page for an empty fetch', () => {
    expect(pageSlice([], 50)).toEqual({ rows: [], hasMore: false });
  });

  it('does not mutate the input array', () => {
    const raw = Array.from({ length: 2 }, (_, i) => row(i));
    const copy = [...raw];
    pageSlice(raw, 1);
    expect(raw).toEqual(copy);
  });

  it('treats pageSize 0 as an empty page with hasMore when rows exist', () => {
    const raw = Array.from({ length: 2 }, (_, i) => row(i));
    expect(pageSlice(raw, 0)).toEqual({ rows: [], hasMore: true });
    expect(pageSlice([], 0)).toEqual({ rows: [], hasMore: false });
  });

  it('handles a fetch far longer than pageSize+1 (defensive cap)', () => {
    const raw = Array.from({ length: 120 }, (_, i) => row(i));
    const { rows, hasMore } = pageSlice(raw, 50);
    expect(rows).toHaveLength(50);
    expect(hasMore).toBe(true);
  });
});
