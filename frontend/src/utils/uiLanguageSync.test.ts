import { describe, it, expect, vi, beforeEach } from 'vitest';

const onMock = vi.fn();
const offMock = vi.fn();
const listeners: Record<string, (lng: string) => void> = {};

vi.mock('@/i18n', () => ({
  default: {
    language: 'zh-CN',
    on: (ev: string, fn: (lng: string) => void) => {
      listeners[ev] = fn;
      onMock(ev, fn);
    },
    off: (ev: string, fn: (lng: string) => void) => offMock(ev, fn),
  },
}));

const apiPutMock = vi.fn();
vi.mock('@/utils/fetchInterceptor', () => ({
  apiPut: (...args: unknown[]) => apiPutMock(...args),
}));

import { initUiLanguageSync } from './uiLanguageSync';

beforeEach(() => {
  apiPutMock.mockReset();
  apiPutMock.mockResolvedValue({ success: true });
  onMock.mockReset();
  for (const k of Object.keys(listeners)) delete listeners[k];
});

describe('initUiLanguageSync', () => {
  it('persists the initial UI language (primary subtag) and subscribes to changes', () => {
    initUiLanguageSync();
    expect(apiPutMock).toHaveBeenCalledWith('system-config', {
      mcp: { uiLanguage: 'zh' },
    });
    expect(onMock).toHaveBeenCalledWith('languageChanged', expect.any(Function));
  });

  it('normalizes changed languages and keeps syncing on every change', () => {
    initUiLanguageSync();
    apiPutMock.mockClear();
    listeners['languageChanged']('fr-FR');
    expect(apiPutMock).toHaveBeenCalledWith('system-config', {
      mcp: { uiLanguage: 'fr' },
    });
    listeners['languageChanged']('EN');
    expect(apiPutMock).toHaveBeenCalledWith('system-config', {
      mcp: { uiLanguage: 'en' },
    });
  });

  it('swallows backend errors (admin-only endpoint)', async () => {
    apiPutMock.mockRejectedValue(new Error('401'));
    initUiLanguageSync();
    // The initial sync must not produce an unhandled rejection.
    await Promise.resolve();
  });
});
