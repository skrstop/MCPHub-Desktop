import { describe, it, expect } from 'vitest';
import { buildSmartSearchScopes, scopeForRequest } from './smartSearchScope';

const labels = { all: 'All', group: 'Group', server: 'Server' };

describe('buildSmartSearchScopes', () => {
  it('returns only the all option for empty inputs', () => {
    expect(buildSmartSearchScopes([], [], labels)).toEqual([
      { value: 'all', label: 'All', kind: 'all' },
    ]);
  });

  it('orders all → groups → servers', () => {
    const opts = buildSmartSearchScopes(
      [
        { id: 'g1', name: 'web' },
        { id: 'g2', name: 'data' },
      ],
      [{ name: 'playwright' }, { name: 'codegraph' }],
      labels,
    );
    expect(opts.map((o) => o.kind)).toEqual(['all', 'group', 'group', 'server', 'server']);
    expect(opts[1]).toEqual({ value: 'group:g1', label: 'Group: web', kind: 'group' });
    expect(opts[3]).toEqual({ value: 'server:playwright', label: 'Server: playwright', kind: 'server' });
  });

  it('uses group id (not name) as the value', () => {
    const opts = buildSmartSearchScopes([{ id: 'uuid-1', name: 'web' }], [], labels);
    expect(opts[1].value).toBe('group:uuid-1');
  });

  it('handles server names containing colons', () => {
    const opts = buildSmartSearchScopes([], [{ name: 'a:b' }], labels);
    expect(opts[1].value).toBe('server:a:b');
  });
});

describe('scopeForRequest', () => {
  it('passes the scope through verbatim (backend parses the prefix)', () => {
    expect(scopeForRequest('all')).toBe('all');
    expect(scopeForRequest('group:uuid-1')).toBe('group:uuid-1');
    expect(scopeForRequest('server:playwright')).toBe('server:playwright');
  });

  it('a server literally named "all" keeps its prefix (no global degrade)', () => {
    expect(scopeForRequest('server:all')).toBe('server:all');
  });

  it('server names containing colons keep the full value', () => {
    expect(scopeForRequest('server:a:server:b')).toBe('server:a:server:b');
  });
});
