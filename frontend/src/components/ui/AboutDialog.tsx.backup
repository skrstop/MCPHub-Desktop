import React, { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { X, RefreshCw, Download } from 'lucide-react';
import { checkForAppUpdate, installAppUpdate } from '@/utils/version';

interface AboutDialogProps {
  isOpen: boolean;
  onClose: () => void;
  version: string;
}

const AboutDialog: React.FC<AboutDialogProps> = ({ isOpen, onClose, version }) => {
  const { t } = useTranslation();
  const [hasNewVersion, setHasNewVersion] = useState(false);
  const [latestVersion, setLatestVersion] = useState('');
  const [isChecking, setIsChecking] = useState(false);
  const [isInstalling, setIsInstalling] = useState(false);
  const [downloadProgress, setDownloadProgress] = useState<{ downloaded: number; total: number } | null>(null);
  const [installError, setInstallError] = useState<string | null>(null);

  const checkForUpdates = async () => {
    setIsChecking(true);
    setInstallError(null);
    try {
      const update = await checkForAppUpdate();
      if (update) {
        setLatestVersion(update.version);
        setHasNewVersion(true);
      } else {
        setHasNewVersion(false);
        setLatestVersion('');
      }
    } catch (error) {
      console.error('Failed to check for updates:', error);
    } finally {
      setIsChecking(false);
    }
  };

  const handleInstallUpdate = async () => {
    setIsInstalling(true);
    setInstallError(null);
    setDownloadProgress({ downloaded: 0, total: 0 });
    try {
      await installAppUpdate((event) => {
        if (event.event === 'Started') {
          setDownloadProgress({ downloaded: 0, total: event.data.contentLength ?? 0 });
        } else if (event.event === 'Progress') {
          setDownloadProgress((prev) => ({
            downloaded: (prev?.downloaded ?? 0) + event.data.chunkLength,
            total: prev?.total ?? 0,
          }));
        }
      });
    } catch (error) {
      console.error('Failed to install update:', error);
      setInstallError(error instanceof Error ? error.message : String(error));
      setIsInstalling(false);
      setDownloadProgress(null);
    }
  };

  useEffect(() => {
    if (isOpen) {
      checkForUpdates();
    }
  }, [isOpen, version]);

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 bg-black/50 bg-opacity-30 z-50 flex items-center justify-center p-4">
      <div className="bg-white dark:bg-gray-800 rounded-lg shadow-lg max-w-md w-full">
        <div className="p-6 relative">
          {/* Close button (X) in the top-right corner */}
          <button
            onClick={onClose}
            className="absolute top-4 right-4 text-gray-400 hover:text-gray-500 dark:text-gray-500 dark:hover:text-gray-400"
            aria-label={t('common.close')}
          >
            <X className="h-5 w-5" />
          </button>

          <h3 className="text-lg font-medium text-gray-900 dark:text-gray-100 mb-4">
            {t('about.title')}
          </h3>

          <div className="space-y-4">
            <div className="flex items-center justify-between">
              <span className="text-gray-700 dark:text-gray-300">
                {t('about.currentVersion')}:
              </span>
              <span className="font-medium text-gray-900 dark:text-gray-100">
                {version}
              </span>
            </div>

            {hasNewVersion && latestVersion && (
              <div className="bg-blue-50 dark:bg-blue-900 p-3 rounded">
                <div className="flex items-start">
                  <div className="flex-shrink-0">
                    <svg className="h-5 w-5 text-blue-600 dark:text-blue-300" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
                    </svg>
                  </div>
                  <div className="ml-3 flex-1 text-sm text-blue-700 dark:text-blue-300">
                    <p>{t('about.newVersionAvailable', { version: latestVersion })}</p>
                    <div className="mt-2 flex items-center gap-3">
                      <button
                        onClick={handleInstallUpdate}
                        disabled={isInstalling}
                        className="inline-flex items-center gap-2 px-3 py-1.5 bg-blue-600 hover:bg-blue-700 text-white rounded-md text-xs font-medium disabled:opacity-60"
                      >
                        <Download className={`h-3.5 w-3.5 ${isInstalling ? 'animate-pulse' : ''}`} />
                        {isInstalling
                          ? downloadProgress && downloadProgress.total > 0
                            ? `${t('about.downloading')} ${Math.round((downloadProgress.downloaded / downloadProgress.total) * 100)}%`
                            : t('about.installing')
                          : t('about.downloadAndInstall')}
                      </button>
                      <a
                        href="https://github.com/samanhappy/mcphub-desktop/releases"
                        target="_blank"
                        rel="noopener noreferrer"
                        className="text-blue-600 dark:text-blue-400 hover:underline text-xs"
                      >
                        {t('about.viewOnGitHub')}
                      </a>
                    </div>
                    {installError && (
                      <p className="mt-2 text-xs text-red-600 dark:text-red-400">
                        {t('about.updateError')}: {installError}
                      </p>
                    )}
                  </div>
                </div>
              </div>
            )}

            <button
              onClick={checkForUpdates}
              disabled={isChecking}
              className={`mt-4 inline-flex items-center px-4 py-2 border border-gray-200 dark:border-gray-600 rounded-md shadow-sm text-sm font-medium btn-secondary
                ${isChecking
                  ? 'text-gray-400 dark:text-gray-500 bg-gray-100 dark:bg-gray-800'
                  : 'text-gray-700 dark:text-gray-300 bg-white dark:bg-gray-700 hover:bg-gray-50 dark:hover:bg-gray-600'
                } focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500`}
            >
              <RefreshCw className={`h-4 w-4 mr-2 ${isChecking ? 'animate-spin' : ''}`} />
              {isChecking ? t('about.checking') : t('about.checkForUpdates')}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
};

export default AboutDialog;
