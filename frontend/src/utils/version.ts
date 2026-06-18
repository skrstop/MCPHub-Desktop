import { check, type Update, type DownloadEvent } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
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

// GitHub Release latest.json URL for fallback version check (Linux)
const LATEST_JSON_URL = 'https://github.com/skrstop/MCPHub-Desktop/releases/latest/download/latest.json';

let cachedUpdate: Update | null = null;

/**
 * Check whether a new application version is available via the Tauri updater plugin.
 * On platforms where Tauri updater doesn't work (e.g. Linux deb/rpm),
 * falls back to checking GitHub latest.json for version info.
 * Returns the update metadata when available, or `null` if the app is up-to-date
 * or running outside the Tauri runtime.
 */
export const checkForAppUpdate = async (): Promise<UpdateInfo | null> => {
  try {
    const update = await check();
    cachedUpdate = update;
    if (update) {
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
    return await checkFallbackUpdate();
  } catch (error) {
    console.error('Failed to check for application update via Tauri updater:', error);
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

      return {
        version: latestVersion,
        notes: data.notes as string | undefined,
        date: data.pub_date as string | undefined,
        canAutoUpdate: false,
        downloadUrl,
      };
    }
    return null;
  } catch (error) {
    console.warn('Fallback update check failed:', error);
    return null;
  }
};

/**
 * Download and install the latest update, then relaunch the app.
 * Re-uses the most recent `check()` result when present.
 * Only works on platforms with Tauri updater support (macOS, Windows).
 */
export const installAppUpdate = async (
  onEvent?: (event: DownloadEvent) => void,
): Promise<void> => {
  const update = cachedUpdate ?? (await check());
  if (!update) {
    throw new Error('No update available');
  }
  await update.downloadAndInstall(onEvent);
  await relaunch();
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
