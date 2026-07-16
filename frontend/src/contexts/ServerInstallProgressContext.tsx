import React, { createContext, useState, useEffect, useContext, useCallback, useRef } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { isTauri } from '@/utils/tauriClient';

/**
 * Download/install progress for a stdio (npx/uvx) MCP server, mirroring the
 * backend `ServerInstallProgress` emitted on the `server://install-progress`
 * event.
 *
 * phase:
 *  - "downloading" - npx/uvx is fetching packages (progress may be null for an
 *    indeterminate bar).
 *  - "done"        - connected successfully.
 *  - "error"       - connect failed / timed out.
 */
export interface ServerInstallProgress {
  server: string;
  phase: string;
  progress: number | null;
  message: string | null;
}

/** Result of a best-effort "update available" check. Mirrors backend `ServerUpdateInfo`. */
export interface ServerUpdateInfo {
  server: string;
  /** Whether a newer package version is available. */
  hasUpdate: boolean;
  /** Last recorded installed package version (from the registry, not the
   *  server's self-reported version). */
  current: string | null;
  /** Latest version published on the registry. */
  latest: string | null;
}

interface ServerInstallProgressContextType {
  getProgress: (name: string) => ServerInstallProgress | null;
  getUpdate: (name: string) => ServerUpdateInfo | null;
  /** Dismiss the current update badge for a server AND suppress the same
   *  latest-version from re-appearing on subsequent reconnects (avoids the
   *  badge re-prompting right after a reinstall/update click). */
  dismissUpdate: (name: string) => void;
  /** True while a package download is in progress for this server. */
  isInstalling: (name: string) => boolean;
}

const ServerInstallProgressContext = createContext<ServerInstallProgressContextType | undefined>(undefined);

export const ServerInstallProgressProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const [progress, setProgress] = useState<Record<string, ServerInstallProgress>>({});
  const [updates, setUpdates] = useState<Record<string, ServerUpdateInfo>>({});
  // Per-server timers to clear a terminal (done/error) progress entry after a
  // brief flash so the user sees the final state before it disappears.
  const clearTimers = useRef<Record<string, ReturnType<typeof setTimeout>>>({});

  useEffect(() => {
    if (!isTauri()) return;
    let unlistenProgress: UnlistenFn | undefined;
    let unlistenUpdate: UnlistenFn | undefined;
    let cancelled = false;

    listen<ServerInstallProgress>('server://install-progress', (event) => {
      const p = event.payload;
      if (!p || !p.server) return;
      setProgress((prev) => ({ ...prev, [p.server]: p }));
      if (p.phase === 'done' || p.phase === 'error') {
        const name = p.server;
        if (clearTimers.current[name]) clearTimeout(clearTimers.current[name]);
        clearTimers.current[name] = setTimeout(() => {
          setProgress((prev) => {
            const next = { ...prev };
            delete next[name];
            return next;
          });
          delete clearTimers.current[name];
        }, 1500);
      }
    }).then((un) => {
      if (cancelled) un();
      else unlistenProgress = un;
    });

    listen<ServerUpdateInfo>('server://update-available', (event) => {
      const u = event.payload;
      if (!u || !u.server) return;
      // Always trust the latest check result. We deliberately do NOT suppress
      // by a "dismissed" set here: the backend `mark_reinstalled` flag already
      // records the freshly-installed version after an update click (so the
      // next check emits hasUpdate=false), and suppressing by version would
      // hide a legitimately-available update if the installed version ever
      // rolls back (e.g. a cached older version).
      setUpdates((prev) => ({ ...prev, [u.server]: u }));
    }).then((un) => {
      if (cancelled) un();
      else unlistenUpdate = un;
    });

    return () => {
      cancelled = true;
      unlistenProgress?.();
      unlistenUpdate?.();
      Object.values(clearTimers.current).forEach(clearTimeout);
      clearTimers.current = {};
    };
  }, []);

  const getProgress = useCallback((name: string) => progress[name] ?? null, [progress]);
  const getUpdate = useCallback((name: string) => updates[name] ?? null, [updates]);
  // Clear the update badge immediately (e.g. right after the user clicks
  // update). The backend's `mark_reinstalled` flag ensures the post-reinstall
  // check records the new version and emits hasUpdate=false, keeping it clear.
  const dismissUpdate = useCallback((name: string) => {
    setUpdates((prev) => {
      const next = { ...prev };
      delete next[name];
      return next;
    });
  }, []);
  const isInstalling = useCallback(
    (name: string) => {
      const p = progress[name];
      return !!p && p.phase === 'downloading';
    },
    [progress],
  );

  const value: ServerInstallProgressContextType = { getProgress, getUpdate, dismissUpdate, isInstalling };
  return (
    <ServerInstallProgressContext.Provider value={value}>{children}</ServerInstallProgressContext.Provider>
  );
};

export const useServerInstallProgress = () => {
  const ctx = useContext(ServerInstallProgressContext);
  if (!ctx) {
    throw new Error('useServerInstallProgress must be used within a ServerInstallProgressProvider');
  }
  return ctx;
};
