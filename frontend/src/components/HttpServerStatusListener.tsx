import React, { createContext, useContext, useEffect, useState } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { X, RefreshCw, AlertTriangle, Zap } from 'lucide-react';
import { isTauri } from '@/utils/tauriClient';
import { useTranslation } from 'react-i18next';
import { useToast } from '@/contexts/ToastContext';

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
  /** Loopback-hijack warning: server IS running but 127.0.0.1:<port> answers
   *  with a foreign /health (another app bound localhost on the same port). */
  warning?: string | null;
};

type PortOccupier = { pid: number; name: string };

// errorKind → i18n key suffix. Each kind renders exactly its own cause +
// suggestion — only permissionDenied mentions the firewall (its signature),
// addrInUse talks about the occupying app, etc. No combined catch-all blurb.
const KIND_KEY: Record<string, string> = {
  addrInUse: 'AddrInUse',
  permissionDenied: 'PermissionDenied',
  addrNotAvailable: 'AddrNotAvailable',
  // Loopback address squatted while the wildcard bind succeeded — start
  // refused to run exposed. Kind-specific copy; falls to "Other" otherwise.
  loopbackOccupied: 'loopbackOccupied',
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
  const { showToast } = useToast();
  const [failure, setFailure] = useState<HttpServerFailure | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [retrying, setRetrying] = useState(false);
  // Loopback-hijack mode: the server IS running, but localhost is served by
  // another app. Same dialog, yellow styling, kill-and-restart action.
  const [loopback, setLoopback] = useState(false);
  const [occupiers, setOccupiers] = useState<PortOccupier[]>([]);
  const [occupierLoading, setOccupierLoading] = useState(false);
  const [killEnabled, setKillEnabled] = useState(true);

  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;

    const showError = (s: RawStatus) => {
      // Loopback hijack: server running but localhost is served by another
      // app — raise the SAME dialog (yellow mode) so the user can kill the
      // squatter right there instead of a toast they may never notice.
      if (!s.error && s.warning) {
        setLoopback(true);
        setFailure({
          port: s.port || 0,
          error: s.warning,
          errorKind: null,
          detail: null,
        });
        setDialogOpen(true);
        return;
      }
      if (!s.error) {
        // Running / recovered — clear failure state and close the dialog.
        setFailure(null);
        setDialogOpen(false);
        setLoopback(false);
        return;
      }
      setLoopback(false);
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
        if (s && (s.error || s.warning)) showError(s);
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

  // Probe who is squatting on the port whenever the dialog opens (both the
  // bind-failure and loopback-hijack modes). Empty result = firewall /
  // permission problem — hide the kill UI.
  useEffect(() => {
    if (!dialogOpen || !failure) return;
    let cancelled = false;
    setOccupiers([]);
    setOccupierLoading(true);
    invoke<PortOccupier[]>('detect_port_occupier', { port: failure.port })
      .then((list) => {
        if (!cancelled) setOccupiers(Array.isArray(list) ? list : []);
      })
      .catch(() => {
        if (!cancelled) setOccupiers([]);
      })
      .finally(() => {
        if (!cancelled) setOccupierLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [dialogOpen, failure?.port]);

  // Kill occupier(s) then restart the HTTP service. Used when the dialog
  // detected a listening squatter and the user opted into "kill & restart".
  const handleKillAndRestart = async () => {
    if (!failure) return;
    setRetrying(true);
    try {
      for (const o of occupiers) {
        try {
          await invoke('kill_port_occupier', { pid: o.pid });
        } catch {
          // One unkillable process (e.g. protected system service) — keep
          // going; the restart attempt below will report the real outcome.
        }
      }
      await invoke('start_http_server', { port: failure.port });
      setFailure(null);
      setDialogOpen(false);
      setLoopback(false);
      showToast(t('pages.dashboard.httpKillRestarted'), 'success');
    } catch {
      showToast(t('pages.dashboard.httpKillFailed'), 'error');
    } finally {
      setRetrying(false);
    }
  };

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
      setLoopback(false);
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
                  style={{ color: loopback ? 'oklch(0.65 0.15 75)' : 'oklch(0.45 0.18 25)' }}
                />
                {loopback
                  ? t('pages.dashboard.httpFailTitleLoopback')
                  : t('pages.dashboard.httpFailTitle')}
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
              {/* Loopback mode: service alive, localhost hijacked. */}
              {loopback && (
                <div
                  className="rounded-lg border-l-4 p-3 text-[13px] leading-relaxed"
                  style={{
                    background: 'var(--hub-bg-2)',
                    borderColor: 'oklch(0.65 0.15 75)',
                    color: 'var(--hub-ink)',
                  }}
                >
                  {t('pages.dashboard.httpLoopbackBanner', { port: failure.port })}
                </div>
              )}
              <div>
                <p className="text-[12px] mb-1" style={{ color: 'var(--hub-ink-3)' }}>
                  {t('pages.dashboard.httpFailAddress') || 'Service address'}
                </p>
                <p className="hub-mono text-[13px] break-all" style={{ color: 'var(--hub-ink)' }}>
                  {failure.port ? `http://localhost:${failure.port}` : '-'}
                </p>
              </div>
              {/* Occupier card — probed on open; hidden when nobody listens
                  (firewall / permission problem, kill is meaningless). */}
              {(occupiers.length > 0 || occupierLoading) && (
                <div
                  className="rounded-lg border p-3"
                  style={{ borderColor: 'var(--hub-line-2)' }}
                >
                  <p className="text-[12px] mb-2" style={{ color: 'var(--hub-ink-3)' }}>
                    {t('pages.dashboard.httpOccupierTitle')}
                  </p>
                  {occupierLoading ? (
                    <p className="text-[13px]" style={{ color: 'var(--hub-ink-3)' }}>
                      {t('pages.dashboard.httpOccupierProbing')}
                    </p>
                  ) : (
                    <ul className="space-y-1">
                      {occupiers.map((o) => (
                        <li key={o.pid} className="hub-mono text-[13px] flex items-center gap-2" style={{ color: 'var(--hub-ink)' }}>
                          <span className="font-semibold">{o.name}</span>
                          <span style={{ color: 'var(--hub-ink-3)' }}>PID {o.pid}</span>
                        </li>
                      ))}
                    </ul>
                  )}
                  {occupiers.length > 0 && (
                    <label
                      className="mt-3 flex items-start gap-2 rounded-lg p-2 cursor-pointer"
                      style={{ background: 'var(--hub-bg-2)' }}
                    >
                      <input
                        type="checkbox"
                        className="hub-checkbox mt-0.5"
                        checked={killEnabled}
                        onChange={(e) => setKillEnabled(e.target.checked)}
                      />
                      <span className="text-[13px] leading-snug" style={{ color: 'var(--hub-ink)' }}>
                        <span className="font-semibold">
                          {t('pages.dashboard.httpKillOption')}
                        </span>
                        <span className="block text-[12px]" style={{ color: 'var(--hub-ink-3)' }}>
                          {t('pages.dashboard.httpKillHint')}
                        </span>
                      </span>
                    </label>
                  )}
                </div>
              )}
              <div>
                <p className="text-[12px] mb-1" style={{ color: 'var(--hub-ink-3)' }}>
                  {t('pages.dashboard.httpFailReason') || 'Reason'}
                </p>
                <p className="text-[13px] leading-relaxed" style={{ color: 'var(--hub-ink)' }}>
                  {loopback
                    ? t('pages.dashboard.httpFailReasonLoopback', { port: failure.port })
                    : localizedFailureReason(t, failure)}
                </p>
              </div>
              <div>
                <p className="text-[12px] mb-1" style={{ color: 'var(--hub-ink-3)' }}>
                  {t('pages.dashboard.httpFailSuggestion') || 'Suggestion'}
                </p>
                <p className="text-[13px] leading-relaxed" style={{ color: 'var(--hub-ink)' }}>
                  {loopback
                    ? t('pages.dashboard.httpFailSuggestionLoopback', { port: failure.port })
                    : localizedFailureSuggestion(t, failure)}
                </p>
              </div>
              {/* Technical OS error detail — only for uncategorized failures. */}
              {!loopback && failure.errorKind === 'other' && failure.detail && (
                <p className="hub-mono text-[11px] break-all" style={{ color: 'var(--hub-ink-3)' }}>
                  {failure.detail}
                </p>
              )}
            </div>
            <div className="flex items-center justify-end gap-2 p-5 border-t border-[var(--hub-line-2)]">
              <button onClick={() => setDialogOpen(false)} className="hub-btn">
                {t('common.close') || 'Close'}
              </button>
              {occupiers.length > 0 && killEnabled ? (
                <button onClick={handleKillAndRestart} disabled={retrying} className="hub-btn primary">
                  <Zap size={14} className={retrying ? 'animate-pulse' : ''} />
                  {t('pages.dashboard.httpKillRestart')}
                </button>
              ) : (
                <button onClick={handleRetry} disabled={retrying} className="hub-btn primary">
                  <RefreshCw size={14} className={retrying ? 'animate-spin' : ''} />
                  {t('pages.dashboard.httpFailRetry') || 'Retry start'}
                </button>
              )}
            </div>
          </div>
        </div>
      )}
    </HttpServerStatusContext.Provider>
  );
};

export default HttpServerStatusProvider;
