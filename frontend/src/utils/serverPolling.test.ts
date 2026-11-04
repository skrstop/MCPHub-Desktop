import { describe, it, expect } from 'vitest';
import { createServerPollingPolicy } from './serverPolling';

const server = (status: string) => ({ name: 'a', status });

describe('server polling policy', () => {
  it('keeps polling after reinstall while package metadata resolves despite stable status', () => {
    let now = 0;
    const policy = createServerPollingPolicy(() => now, 60000);
    for (now = 0; now < 60000; now += 2000) {
      expect(policy.success([server('connected')])).toBe(2000);
    }
    expect(policy.success([server('connected')])).toBe(10000);
  });

  it('preserves the action deadline when a polling session restarts', () => {
    let now = 58000;
    const policy = createServerPollingPolicy(() => now, 60000);
    for (let i = 0; i < 3; i++) expect(policy.success([server('connected')])).toBe(2000);
    now = 60000;
    expect(policy.success([server('connected')])).toBe(10000);
  });

  it('defaults to the legacy behavior without a minimum window', () => {
    let now = 0;
    const policy = createServerPollingPolicy(() => now);
    // Stable + connected + no status change → slow interval immediately
    // after the stable samples settle.
    policy.success([server('connected')]);
    policy.success([server('connected')]);
    expect(policy.success([server('connected')])).toBe(10000);
    // Default clock (Date.now) works: a fresh policy starts fast.
    expect(createServerPollingPolicy().success([server('connecting')])).toBe(2000);
  });

  it('reopens the fast window when a status change follows a settled period', () => {
    let now = 0;
    const policy = createServerPollingPolicy(() => now);
    policy.success([server('connected')]);
    policy.success([server('connected')]);
    expect(policy.success([server('connected')])).toBe(10000);
    now += 10000;
    // Status change after the window closed → fresh fast window.
    expect(policy.success([server('connecting')])).toBe(2000);
  });

  it('signatures are order-independent across multiple servers', () => {
    let now = 0;
    const policy = createServerPollingPolicy(() => now);
    const a = { name: 'a', status: 'connected' };
    const b = { name: 'b', status: 'connecting' };
    expect(policy.success([a, b])).toBe(2000);
    // Same set, different order → identical signature → stable sample #2.
    expect(policy.success([b, a])).toBe(2000);
    expect(policy.success([a, b])).toBe(2000);
  });

  it('backs off exponentially on failures and reopens the fast window on recovery', () => {
    let now = 0;
    const policy = createServerPollingPolicy(() => now);
    expect(policy.failure()).toBe(2000);
    now += 2000;
    expect(policy.failure()).toBe(4000);
    now += 4000;
    expect(policy.failure()).toBe(8000);
    // Recovery after failures restarts a fresh fast window.
    expect(policy.success([server('disconnected')])).toBe(2000);
    now += 2000;
    expect(policy.success([server('connected')])).toBe(2000);
  });
});
