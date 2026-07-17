import React, { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ArrowUpRight, CheckCircle2, Download, Loader2, RefreshCw, X } from 'lucide-react';
import { ChangelogUpdateInfo } from '@/types';
import {
  buildChangelogFromTauriUpdate,
  fetchChangelogUpdateInfo,
} from '@/services/changelogService';
import { checkForAppUpdate, installAppUpdate, type UpdateInfo } from '@/utils/version';
import { isTauri } from '@/utils/tauriClient';
import Markdown from './Markdown';

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
  const [isInstalling, setIsInstalling] = useState(false);

  useEffect(() => {
    setUpdateInfo(initialUpdateInfo ?? null);
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
    setIsInstalling(true);
    try {
      await installAppUpdate((event) => {
        console.log('Download event:', event);
      });
    } catch (error) {
      console.error('Failed to install update:', error);
    } finally {
      setIsInstalling(false);
    }
  };

  useEffect(() => {
    if (isOpen) {
      checkForUpdates(false);
    }
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
            <p className="text-xs mt-1" style={{ color: 'var(--hub-ink-3)' }}>MCPHub Desktop</p>
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
                  {isInstalling ? t('about.installing') : t('about.installUpdate')}
                </button>
              )}
              {tauriUpdate && tauriUpdate.canAutoUpdate === false && (
                <a
                  href={tauriUpdate.downloadUrl || 'https://github.com/skrstop/MCPHub-Desktop/releases/latest'}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="hub-btn primary"
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
              >
                {t('about.viewReleaseNotes')}
                <ArrowUpRight className="h-3.5 w-3.5" />
              </a>
              {latestEntry?.url && (
                <a
                  href={latestEntry.url}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="hub-btn ghost"
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
