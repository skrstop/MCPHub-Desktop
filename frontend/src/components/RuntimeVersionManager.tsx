import { invoke } from '@tauri-apps/api/core';
import React, { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

interface RuntimeVersion {
  version: string;
  installed: boolean;
  active: boolean;
}

interface Props {
  runtime: 'node' | 'python';
}

const RuntimeVersionManager: React.FC<Props> = ({ runtime }) => {
  const { t } = useTranslation();
  const [versions, setVersions] = useState<RuntimeVersion[]>([]);
  const [loading, setLoading] = useState(true);
  const [actionLoading, setActionLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const listCmd = runtime === 'node' ? 'list_node_versions' : 'list_python_versions';
  const installCmd = runtime === 'node' ? 'install_node_version' : 'install_python_version';
  const uninstallCmd = runtime === 'node' ? 'uninstall_node_version' : 'uninstall_python_version';
  const setActiveCmd = runtime === 'node' ? 'set_active_node_version' : 'set_active_python_version';

  const fetchVersions = useCallback(async () => {
    try {
      const result = await invoke<RuntimeVersion[]>(listCmd);
      setVersions(result);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [listCmd]);

  useEffect(() => {
    fetchVersions();
  }, [fetchVersions]);

  const activeVersion = versions.find((v) => v.active);
  const selectedVersion = activeVersion?.version ?? 'system';

  const handleSelectChange = async (e: React.ChangeEvent<HTMLSelectElement>) => {
    const ver = e.target.value;
    const entry = versions.find((v) => v.version === ver);
    if (!entry) return;

    setActionLoading(true);
    setError(null);
    try {
      if (!entry.installed && ver !== 'system') {
        // Install first, then activate
        await invoke(installCmd, { version: ver });
      }
      await invoke(setActiveCmd, { version: ver });
      await fetchVersions();
    } catch (e) {
      setError(String(e));
    } finally {
      setActionLoading(false);
    }
  };

  const handleUninstall = async (ver: string) => {
    setActionLoading(true);
    setError(null);
    try {
      await invoke(uninstallCmd, { version: ver });
      await fetchVersions();
    } catch (e) {
      setError(String(e));
    } finally {
      setActionLoading(false);
    }
  };

  if (loading) {
    return (
      <div className="flex items-center gap-2 text-sm text-gray-500 py-1">
        <div className="animate-spin h-3.5 w-3.5 border-2 border-blue-500 border-t-transparent rounded-full" />
        {t('common.loading') || 'Loading...'}
      </div>
    );
  }

  const activeEntry = versions.find((v) => v.version === selectedVersion);

  return (
    <div className="flex items-center gap-3">
      {error && (
        <div className="w-full mb-2 p-2 bg-red-50 border border-red-200 rounded text-xs text-red-700">
          {error}
        </div>
      )}

      <div className="relative flex-1">
        <select
          value={selectedVersion}
          onChange={handleSelectChange}
          disabled={actionLoading}
          className="w-full py-1.5 pl-3 pr-8 border border-gray-300 rounded-md shadow-sm text-sm focus:outline-none focus:ring-blue-500 focus:border-blue-500 disabled:opacity-50 bg-white form-input"
        >
          {versions.map((v) => (
            <option key={v.version} value={v.version}>
              {v.version === 'system'
                ? `${t('settings.runtimeUseSystem')} (${t('settings.runtimeDefault')})`
                : `v${v.version}${v.installed ? ` — ${t('settings.runtimeInstalled')}` : ''}`}
            </option>
          ))}
        </select>
        {actionLoading && (
          <div className="absolute right-8 top-1/2 -translate-y-1/2">
            <div className="animate-spin h-3.5 w-3.5 border-2 border-blue-500 border-t-transparent rounded-full" />
          </div>
        )}
      </div>

      {/* Uninstall button — only shown when an installed non-system, non-active version is selected */}
      {activeEntry && activeEntry.version !== 'system' && activeEntry.installed && !activeEntry.active && (
        <button
          type="button"
          onClick={() => handleUninstall(activeEntry.version)}
          disabled={actionLoading}
          className="text-xs px-2.5 py-1.5 bg-white hover:bg-red-50 disabled:opacity-50 text-red-600 border border-red-200 rounded font-medium transition-colors whitespace-nowrap"
        >
          {t('settings.runtimeUninstall')}
        </button>
      )}
    </div>
  );
};

export default RuntimeVersionManager;
