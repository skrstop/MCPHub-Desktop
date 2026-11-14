import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../utils/fetchInterceptor', () => ({
  apiGet: vi.fn(),
  apiPost: vi.fn(),
  apiPut: vi.fn(),
  apiDelete: vi.fn(),
}));

import { apiGet, apiPut } from '../utils/fetchInterceptor';
import { getDecisionTools, decisionStatus, decisionSetEnabled } from './decisionService';

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

  it('falls back to [] when data is missing (decision disabled)', async () => {
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
  it('returns the status payload', async () => {
    mockedGet.mockResolvedValue({
      success: true,
      data: { enabled: true, model: 'julia/1', modelReady: true },
    });
    expect(await decisionStatus()).toEqual({ enabled: true, model: 'julia/1', modelReady: true });
    expect(mockedGet).toHaveBeenCalledWith('/decision/status');
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
