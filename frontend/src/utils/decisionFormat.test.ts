import { describe, expect, it } from 'vitest';
import {
  DECISION_TYPES,
  decisionTypeLabelKey,
  formatProbability,
  formatScore,
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
