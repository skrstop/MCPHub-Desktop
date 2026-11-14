import { describe, it, expect } from 'vitest';
import { mapRestToCommand, transformTauriResponse } from './tauriClient';

describe('mapRestToCommand: /decision routes', () => {
  it('maps GET /decision/tools to decision_tools with no args', () => {
    const r = mapRestToCommand('GET', '/decision/tools');
    expect(r.command).toBe('decision_tools');
    expect(r.args).toEqual({});
  });

  it('maps PUT /decision/enabled with the enabled flag (defaults false)', () => {
    expect(mapRestToCommand('PUT', '/decision/enabled', { enabled: true })).toEqual({
      command: 'decision_set_enabled',
      args: { enabled: true },
    });
    expect(mapRestToCommand('PUT', '/decision/enabled', null)).toEqual({
      command: 'decision_set_enabled',
      args: { enabled: false },
    });
  });

  it('maps POST /decision/decide wrapping the body as request', () => {
    const body = { state: 's', question: 'q', options: ['a', 'b'] };
    expect(mapRestToCommand('POST', '/decision/decide', body)).toEqual({
      command: 'decision_decide',
      args: { request: body },
    });
    expect(mapRestToCommand('POST', '/decision/decide', null)).toEqual({
      command: 'decision_decide',
      args: { request: {} },
    });
  });

  it('maps POST /decision/download-model with model + optional source', () => {
    expect(
      mapRestToCommand('POST', '/decision/download-model', { model: 'julia/1', source: 'github' }),
    ).toEqual({
      command: 'decision_download_model',
      args: { model: 'julia/1', source: 'github' },
    });
    expect(mapRestToCommand('POST', '/decision/download-model', { model: 'julia/1' })).toEqual({
      command: 'decision_download_model',
      args: { model: 'julia/1', source: null },
    });
  });

  it('maps GET /decision/history with numeric limit/offset coercion', () => {
    expect(mapRestToCommand('GET', '/decision/history?limit=50&offset=100')).toEqual({
      command: 'decision_history_list',
      args: { limit: 50, offset: 100 },
    });
    // Non-numeric values must fall back to null (serde Option<i64> rejects NaN).
    expect(mapRestToCommand('GET', '/decision/history?limit=abc')).toEqual({
      command: 'decision_history_list',
      args: { limit: null, offset: null },
    });
    expect(mapRestToCommand('GET', '/decision/history')).toEqual({
      command: 'decision_history_list',
      args: { limit: null, offset: null },
    });
  });

  it('maps GET /decision/history/:id with percent-decoding', () => {
    expect(mapRestToCommand('GET', '/decision/history/abc%20123')).toEqual({
      command: 'decision_history_get',
      args: { id: 'abc 123' },
    });
  });

  it('maps DELETE /decision/history (exact path) to clear-all', () => {
    expect(mapRestToCommand('DELETE', '/decision/history')).toEqual({
      command: 'decision_history_clear',
      args: {},
    });
  });

  it('throws for unmapped decision methods (no silent fallback)', () => {
    expect(() => mapRestToCommand('POST', '/decision/tools')).toThrow();
  });
});

describe('transformTauriResponse: decision_tools', () => {
  it('wraps a tool-definition array as {success, data}', () => {
    const tools = [{ name: 'decision_decide', description: 'd', inputSchema: { type: 'object' } }];
    expect(transformTauriResponse('decision_tools', tools)).toMatchObject({
      success: true,
      data: tools,
    });
  });

  it('preserves an empty array (decision disabled) instead of nulling it', () => {
    const r = transformTauriResponse('decision_tools', []) as { success: boolean; data: unknown[] };
    expect(r.success).toBe(true);
    expect(r.data).toEqual([]);
  });
});
