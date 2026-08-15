import React, { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ArrowUpRight, CheckCircle2, Download, Loader2, RefreshCw, X } from 'lucide-react';
import { ChangelogUpdateInfo } from '@/types';
import {
  buildChangelogFromTauriUpdate,
  fetchChangelogUpdateInfo,
} from '@/services/changelogService';
import {
  cancelAppUpdate,
  checkForAppUpdate,
  installAppUpdate,
  peekCachedAppUpdate,
  type UpdateInfo,
} from '@/utils/version';
import { openExternal } from '@/utils/externalLink';
import { isTauri } from '@/utils/tauriClient';
import Markdown from './Markdown';

/** Format a byte count as a human-readable size (B/KB/MB/GB). */
const formatBytes = (n: number): string => {
  if (!Number.isFinite(n) || n < 0) return '0 B';
  if (n < 1024) return `${Math.round(n)} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
};

/** Format a download speed (bytes/sec) as a human-readable rate. */
const formatSpeed = (bps: number): string => `${formatBytes(bps)}/s`;

interface AboutDialogProps {
  isOpen: boolean;
  onClose: () => void;
  version: string;
  initialUpdateInfo?: ChangelogUpdateInfo | null;
  onUpdateInfoChange?: (info: ChangelogUpdateInfo | null) => void;
}

const AboutDialog: React.FC<AboutDialogProps> = ({
  isOpen,
  onClose,
  version,
  initialUpdateInfo,
  onUpdateInfoChange,
}) => {
  const { t, i18n } = useTranslation();
  const [updateInfo, setUpdateInfo] = useState<ChangelogUpdateInfo | null>(
    initialUpdateInfo ?? null,
  );
  const [isChecking, setIsChecking] = useState(false);
  const [tauriUpdate, setTauriUpdate] = useState<UpdateInfo | null>(null);
  // Install lifecycle phases surfaced to the UI. The Tauri updater plugin emits
  // Started/Progress/Finished for the *download*; the install itself runs silently
  // after `Finished` (no per-step percent), so it is shown as indeterminate.
  const [installPhase, setInstallPhase] = useState<
    'idle' | 'downloading' | 'installing' | 'done' | 'error'
  >('idle');
  const [downloaded, setDownloaded] = useState(0);
  const [totalBytes, setTotalBytes] = useState(0);
  const [speedBps, setSpeedBps] = useState(0);
  // Rolling speed tracking across Progress events (EMA). Refs avoid re-render
  // thrash and keep timing state out of React state.
  const lastTsRef = useRef<number | null>(null);
  const speedEmaRef = useRef<number>(0);

  const isInstalling = installPhase === 'downloading' || installPhase === 'installing';

  // Mirror the parent's update info into local state, but only when it
  // represents a detected update — otherwise this would clobber an in-flight
  // or just-completed self-check triggered on open (the parent's "no-update"
  // / null arriving late would overwrite the dialog's own fresh result).
  useEffect(() => {
    if (initialUpdateInfo?.hasUpdate) {
      setUpdateInfo(initialUpdateInfo);
    }
  }, [initialUpdateInfo]);

  const checkForUpdates = async (force = false, source: 'about' | 'manual' = force ? 'manual' : 'about') => {
    setIsChecking(true);
    // 立即设置 updateInfo 为"检查更新中"状态
    setUpdateInfo({
      hasUpdate: false,
      latestVersion: '',
      entries: [],
      totalUpdateCount: 0,
      source: 'checking',
    });
    try {
      // 在 Tauri 环境下使用原生 updater 插件
      if (isTauri()) {
        const update = await checkForAppUpdate(source);
        setTauriUpdate(update);
        // 同时获取 changelog 信息用于显示
        const info = await fetchChangelogUpdateInfo({
          currentVersion: version,
          locale: i18n.language,
          force,
        });
        // If changelog API returned empty (desktop intercepts it) but we have
        // update info from Tauri updater (including fallback), construct a minimal
        // ChangelogUpdateInfo so the UI can show "new version available"
        if (update && (!info || !info.hasUpdate)) {
          const tauriInfo = buildChangelogFromTauriUpdate(update);
          setUpdateInfo(tauriInfo);
          // Sync the new-version result back to the root provider so the sidebar
          // badge lights up after a manual check that finds a new version.
          onUpdateInfoChange?.(tauriInfo);
        } else {
          // 确保在正常完成时也设置 updateInfo，避免一直显示"检查更新中..."
          if (info) {
            setUpdateInfo(info);
            onUpdateInfoChange?.(info);
          } else {
            setUpdateInfo({
              hasUpdate: false,
              latestVersion: '',
              entries: [],
              totalUpdateCount: 0,
              source: 'no-update',
            });
          }
        }
      } else {
        // Web 环境下使用 changelog API
        const info = await fetchChangelogUpdateInfo({
          currentVersion: version,
          locale: i18n.language,
          force,
        });
        // 确保在正常完成时也设置 updateInfo，避免一直显示"检查更新中..."
        if (info) {
          setUpdateInfo(info);
          onUpdateInfoChange?.(info);
        } else {
          setUpdateInfo({
            hasUpdate: false,
            latestVersion: '',
            entries: [],
            totalUpdateCount: 0,
            source: 'no-update',
          });
        }
      }
    } catch (error) {
      console.error('Failed to check for updates:', error);
      // 确保在错误时也设置 updateInfo，避免一直显示"检查更新中..."
      if (!updateInfo) {
        setUpdateInfo({
          hasUpdate: false,
          latestVersion: '',
          entries: [],
          totalUpdateCount: 0,
          source: 'error',
        });
      }
    } finally {
      setIsChecking(false);
    }
  };

  const handleInstallUpdate = async () => {
    if (!tauriUpdate) return;
    // Reset download/install state for a fresh run.
    setInstallPhase('downloading');
    setDownloaded(0);
    setTotalBytes(0);
    setSpeedBps(0);
    lastTsRef.current = null;
    speedEmaRef.current = 0;
    try {
      const completed = await installAppUpdate((event) => {
        if (event.event === 'Started') {
          setTotalBytes(event.data.contentLength ?? 0);
          setInstallPhase('downloading');
          setDownloaded(0);
        } else if (event.event === 'Progress') {
          const now = performance.now();
          if (lastTsRef.current != null) {
            const dt = (now - lastTsRef.current) / 1000;
            if (dt > 0) {
              const inst = event.data.chunkLength / dt;
              // Exponential moving average smooths bursty chunk reads.
              speedEmaRef.current =
                speedEmaRef.current === 0
                  ? inst
                  : 0.3 * inst + 0.7 * speedEmaRef.current;
              setSpeedBps(speedEmaRef.current);
            }
          }
          lastTsRef.current = now;
          setDownloaded((prev) => prev + event.data.chunkLength);
        } else if (event.event === 'Finished') {
          // Download done — the updater now verifies signature + installs
          // silently, then resolves. No granular install percent is exposed,
          // so we show an indeterminate "installing" state until relaunch.
          setInstallPhase('installing');
          setSpeedBps(0);
        }
      });
      // `false` = user cancelled — return to idle so the button can be used again.
      if (!completed) {
        setInstallPhase('idle');
        return;
      }
      setInstallPhase('done');
    } catch (error) {
      console.error('Failed to install update:', error);
      setInstallPhase('error');
    }
  };

  const handleCancelInstall = async () => {
    // Optimistically show cancelling; the terminal event resets phase to idle
    // via the install promise resolving false. Install phase cannot cancel.
    if (installPhase !== 'downloading') return;
    try {
      await cancelAppUpdate();
    } catch (error) {
      console.error('Failed to cancel update install:', error);
    }
  };

  // External links (release notes, official website, GitHub) must go through
  // the shell opener — plain `<a target="_blank">` doesn't open the system
  // browser from a Tauri 2 webview. preventDefault keeps the inner webview
  // from also trying (and failing) to navigate.
  const handleExternalClick = (e: React.MouseEvent<HTMLAnchorElement>, url: string) => {
    e.preventDefault();
    openExternal(url);
  };

  // On open: if a new version was already detected (startup check found an
  // update), show it directly without re-fetching. In every other case — no
  // info yet, or a completed check that found nothing ("已是最新" / disabled /
  // error) — automatically run a fresh check so entering About always gets a
  // current answer when there's no update to show. Any entry point (auto-open,
  // profile menu, tray/app-menu) follows the same rule; explicit refresh is
  // still available via the "Check for Updates" button inside the dialog.
  useEffect(() => {
    if (isOpen) {
      if (initialUpdateInfo?.hasUpdate) {
        setUpdateInfo(initialUpdateInfo);
        setTauriUpdate(peekCachedAppUpdate());
        return;
      }
      checkForUpdates(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isOpen]);

  const latestEntry = updateInfo?.entries[0] ?? null;
  const hasNewVersion = Boolean(updateInfo?.hasUpdate && updateInfo.latestVersion);
  const extraReleaseCount = Math.max(
    0,
    (updateInfo?.totalUpdateCount ?? 0) - (updateInfo?.entries.length ?? 0),
  );

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 bg-black/50 z-50 flex items-center justify-center p-4">
      <div className="hub-card w-full max-w-[520px] max-h-[85vh] flex flex-col shadow-xl">
        <div className="p-5 pb-3 relative shrink-0">
          <button
            onClick={onClose}
            className="hub-icon-btn sm absolute top-4 right-4"
            aria-label={t('common.close')}
          >
            <X className="h-4 w-4" />
          </button>

          <div className="pr-8">
            <h3 className="hub-h1">{t('about.title')}</h3>
            <p className="hub-sub">{t('about.versionInfo', { version })}</p>
          </div>
        </div>

        <div className="px-5 pb-5 space-y-4 overflow-y-auto min-h-0 flex-1">
            {isChecking || updateInfo?.source === 'checking' ? (
              <div className="flex items-center gap-2 text-[13px]" style={{ color: 'var(--hub-ink-2)' }}>
                <RefreshCw className="h-4 w-4 animate-spin" style={{ color: 'var(--hub-accent)' }} />
                {t('about.checking')}
              </div>
            ) : updateInfo?.source === 'disabled' ? (
              <div className="hub-card-pad rounded-md" style={{ background: 'var(--hub-bg-2)' }}>
                <p className="text-[13px]" style={{ color: 'var(--hub-ink-2)' }}>
                  {t('about.updateChecksDisabled')}
                </p>
              </div>
            ) : hasNewVersion ? (
              <div
                className="rounded-md border p-4"
                style={{
                  borderColor: 'var(--hub-line)',
                  background: 'var(--hub-bg-2)',
                }}
              >
                <div className="flex items-start justify-between gap-3">
                  <div>
                    <div className="hub-mono text-[11px]" style={{ color: 'var(--hub-warn)' }}>
                      {t('about.newVersion')}
                    </div>
                    <div className="mt-1 text-[15px] font-medium" style={{ color: 'var(--hub-ink)' }}>
                      {t('about.newVersionAvailable', { version: updateInfo?.latestVersion })}
                    </div>
                  </div>
                </div>

                {latestEntry?.summary ? (
                  <div className="mt-3">
                    <Markdown>{latestEntry.summary}</Markdown>
                  </div>
                ) : updateInfo?.source === 'npm-fallback' ? (
                  <p className="mt-3 text-[13px]" style={{ color: 'var(--hub-ink-2)' }}>
                    {t('about.releaseNotesUnavailable')}
                  </p>
                ) : null}
              </div>
            ) : (
              <div className="flex items-center gap-2 text-[13px]" style={{ color: 'var(--hub-ink-2)' }}>
                <CheckCircle2 className="h-4 w-4" style={{ color: 'var(--hub-ok)' }} />
                {t('about.upToDate')}
              </div>
            )}

            {/* Multi-version changelog list (web / changelog-API path). On desktop the
                updater falls back to a single entry whose content duplicates the
                "new version available" block above, so we hide it there to avoid a
                redundant card. */}
            {updateInfo?.entries.length && updateInfo.source !== 'tauri-fallback' ? (
              <div className="hub-card overflow-hidden">
                <div className="px-4 py-3 hub-border-b">
                  <h4 className="hub-card-title">{t('about.latestChanges')}</h4>
                </div>
                <div className="hub-divider">
                  {updateInfo.entries.map((entry) => (
                    <div key={entry.version} className="p-4">
                      <div className="flex items-center justify-between gap-3">
                        <div>
                          <div className="hub-mono text-[12px]" style={{ color: 'var(--hub-accent)' }}>
                            v{entry.version}
                          </div>
                          <div className="mt-1 text-[13px] font-medium" style={{ color: 'var(--hub-ink)' }}>
                            {entry.title}
                          </div>
                        </div>
                        <a
                          href={entry.changelogUrl}
                          target="_blank"
                          rel="noopener noreferrer"
                          className="hub-icon-btn sm"
                          aria-label={t('about.viewReleaseNotes')}
                          onClick={(e) => handleExternalClick(e, entry.changelogUrl)}
                        >
                          <ArrowUpRight className="h-3.5 w-3.5" />
                        </a>
                      </div>
                      {entry.highlights.length > 0 && (
                        <ul className="mt-2 space-y-1 list-none p-0">
                          {entry.highlights.slice(0, 3).map((item, idx) => (
                            <li key={idx} className="text-[12.5px]" style={{ color: 'var(--hub-ink-2)' }}>
                              <span style={{ color: 'var(--hub-accent)' }}>•</span>{' '}
                              <Markdown inline>{item}</Markdown>
                            </li>
                          ))}
                        </ul>
                      )}
                    </div>
                  ))}
                </div>
                {extraReleaseCount > 0 && (
                  <div className="px-4 py-2 hub-border-t text-[12px]" style={{ color: 'var(--hub-ink-3)' }}>
                    {t('about.earlierReleases', { count: extraReleaseCount })}
                  </div>
                )}
              </div>
            ) : null}
          </div>

          {/* Download / install progress. Surfaced from the Tauri updater
              DownloadEvent stream so the user can tell the update is actually
              working (percent + speed while downloading; indeterminate while
              installing, since the plugin exposes no install percent).
              Pinned OUTSIDE the scrollable notes area — long release notes
              must never push the progress indicator out of view. */}
          {installPhase !== 'idle' && (
            <div
              className="mx-5 mb-3 rounded-md border p-3 shrink-0"
              style={{
                borderColor: 'var(--hub-line)',
                background: 'var(--hub-bg-2)',
              }}
            >
                {installPhase === 'downloading' && (
                  <>
                    <div className="flex items-center justify-between gap-3 text-[12px] tabular-nums">
                      <span className="flex items-center gap-1.5" style={{ color: 'var(--hub-ink-2)' }}>
                        <Download className="h-3.5 w-3.5 animate-pulse" style={{ color: 'var(--hub-accent)' }} />
                        {t('about.downloading') || 'Downloading...'}
                      </span>
                      {totalBytes > 0 && (
                        <span style={{ color: 'var(--hub-ink-3)' }}>
                          {formatBytes(downloaded)} / {formatBytes(totalBytes)}
                        </span>
                      )}
                    </div>
                    <div
                      className="mt-2 h-1.5 w-full rounded overflow-hidden"
                      style={{ background: 'var(--hub-bg-1, rgba(0,0,0,0.06))' }}
                    >
                      {totalBytes > 0 ? (
                        <div
                          className="h-full transition-all duration-150"
                          style={{
                            width: `${Math.max(0, Math.min(100, (downloaded / totalBytes) * 100))}%`,
                            background: 'var(--hub-accent, #3b82f6)',
                          }}
                        />
                      ) : (
                        <div
                          className="h-full w-1/3 animate-pulse"
                          style={{ background: 'var(--hub-accent, #3b82f6)' }}
                        />
                      )}
                    </div>
                    <div className="mt-1.5 flex items-center justify-between text-[11px] tabular-nums">
                      <span style={{ color: 'var(--hub-ink-3)' }}>
                        {totalBytes > 0
                          ? `${Math.min(100, (downloaded / totalBytes) * 100).toFixed(0)}%`
                          : ''}
                      </span>
                      <span style={{ color: 'var(--hub-ink-3)' }}>
                        {speedBps > 0 ? formatSpeed(speedBps) : ''}
                      </span>
                    </div>
                  </>
                )}
                {installPhase === 'installing' && (
                  <div className="flex items-center gap-2 text-[12px]" style={{ color: 'var(--hub-ink-2)' }}>
                    <Loader2 className="h-3.5 w-3.5 animate-spin" style={{ color: 'var(--hub-accent)' }} />
                    {t('about.installing') || 'Installing update...'}
                  </div>
                )}
                {installPhase === 'done' && (
                  <div className="flex items-center gap-2 text-[12px]" style={{ color: 'var(--hub-ink-2)' }}>
                    <CheckCircle2 className="h-3.5 w-3.5" style={{ color: 'var(--hub-ok)' }} />
                    {t('about.installed') || 'Update installed. Relaunching...'}
                  </div>
                )}
                {installPhase === 'error' && (
                  <div className="flex items-center gap-2 text-[12px]" style={{ color: 'var(--hub-warn)' }}>
                    <X className="h-3.5 w-3.5" />
                    {t('about.installFailed') || 'Update failed. Please try again.'}
                  </div>
                )}
            </div>
          )}

          <div className="flex flex-wrap items-center gap-2 pt-3 px-5 pb-5 shrink-0 border-t" style={{ borderColor: 'var(--hub-line)' }}>
              <button
                onClick={() => checkForUpdates(true)}
                disabled={isChecking || isInstalling}
                className={`hub-btn ${(isChecking || isInstalling) ? 'opacity-60 cursor-not-allowed' : ''}`}
              >
                <RefreshCw className={`h-4 w-4 ${isChecking ? 'animate-spin' : ''}`} />
                {isChecking ? t('about.checking') : t('about.checkForUpdates')}
              </button>
              {tauriUpdate && tauriUpdate.canAutoUpdate !== false && (
                installPhase === 'downloading' ? (
                  <button
                    onClick={handleCancelInstall}
                    className="hub-btn"
                    style={{ borderColor: 'var(--hub-line)' }}
                  >
                    <X className="h-4 w-4" />
                    {t('about.cancelUpdate') || 'Cancel Update'}
                  </button>
                ) : (
                  <button
                    onClick={handleInstallUpdate}
                    disabled={isInstalling}
                    className={`hub-btn primary ${isInstalling ? 'opacity-60 cursor-not-allowed' : ''}`}
                  >
                    {isInstalling ? (
                      <Loader2 className="h-4 w-4 animate-spin" />
                    ) : (
                      <Download className="h-4 w-4" />
                    )}
                    {installPhase === 'installing'
                      ? t('about.installing') || 'Installing update...'
                      : t('about.installUpdate')}
                  </button>
                )
              )}
              {tauriUpdate && tauriUpdate.canAutoUpdate === false && (
                <a
                  href={tauriUpdate.downloadUrl || 'https://github.com/skrstop/MCPHub-Desktop/releases/latest'}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="hub-btn primary"
                  onClick={(e) => handleExternalClick(e, tauriUpdate.downloadUrl || 'https://github.com/skrstop/MCPHub-Desktop/releases/latest')}
                >
                  <Download className="h-4 w-4" />
                  {t('about.downloadManual')}
                </a>
              )}
              <a
                href={`https://github.com/skrstop/MCPHub-Desktop/releases`}
                target="_blank"
                rel="noopener noreferrer"
                className="hub-btn"
                onClick={(e) => handleExternalClick(e, 'https://github.com/skrstop/MCPHub-Desktop/releases')}
              >
                {t('about.viewReleaseNotes')}
                <ArrowUpRight className="h-3.5 w-3.5" />
              </a>
              <a
                href="https://github.com/skrstop/MCPHub-Desktop"
                target="_blank"
                rel="noopener noreferrer"
                className="hub-btn"
                onClick={(e) => handleExternalClick(e, 'https://github.com/skrstop/MCPHub-Desktop')}
              >
                {t('about.officialWebsite')}
                <ArrowUpRight className="h-3.5 w-3.5" />
              </a>
              {latestEntry?.url && (
                <a
                  href={latestEntry.url}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="hub-btn ghost"
                  onClick={(e) => handleExternalClick(e, latestEntry.url)}
                >
                  {t('about.viewOnGitHub')}
                </a>
              )}
            </div>
      </div>
    </div>
  );
};

export default AboutDialog;
