import { check, type Update, type DownloadEvent } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';

export interface UpdateInfo {
  version: string;
  notes?: string;
  date?: string;
}

let cachedUpdate: Update | null = null;

/**
 * Check whether a new application version is available via the Tauri updater plugin.
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
      };
    }
    return null;
  } catch (error) {
    console.error('Failed to check for application update:', error);
    return null;
  }
};

/**
 * Download and install the latest update, then relaunch the app.
 * Re-uses the most recent `check()` result when present.
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
