/// <reference types="node" />
import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import {
  SMART_SEARCH_SENTINELS,
  localizeSmartSearchError,
} from './smartSearchScope';

const t = (key: string) => `T(${key})`;

describe('localizeSmartSearchError', () => {
  it('maps the mv-runtime sentinel to its i18n key', () => {
    expect(
      localizeSmartSearchError('mv runtime not running', t),
    ).toBe('T(server.smartSearch.errMvNotRunning)');
  });

  it('maps the disabled sentinel to its i18n key', () => {
    expect(
      localizeSmartSearchError('smart routing is not enabled', t),
    ).toBe('T(server.smartSearch.errDisabled)');
  });

  it('matches sentinels embedded in longer messages', () => {
    expect(
      localizeSmartSearchError('prefix: mv runtime not running (detail)', t),
    ).toBe('T(server.smartSearch.errMvNotRunning)');
  });

  it('passes unknown messages through verbatim', () => {
    expect(localizeSmartSearchError('some other failure', t)).toBe(
      'some other failure',
    );
  });

  it('mv sentinel wins when both appear (first match)', () => {
    expect(
      localizeSmartSearchError(
        'mv runtime not running / smart routing is not enabled',
        t,
      ),
    ).toBe('T(server.smartSearch.errMvNotRunning)');
  });
});

describe('sentinel contract (Rust sources)', () => {
  const rustRoot = resolve(__dirname, '../../../src-tauri/src');

  it('mv runtime sentinel is a live anyhow! literal in mv/mod.rs', () => {
    const src = readFileSync(resolve(rustRoot, 'mv/mod.rs'), 'utf8');
    // Pin the quoted anyhow! literal (not a bare substring) so a reworded
    // error string fails here even if the old phrase survives in a comment.
    expect(src).toContain(`anyhow!("${SMART_SEARCH_SENTINELS.mvNotRunning}")`);
  });

  it('mv runtime sentinel is also pinned in smart_routing/search.rs (the dialog-path error)', () => {
    // search.rs emits its own mv-not-running error before touching the mv
    // runtime — that is the string the search dialog actually receives.
    const src = readFileSync(
      resolve(rustRoot, 'smart_routing/search.rs'),
      'utf8',
    );
    expect(src).toContain(`anyhow!("${SMART_SEARCH_SENTINELS.mvNotRunning}")`);
  });

  it('disabled sentinel is a live anyhow! literal in smart_routing/search.rs', () => {
    const src = readFileSync(
      resolve(rustRoot, 'smart_routing/search.rs'),
      'utf8',
    );
    expect(src).toContain(`anyhow!("${SMART_SEARCH_SENTINELS.disabled}")`);
  });
});
