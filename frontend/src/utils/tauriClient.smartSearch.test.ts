import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('./fetchInterceptor', () => ({
  apiGet: vi.fn(),
  apiPost: vi.fn(),
  apiPut: vi.fn(),
  apiDelete: vi.fn(),
}));

import { mapRestToCommand, transformTauriResponse } from './tauriClient';

describe('mapRestToCommand: POST /smart-routing/search', () => {
  it('maps to smart_routing_search with the payload wrapper', () => {
    const r = mapRestToCommand('POST', '/smart-routing/search', {
      query: 'deploy',
      scope: 'group:uuid-1',
      limit: 20,
    });
    expect(r.command).toBe('smart_routing_search');
    expect(r.args).toEqual({
      payload: { query: 'deploy', scope: 'group:uuid-1', limit: 20 },
    });
  });

  it('defaults scope to "all" and query to "" when body fields are missing', () => {
    const r = mapRestToCommand('POST', '/smart-routing/search', {});
    expect(r.args).toEqual({
      payload: { query: '', scope: 'all', limit: undefined },
    });
  });

  it('handles a null body', () => {
    const r = mapRestToCommand('POST', '/smart-routing/search', null);
    expect(r.command).toBe('smart_routing_search');
    expect(r.args).toEqual({
      payload: { query: '', scope: 'all', limit: undefined },
    });
  });

  it('throws loudly for other methods on the same path (no silent fallback)', () => {
    expect(() => mapRestToCommand('GET', '/smart-routing/search')).toThrow(
      'Unmapped route: GET /smart-routing/search',
    );
  });
});

describe('transformTauriResponse: smart_routing_search', () => {
  beforeEach(() => vi.clearAllMocks());

  it('wraps a result array as {success, data}', () => {
    const rows = [
      {
        serverName: 'playwright',
        toolName: 'click',
        contentId: 'c1',
        contentType: 'tool',
        textContent: null,
        metadata: { description: 'd' },
        score: 1.42,
      },
    ];
    expect(transformTauriResponse('smart_routing_search', rows)).toEqual({
      success: true,
      data: rows,
    });
  });

  it('coerces a non-array result to an empty array', () => {
    expect(transformTauriResponse('smart_routing_search', null)).toEqual({
      success: true,
      data: [],
    });
    expect(transformTauriResponse('smart_routing_search', 'garbage')).toEqual({
      success: true,
      data: [],
    });
  });

  it('preserves an empty result array', () => {
    expect(transformTauriResponse('smart_routing_search', [])).toEqual({
      success: true,
      data: [],
    });
  });
});
