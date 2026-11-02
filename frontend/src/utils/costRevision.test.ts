import { describe, it, expect } from 'vitest';
import { serverCostRevision } from './costRevision';
import type { Server } from '../types';

describe('serverCostRevision', () => {
  it('ignores polling metadata but detects same-count definition and exposure changes', () => {
    const server: Server = {
      name: 's',
      status: 'connected',
      tools: [{ name: 't', inputSchema: {} }],
    } as Server;
    const revision = serverCostRevision([server]);
    expect(serverCostRevision([{ ...server, recovery: { attempts: 2 } } as Server])).toBe(revision);
    expect(serverCostRevision([{ ...server, enabled: false } as Server])).not.toBe(revision);
    expect(
      serverCostRevision([
        { ...server, tools: [{ name: 't', inputSchema: { type: 'object' } }] } as Server,
      ]),
    ).not.toBe(revision);
    expect(serverCostRevision([{ ...server, status: 'disconnected' } as Server])).not.toBe(revision);
  });

  it('changes when tool/prompt/resource definitions change', () => {
    const server: Server = {
      name: 's',
      status: 'connected',
      tools: [{ name: 't', inputSchema: {} }],
      prompts: [{ name: 'p' }],
      resources: [{ uri: 'file:///a' }],
    } as unknown as Server;
    const revision = serverCostRevision([server]);
    expect(
      serverCostRevision([
        { ...server, prompts: [{ name: 'p' }, { name: 'p2' }] } as unknown as Server,
      ]),
    ).not.toBe(revision);
    expect(
      serverCostRevision([
        { ...server, resources: [{ uri: 'file:///b' }] } as unknown as Server,
      ]),
    ).not.toBe(revision);
  });

  it('is order-sensitive across servers (list identity matters)', () => {
    const a = { name: 'a', status: 'connected', tools: [] } as unknown as Server;
    const b = { name: 'b', status: 'connected', tools: [] } as unknown as Server;
    expect(serverCostRevision([a, b])).not.toBe(serverCostRevision([b, a]));
  });
});
