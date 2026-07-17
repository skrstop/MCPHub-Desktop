import { ApiResponse, apiGet } from '@/utils/fetchInterceptor';
import { ChangelogUpdateInfo } from '@/types';
import type { UpdateInfo } from '@/utils/version';

const RELEASE_BASE = 'https://github.com/skrstop/MCPHub-Desktop/releases';

/**
 * Build a `ChangelogUpdateInfo` from a Tauri updater result.
 *
 * On desktop the `/changelog/update-info` endpoint is stubbed (returns no
 * update), so the About dialog and the startup update check both derive the
 * "new version available" payload from the Tauri updater here. Keeping this in
 * one place ensures the About dialog and the startup badge/auto-open stay in
 * sync.
 */
export function buildChangelogFromTauriUpdate(update: UpdateInfo): ChangelogUpdateInfo {
  const releaseUrl = `${RELEASE_BASE}/tag/v${update.version}`;
  return {
    latestVersion: update.version,
    hasUpdate: true,
    entries: update.notes
      ? [{
          version: update.version,
          title: update.version,
          summary: update.notes,
          highlights: [],
          changelogUrl: releaseUrl,
          url: releaseUrl,
        }]
      : [],
    totalUpdateCount: 1,
    changelogUrl: `${RELEASE_BASE}/latest`,
    allChangelogUrl: RELEASE_BASE,
    source: 'tauri-fallback' as ChangelogUpdateInfo['source'],
  };
}

export async function fetchChangelogUpdateInfo(input: {
  currentVersion: string;
  locale?: string;
  force?: boolean;
}): Promise<ChangelogUpdateInfo | null> {
  const params = new URLSearchParams();
  params.set('currentVersion', input.currentVersion);
  params.set('locale', normalizeLocale(input.locale));
  if (input.force) params.set('force', 'true');

  const response = await apiGet<ApiResponse<ChangelogUpdateInfo>>(
    `/changelog/update-info?${params.toString()}`,
  );
  if (!response.success || !response.data) return null;
  return response.data;
}

export function shouldShowUpdateBadge(info: ChangelogUpdateInfo | null): boolean {
  // There is no "dismiss" action — any detected new version lights the badge.
  return Boolean(info?.hasUpdate && info.latestVersion);
}

function normalizeLocale(value?: string): 'en' | 'zh' {
  return value?.toLowerCase().startsWith('zh') ? 'zh' : 'en';
}

