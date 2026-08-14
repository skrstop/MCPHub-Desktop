import { check, type Update, type DownloadEvent } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import { invoke, Channel } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { isTauri } from '@/utils/tauriClient';

export interface UpdateInfo {
  version: string;
  notes?: string;
  date?: string;
  /** Whether this platform supports auto-update via Tauri updater.
   *  Linux (deb/rpm) does NOT support auto-update; user must download manually. */
  canAutoUpdate?: boolean;
  /** Download URL for manual update (used on Linux) */
  downloadUrl?: string;
}

/** Where an update check was triggered from, for log attribution. */
export type UpdateCheckSource = 'startup' | 'about' | 'manual';

// GitHub Release latest.json URL for fallback version check (Linux)
const LATEST_JSON_URL = 'https://github.com/skrstop/MCPHub-Desktop/releases/latest/download/latest.json';

let cachedUpdate: Update | null = null;

/**
 * Append an update-check event to the application log (the same log the Logs
 * page reads). Fire-and-forget: a logging failure must never break the update
 * check itself, so errors are swallowed. No-op outside the Tauri runtime.
 */
export const logUpdateEvent = (level: 'info' | 'warn' | 'error' | 'debug', message: string): void => {
  if (!isTauri()) return;
  invoke('log_event', { level, message }).catch((e) => {
    console.warn('[update] failed to write log event:', e);
  });
};

/** Current app version, lazily fetched and cached for log messages. */
let cachedCurrentVersion: string | null = null;
const getCurrentVersion = async (): Promise<string> => {
  if (cachedCurrentVersion) return cachedCurrentVersion;
  try {
    const { getVersion } = await import('@tauri-apps/api/app');
    cachedCurrentVersion = await getVersion();
  } catch {
    cachedCurrentVersion = 'unknown';
  }
  return cachedCurrentVersion;
};

/**
 * Check whether a new application version is available via the Tauri updater plugin.
 * On platforms where Tauri updater doesn't work (e.g. Linux deb/rpm),
 * falls back to checking GitHub latest.json for version info.
 * Returns the update metadata when available, or `null` if the app is up-to-date
 * or running outside the Tauri runtime.
 *
 * `source` attributes the check in the application log
 * (startup | about | manual); defaults to 'about'.
 */
export const checkForAppUpdate = async (
  source: UpdateCheckSource = 'about',
): Promise<UpdateInfo | null> => {
  logUpdateEvent('info', `[update] checking for updates (source=${source})`);
  const currentVersion = await getCurrentVersion();
  try {
    const update = await check();
    cachedUpdate = update;
    if (update) {
      logUpdateEvent(
        'info',
        `[update] new version available: ${currentVersion} -> ${update.version} (autoUpdate=true)`,
      );
      return {
        version: update.version,
        notes: update.body,
        date: update.date,
        canAutoUpdate: true,
      };
    }
    // Tauri updater returned null — either up-to-date or platform not supported.
    // On Linux (deb/rpm), Tauri updater doesn't work, so we fall back to
    // checking GitHub latest.json to at least notify the user.
    const fallback = await checkFallbackUpdate();
    if (!fallback) {
      logUpdateEvent('info', `[update] already up to date (current=${currentVersion})`);
    }
    return fallback;
  } catch (error) {
    logUpdateEvent(
      'warn',
      `[update] Tauri updater check failed: ${error instanceof Error ? error.message : String(error)}`,
    );
    // Tauri updater failed, try fallback
    return await checkFallbackUpdate();
  }
};

/**
 * Fallback: check GitHub latest.json for version info.
 * Used when Tauri updater doesn't support the current platform (e.g. Linux deb/rpm).
 * Returns UpdateInfo with canAutoUpdate=false so the UI can show a "Download" link
 * instead of an "Install Update" button.
 */
const checkFallbackUpdate = async (): Promise<UpdateInfo | null> => {
  if (!isTauri()) return null;
  try {
    const response = await fetch(LATEST_JSON_URL, {
      signal: AbortSignal.timeout(10000), // 10s timeout
    });
    if (!response.ok) {
      console.warn('Failed to fetch latest.json for fallback update check:', response.status);
      return null;
    }
    const data = await response.json();
    const latestVersion = data.version as string | undefined;
    if (!latestVersion) return null;

    // Get current app version
    const { getVersion } = await import('@tauri-apps/api/app');
    const currentVersion = await getVersion();

    if (compareVersions(currentVersion, latestVersion) > 0) {
      // New version available — always link to GitHub Releases page for manual download
      const downloadUrl = 'https://github.com/skrstop/MCPHub-Desktop/releases/latest';

      logUpdateEvent(
        'info',
        `[update] new version available: ${currentVersion} -> ${latestVersion} (autoUpdate=false, fallback)`,
      );
      return {
        version: latestVersion,
        notes: data.notes as string | undefined,
        date: data.pub_date as string | undefined,
        canAutoUpdate: false,
        downloadUrl,
      };
    }
    logUpdateEvent('info', `[update] already up to date (current=${currentVersion}, fallback)`);
    return null;
  } catch (error) {
    logUpdateEvent(
      'warn',
      `[update] fallback update check failed: ${error instanceof Error ? error.message : String(error)}`,
    );
    console.warn('Fallback update check failed:', error);
    return null;
  }
};

/**
 * Download and install the latest update, then relaunch the app.
 * Re-uses the most recent `check()` result when present.
 * Only works on platforms with Tauri updater support (macOS, Windows).
 *
 * Uses the custom `install_update_cancelable` command instead of the plugin's
 * `downloadAndInstall` so the download can be aborted via `cancelAppUpdate`
 * (the plugin IPC has no cancellation API). Resolves to `false` when the user
 * cancelled — callers must not relaunch or treat it as success.
 */
export const installAppUpdate = async (
  onEvent?: (event: DownloadEvent) => void,
): Promise<boolean> => {
  const update = cachedUpdate ?? (await check());
  cachedUpdate = update;
  if (!update) {
    logUpdateEvent('warn', '[update] install requested but no update is available');
    throw new Error('No update available');
  }
  const currentVersion = await getCurrentVersion();
  logUpdateEvent('info', `[update] installing update: ${currentVersion} -> ${update.version}`);

  // Progress streams over the channel (same wire shape as the plugin's
  // DownloadEvent); the terminal result arrives via a `updater://install-result`
  // event, since cancelling aborts the download task without rejecting the
  // channel. installId ties the result to this attempt.
  const installId = Date.now();
  const channel = new Channel<DownloadEvent>();
  if (onEvent) channel.onmessage = onEvent;
  let installError: Error | null = null;

  const finished = new Promise<boolean>((resolve) => {
    let settled = false;
    let unlisten: (() => void) | undefined;
    const settle = (ok: boolean) => {
      if (settled) return;
      settled = true;
      unlisten?.();
      resolve(ok);
    };
    listen<{ installId: number; status: string; error?: string }>(
      'updater://install-result',
      (e) => {
        if (e.payload.installId !== installId) return; // stale attempt
        if (e.payload.status === 'ok') settle(true);
        else if (e.payload.status === 'cancelled') settle(false);
        else {
          logUpdateEvent('error', `[update] install failed: ${e.payload.error ?? 'unknown'}`);
          installError = new Error(e.payload.error ?? 'Install failed');
          settle(false);
        }
      },
    ).then((fn) => {
      if (settled) fn();
      else unlisten = fn;
    });
  });

  await invoke('install_update_cancelable', {
    installId,
    rid: update.rid,
    onEvent: channel,
  });
  const ok = await finished;
  if (!ok) {
    if (installError) throw installError;
    logUpdateEvent('info', '[update] install cancelled by user');
    return false;
  }
  logUpdateEvent('info', `[update] update installed, relaunching (-> ${update.version})`);
  await relaunch();
  return true;
};

/**
 * Cancel an in-flight update download (download phase only — the install
 * section is atomic and cannot be interrupted). No-op when nothing is running.
 */
export const cancelAppUpdate = async (): Promise<void> => {
  logUpdateEvent('info', '[update] cancelling update install');
  await invoke('cancel_update_install', {});
};

/**
 * Backward-compatible helper: returns the latest available version string,
 * or `null` when there is no newer version (or when running outside Tauri).
 */
export const checkLatestVersion = async (): Promise<string | null> => {
  const info = await checkForAppUpdate();
  return info?.version ?? null;
};

/**
 * Compare two semver-like version strings.
 * Returns a positive number when `latest` is newer than `current`,
 * negative when older, 0 when equal.
 */
export const compareVersions = (current: string, latest: string): number => {
  if (current === 'dev') return 1;
  const currentParts = current.split('.').map(Number);
  const latestParts = latest.split('.').map(Number);

  for (let i = 0; i < 3; i++) {
    const currentPart = currentParts[i] || 0;
    const latestPart = latestParts[i] || 0;
    if (currentPart < latestPart) return 1;
    if (currentPart > latestPart) return -1;
  }
  return 0;
};
