import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../utils/fetchInterceptor', () => ({
  apiGet: vi.fn(),
  apiPost: vi.fn(),
  apiPut: vi.fn(),
  apiDelete: vi.fn(),
}));

import { apiGet, apiPut } from '../utils/fetchInterceptor';
import {
  getDecisionTools,
  decisionStatus,
  decisionSetEnabled,
  listDecisionHistory,
} from './decisionService';

const mockedGet = vi.mocked(apiGet);
const mockedPut = vi.mocked(apiPut);

beforeEach(() => {
  mockedGet.mockReset();
  mockedPut.mockReset();
});

describe('getDecisionTools', () => {
  it('returns the tool definition array on success', async () => {
    const tools = [
      {
        name: 'decision_decide',
        description: 'Run a structured decision',
        inputSchema: { type: 'object', properties: {}, required: [] },
      },
    ];
    mockedGet.mockResolvedValue({ success: true, data: tools });
    const out = await getDecisionTools();
    expect(mockedGet).toHaveBeenCalledWith('/decision/tools');
    expect(out).toEqual(tools);
  });

  it('falls back to [] when data is missing', async () => {
    mockedGet.mockResolvedValue({ success: true, data: undefined as never });
    expect(await getDecisionTools()).toEqual([]);
  });

  it('throws on success:false with the server message', async () => {
    mockedGet.mockResolvedValue({ success: false, message: 'boom' });
    await expect(getDecisionTools()).rejects.toThrow('boom');
  });

  it('throws the fallback message when success:false has no message', async () => {
    mockedGet.mockResolvedValue({ success: false });
    await expect(getDecisionTools()).rejects.toThrow('Failed to get decision tools');
  });
});

describe('decisionStatus / decisionSetEnabled', () => {
  it('returns the status payload (including selectionLost passthrough)', async () => {
    mockedGet.mockResolvedValue({
      success: true,
      data: { enabled: true, model: 'julia/1', modelReady: true, selectionLost: null },
    });
    expect(await decisionStatus()).toEqual({
      enabled: true,
      model: 'julia/1',
      modelReady: true,
      selectionLost: null,
    });
    expect(mockedGet).toHaveBeenCalledWith('/decision/status');
  });

  it('passes a lost selection name through unchanged', async () => {
    mockedGet.mockResolvedValue({
      success: true,
      data: { enabled: true, model: 'julia/1', modelReady: true, selectionLost: 'user/2' },
    });
    const out = await decisionStatus();
    expect(out.selectionLost).toBe('user/2');
  });

  it('sends the enabled flag via PUT', async () => {
    mockedPut.mockResolvedValue({ success: true });
    await decisionSetEnabled(true);
    expect(mockedPut).toHaveBeenCalledWith('/decision/enabled', { enabled: true });
  });

  it('throws on toggle failure', async () => {
    mockedPut.mockResolvedValue({ success: false, message: 'nope' });
    await expect(decisionSetEnabled(false)).rejects.toThrow('nope');
  });
});

describe('listDecisionHistory', () => {
  it('passes limit/offset through to the query string (offset paging contract)', async () => {
    mockedGet.mockResolvedValue({ success: true, data: [] });
    await listDecisionHistory(51, 50);
    expect(mockedGet).toHaveBeenCalledWith('/decision/history?limit=51&offset=50');
  });

  it('defaults to limit=50 offset=0 when called without args', async () => {
    mockedGet.mockResolvedValue({ success: true, data: [] });
    await listDecisionHistory();
    expect(mockedGet).toHaveBeenCalledWith('/decision/history?limit=50&offset=0');
  });

  it('returns the entry array on success', async () => {
    const entry = { id: 'a', createdAt: 'x' };
    mockedGet.mockResolvedValue({ success: true, data: [entry] });
    expect(await listDecisionHistory(10, 0)).toEqual([entry]);
  });

  it('returns [] when data is missing', async () => {
    mockedGet.mockResolvedValue({ success: true });
    expect(await listDecisionHistory(10, 0)).toEqual([]);
  });

  it('throws on success:false with the server message', async () => {
    mockedGet.mockResolvedValue({ success: false, message: 'db locked' });
    await expect(listDecisionHistory(10, 0)).rejects.toThrow('db locked');
  });
});
