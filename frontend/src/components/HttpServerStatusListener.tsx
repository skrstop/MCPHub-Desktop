import React, { createContext, useContext, useEffect, useState } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { X, RefreshCw, AlertTriangle } from 'lucide-react';
import { isTauri } from '@/utils/tauriClient';
import { useTranslation } from 'react-i18next';

/**
 * Surfaces an MCP HTTP-server startup failure (most often a Windows Defender
 * Firewall block / port-in-use) to the user as a modal dialog — a toast is too
 * easy to miss for a failure that leaves the service unreachable.
 *
 * Two paths cover the startup race: `maybe_start` runs at app startup before the
 * webview registers its event listener, so a bind failure that fires then would
 * be missed by a pure event listener. On mount we fetch the last outcome via the
 * `get_http_server_status` command; afterwards we listen for the live
 * `http://server-status` event (emitted when the user changes the port in
 * Settings → `sync_with_config` → `start`).
 *
 * Exposed as a context so the Dashboard can show a persistent warning badge
 * while the service is down: if the user dismisses the dialog without
 * retrying, the badge re-opens it. The failure reason is localized from the
 * structured `errorKind` (addrInUse / permissionDenied / addrNotAvailable /
 * other) — the backend's raw English message stays in the logs.
 */
type HttpServerFailure = {
  port: number;
  error: string;
  errorKind?: string | null;
  detail?: string | null;
};

interface HttpServerStatusContextType {
  /** Current failure (null when the service is running / never failed). */
  failure: HttpServerFailure | null;
  /** Open the failure dialog (from the Dashboard warning badge). */
  openDialog: () => void;
}

const HttpServerStatusContext = createContext<HttpServerStatusContextType>({
  failure: null,
  openDialog: () => {},
});

export const useHttpServerStatus = () => useContext(HttpServerStatusContext);

type RawStatus = {
  running?: boolean;
  port?: number;
  error?: string | null;
  errorKind?: string | null;
  detail?: string | null;
};

// errorKind → i18n key suffix. Each kind renders exactly its own cause +
// suggestion — only permissionDenied mentions the firewall (its signature),
// addrInUse talks about the occupying app, etc. No combined catch-all blurb.
const KIND_KEY: Record<string, string> = {
  addrInUse: 'AddrInUse',
  permissionDenied: 'PermissionDenied',
  addrNotAvailable: 'AddrNotAvailable',
};

/** Localized one-line cause for a failure, keyed by the backend's errorKind. */
export const localizedFailureReason = (
  t: (k: string, opts?: Record<string, unknown>) => string,
  failure: HttpServerFailure,
): string =>
  t(`pages.dashboard.httpFailCause${KIND_KEY[failure.errorKind ?? ''] ?? 'Other'}`, {
    port: failure.port,
  });

/** Localized one-line suggestion for a failure, keyed by the backend's errorKind. */
export const localizedFailureSuggestion = (
  t: (k: string, opts?: Record<string, unknown>) => string,
  failure: HttpServerFailure,
): string =>
  t(`pages.dashboard.httpFailAction${KIND_KEY[failure.errorKind ?? ''] ?? 'Other'}`, {
    port: failure.port,
  });

export const HttpServerStatusProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const { t } = useTranslation();
  const [failure, setFailure] = useState<HttpServerFailure | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [retrying, setRetrying] = useState(false);

  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;

    const showError = (s: RawStatus) => {
      if (!s.error) {
        // Running / recovered — clear failure state and close the dialog.
        setFailure(null);
        setDialogOpen(false);
        return;
      }
      setFailure({
        port: s.port || 0,
        error: s.error,
        errorKind: s.errorKind,
        detail: s.detail,
      });
      setDialogOpen(true);
    };

    // Catch a startup failure that fired before this listener mounted.
    invoke<RawStatus>('get_http_server_status')
      .then((s) => {
        if (s && s.error) showError(s);
      })
      .catch(() => {
        // Command unavailable (older build) — ignore; live events still work.
      });

    listen<RawStatus>('http://server-status', (event) => {
      showError(event.payload);
    }).then((un) => {
      if (cancelled) {
        un();
      } else {
        unlisten = un;
      }
    });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // Retry: re-invoke the backend start with the failed port. On success the
  // backend emits `http://server-status` running=true which clears the failure
  // and closes the dialog via showError above. On failure, `start()` already
  // set the authoritative status (with the correct errorKind, e.g. addrInUse)
  // before returning Err - re-fetch it so the dialog shows the real reason
  // instead of a generic "retry failed" whose kind ("other") would mismatch
  // the actual bind error.
  const handleRetry = async () => {
    if (!failure) return;
    setRetrying(true);
    try {
      await invoke('start_http_server', { port: failure.port });
      setFailure(null);
      setDialogOpen(false);
    } catch {
      try {
        const s = await invoke<RawStatus>('get_http_server_status');
        if (s && s.error) {
          setFailure({
            port: s.port || failure.port,
            error: s.error,
            errorKind: s.errorKind,
            detail: s.detail,
          });
          return;
        }
      } catch {
        // status query unavailable - fall through to generic hint
      }
      setFailure((f) =>
        f ? { ...f, error: t('pages.dashboard.httpFailRetryFailed'), errorKind: null } : f,
      );
    } finally {
      setRetrying(false);
    }
  };

  return (
    <HttpServerStatusContext.Provider
      value={{ failure, openDialog: () => setDialogOpen(true) }}
    >
      {children}
      {dialogOpen && failure && (
        <div className="fixed inset-0 bg-black/50 z-50 flex items-center justify-center p-4">
          <div className="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-md w-full mx-4 border border-gray-100 dark:border-gray-700">
            <div className="flex items-center justify-between p-5 border-b border-[var(--hub-line-2)]">
              <h2 className="text-lg font-bold text-gray-900 dark:text-gray-100 flex items-center gap-2">
                <AlertTriangle
                  size={18}
                  className="flex-shrink-0"
                  style={{ color: 'oklch(0.45 0.18 25)' }}
                />
                {t('pages.dashboard.httpFailTitle') || 'MCP HTTP service failed to start'}
              </h2>
              <button
                onClick={() => setDialogOpen(false)}
                className="hub-icon-btn sm"
                aria-label={t('common.close') || 'Close'}
              >
                <X size={16} />
              </button>
            </div>
            <div className="p-5 space-y-4">
              <div>
                <p className="text-[12px] mb-1" style={{ color: 'var(--hub-ink-3)' }}>
                  {t('pages.dashboard.httpFailAddress') || 'Service address'}
                </p>
                <p className="hub-mono text-[13px] break-all" style={{ color: 'var(--hub-ink)' }}>
                  {failure.port ? `http://localhost:${failure.port}` : '-'}
                </p>
              </div>
              <div>
                <p className="text-[12px] mb-1" style={{ color: 'var(--hub-ink-3)' }}>
                  {t('pages.dashboard.httpFailReason') || 'Reason'}
                </p>
                <p className="text-[13px] leading-relaxed" style={{ color: 'var(--hub-ink)' }}>
                  {localizedFailureReason(t, failure)}
                </p>
              </div>
              <div>
                <p className="text-[12px] mb-1" style={{ color: 'var(--hub-ink-3)' }}>
                  {t('pages.dashboard.httpFailSuggestion') || 'Suggestion'}
                </p>
                <p className="text-[13px] leading-relaxed" style={{ color: 'var(--hub-ink)' }}>
                  {localizedFailureSuggestion(t, failure)}
                </p>
              </div>
              {/* Technical OS error detail — only for uncategorized failures. */}
              {failure.errorKind === 'other' && failure.detail && (
                <p className="hub-mono text-[11px] break-all" style={{ color: 'var(--hub-ink-3)' }}>
                  {failure.detail}
                </p>
              )}
            </div>
            <div className="flex items-center justify-end gap-2 p-5 border-t border-[var(--hub-line-2)]">
              <button onClick={() => setDialogOpen(false)} className="hub-btn">
                {t('common.close') || 'Close'}
              </button>
              <button onClick={handleRetry} disabled={retrying} className="hub-btn primary">
                <RefreshCw size={14} className={retrying ? 'animate-spin' : ''} />
                {t('pages.dashboard.httpFailRetry') || 'Retry start'}
              </button>
            </div>
          </div>
        </div>
      )}
    </HttpServerStatusContext.Provider>
  );
};

export default HttpServerStatusProvider;
