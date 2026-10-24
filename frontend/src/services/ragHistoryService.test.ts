import { describe, it, expect, vi, beforeEach } from 'vitest';

// Mock the transport layer: ragService calls apiPost/apiGet from
// utils/fetchInterceptor — intercept and assert the wire calls.
vi.mock('../utils/fetchInterceptor', () => ({
  apiGet: vi.fn(),
  apiPost: vi.fn(),
  apiPut: vi.fn(),
}));

import { apiPost } from '../utils/fetchInterceptor';
import { listRagDocHistory, getRagDocHistory, deleteRagDocHistory, diffRagDocHistory } from './ragService';

const mockedPost = vi.mocked(apiPost);

beforeEach(() => {
  mockedPost.mockReset();
});

describe('ragService — doc history APIs', () => {
  it('listRagDocHistory posts docId and returns the version list', async () => {
    const versions = [{ version: 2, savedAt: 't', size: 5, name: 'a.md' }];
    mockedPost.mockResolvedValue({ success: true, data: versions });
    await expect(listRagDocHistory('doc-1')).resolves.toEqual(versions);
    expect(mockedPost).toHaveBeenCalledWith('/rag/docs/history/list', { docId: 'doc-1' });
  });

  it('listRagDocHistory defaults to an empty list when data is missing', async () => {
    mockedPost.mockResolvedValue({ success: true, data: undefined });
    await expect(listRagDocHistory('doc-1')).resolves.toEqual([]);
  });

  it('listRagDocHistory throws on success=false', async () => {
    mockedPost.mockResolvedValue({ success: false, message: 'boom' });
    await expect(listRagDocHistory('doc-1')).rejects.toThrow('boom');
  });

  it('getRagDocHistory posts docId + version and returns the doc', async () => {
    const doc = { id: 'doc-1', name: 'a.md (v2)', content: 'text' };
    mockedPost.mockResolvedValue({ success: true, data: doc });
    await expect(getRagDocHistory('doc-1', 2)).resolves.toEqual(doc);
    expect(mockedPost).toHaveBeenCalledWith('/rag/docs/history/get', { docId: 'doc-1', version: 2 });
  });

  it('getRagDocHistory throws a not-found error when data is missing', async () => {
    mockedPost.mockResolvedValue({ success: true, data: undefined });
    await expect(getRagDocHistory('doc-1', 9)).rejects.toThrow('History version not found');
  });

  it('getRagDocHistory throws on success=false', async () => {
    mockedPost.mockResolvedValue({ success: false, message: 'nope' });
    await expect(getRagDocHistory('doc-1', 1)).rejects.toThrow('nope');
  });

  it('deleteRagDocHistory posts docId + version list', async () => {
    mockedPost.mockResolvedValue({ success: true, data: 2 });
    await expect(deleteRagDocHistory('doc-1', [1, 2])).resolves.toBeUndefined();
    expect(mockedPost).toHaveBeenCalledWith('/rag/docs/history/delete', { docId: 'doc-1', versions: [1, 2] });
  });

  it('deleteRagDocHistory throws on success=false', async () => {
    mockedPost.mockResolvedValue({ success: false, message: 'denied' });
    await expect(deleteRagDocHistory('doc-1', [1])).rejects.toThrow('denied');
  });

  it('diffRagDocHistory posts from/to versions and returns the diff text', async () => {
    mockedPost.mockResolvedValue({ success: true, data: '--- v1\n+++ v2\n' });
    await expect(diffRagDocHistory('doc-1', 1, 2)).resolves.toBe('--- v1\n+++ v2\n');
    expect(mockedPost).toHaveBeenCalledWith('/rag/docs/history/diff', {
      docId: 'doc-1',
      fromVersion: 1,
      toVersion: 2,
    });
  });

  it('diffRagDocHistory omits toVersion when comparing against current', async () => {
    mockedPost.mockResolvedValue({ success: true, data: 'diff' });
    await expect(diffRagDocHistory('doc-1', 1, undefined)).resolves.toBe('diff');
    expect(mockedPost).toHaveBeenCalledWith('/rag/docs/history/diff', {
      docId: 'doc-1',
      fromVersion: 1,
      toVersion: undefined,
    });
  });

  it('diffRagDocHistory defaults to an empty string when data is missing', async () => {
    mockedPost.mockResolvedValue({ success: true, data: undefined });
    await expect(diffRagDocHistory('doc-1', 1)).resolves.toBe('');
  });

  it('diffRagDocHistory throws on success=false', async () => {
    mockedPost.mockResolvedValue({ success: false, message: 'bad' });
    await expect(diffRagDocHistory('doc-1', 1)).rejects.toThrow('bad');
  });
});
