import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../utils/fetchInterceptor', () => ({
  apiGet: vi.fn(),
  apiPost: vi.fn(),
}));

import { apiPost } from '../utils/fetchInterceptor';
import { searchSmartRouting } from './smartRoutingService';

const mockedPost = vi.mocked(apiPost);

beforeEach(() => {
  mockedPost.mockReset();
});

describe('searchSmartRouting', () => {
  it('posts query/scope/limit and returns the result array', async () => {
    const rows = [
      {
        serverName: 'playwright',
        toolName: 'browser_navigate',
        contentId: 'playwright:browser_navigate',
        contentType: 'tool',
        textContent: 'Navigate to a URL',
        metadata: { description: 'Navigate to a URL' },
        score: 0.87,
      },
    ];
    mockedPost.mockResolvedValue({ success: true, data: rows });

    const out = await searchSmartRouting('open a page', 'group:uuid-1', 5);
    expect(mockedPost).toHaveBeenCalledWith('/smart-routing/search', {
      query: 'open a page',
      scope: 'group:uuid-1',
      limit: 5,
    });
    expect(out).toEqual(rows);
  });

  it('defaults limit to 20 and scope passthrough', async () => {
    mockedPost.mockResolvedValue({ success: true, data: [] });
    await searchSmartRouting('q', 'all');
    expect(mockedPost).toHaveBeenCalledWith('/smart-routing/search', {
      query: 'q',
      scope: 'all',
      limit: 20,
    });
  });

  it('returns [] when data is missing', async () => {
    mockedPost.mockResolvedValue({ success: true, data: undefined as never });
    expect(await searchSmartRouting('q', 'all')).toEqual([]);
  });

  it('throws on success:false with error field', async () => {
    mockedPost.mockResolvedValue({ success: false, error: 'smart routing is not enabled' });
    await expect(searchSmartRouting('q', 'all')).rejects.toThrow('smart routing is not enabled');
  });

  it('throws on success:false with message field', async () => {
    mockedPost.mockResolvedValue({ success: false, message: '403 admin gate' });
    await expect(searchSmartRouting('q', 'all')).rejects.toThrow('403 admin gate');
  });

  it('throws a fallback message when neither error nor message present', async () => {
    mockedPost.mockResolvedValue({ success: false });
    await expect(searchSmartRouting('q', 'all')).rejects.toThrow('Smart routing search failed');
  });

  it('propagates network rejections', async () => {
    mockedPost.mockRejectedValue(new Error('network down'));
    await expect(searchSmartRouting('q', 'all')).rejects.toThrow('network down');
  });
});

import { fetchSmartRoutingPerformance, reindexSmartRouting, getSmartReindexParticipants } from './smartRoutingService';
import { apiGet } from '../utils/fetchInterceptor';

const mockedGet = vi.mocked(apiGet);

beforeEach(() => {
  mockedGet.mockReset();
});

describe('fetchSmartRoutingPerformance', () => {
  it('returns data on success', async () => {
    const data = { enabled: true } as never;
    mockedGet.mockResolvedValue({ success: true, data });
    expect(await fetchSmartRoutingPerformance()).toBe(data);
    expect(mockedGet).toHaveBeenCalledWith('/smart-routing/performance');
  });

  it('throws with error field', async () => {
    mockedGet.mockResolvedValue({ success: false, error: 'boom' });
    await expect(fetchSmartRoutingPerformance()).rejects.toThrow('boom');
  });

  it('throws with message field fallback', async () => {
    mockedGet.mockResolvedValue({ success: false, message: '403' });
    await expect(fetchSmartRoutingPerformance()).rejects.toThrow('403');
  });

  it('throws default message when both absent', async () => {
    mockedGet.mockResolvedValue({ success: false });
    await expect(fetchSmartRoutingPerformance()).rejects.toThrow(
      'Failed to load smart routing performance',
    );
  });
});

describe('reindexSmartRouting', () => {
  it('posts and returns data on success', async () => {
    const data = { syncedServers: 3 } as never;
    mockedPost.mockResolvedValue({ success: true, data });
    expect(await reindexSmartRouting()).toBe(data);
    expect(mockedPost).toHaveBeenCalledWith('/smart-routing/reindex');
  });

  it('throws with error field', async () => {
    mockedPost.mockResolvedValue({ success: false, error: 'reindexing' });
    await expect(reindexSmartRouting()).rejects.toThrow('reindexing');
  });

  it('throws with message field fallback', async () => {
    mockedPost.mockResolvedValue({ success: false, message: '409' });
    await expect(reindexSmartRouting()).rejects.toThrow('409');
  });

  it('throws default message when both absent', async () => {
    mockedPost.mockResolvedValue({ success: false });
    await expect(reindexSmartRouting()).rejects.toThrow('Failed to rebuild smart routing index');
  });
});

describe('getSmartReindexParticipants', () => {
  it('unwraps the Rust tuple array [servers, tools]', async () => {
    mockedGet.mockResolvedValue({ success: true, data: [4, 42] as never });
    expect(await getSmartReindexParticipants()).toEqual({ servers: 4, tools: 42 });
    expect(mockedGet).toHaveBeenCalledWith('/smart-routing/reindex-participants');
  });

  it('accepts an object shape {servers, tools}', async () => {
    mockedGet.mockResolvedValue({ success: true, data: { servers: 2, tools: 7 } as never });
    expect(await getSmartReindexParticipants()).toEqual({ servers: 2, tools: 7 });
  });

  it('defaults missing fields to 0 (object shape passes values through as-is)', async () => {
    mockedGet.mockResolvedValue({ success: true, data: { servers: '5' } as never });
    expect(await getSmartReindexParticipants()).toEqual({ servers: '5', tools: 0 });
  });

  it('handles null data', async () => {
    mockedGet.mockResolvedValue({ success: true, data: null as never });
    expect(await getSmartReindexParticipants()).toEqual({ servers: 0, tools: 0 });
  });

  it('coerces non-numeric tuple entries to 0', async () => {
    mockedGet.mockResolvedValue({ success: true, data: ['x', 'y'] as never });
    expect(await getSmartReindexParticipants()).toEqual({ servers: 0, tools: 0 });
  });

  it('throws with error field', async () => {
    mockedGet.mockResolvedValue({ success: false, error: 'db down' });
    await expect(getSmartReindexParticipants()).rejects.toThrow('db down');
  });

  it('throws default message when both absent', async () => {
    mockedGet.mockResolvedValue({ success: false });
    await expect(getSmartReindexParticipants()).rejects.toThrow(
      'Failed to load smart routing participants',
    );
  });
});
