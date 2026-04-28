import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import React, { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

interface RuntimeVersion {
  version: string;
  installed: boolean;
  active: boolean;
  /** 已安装版本是否完整可用；未安装项与 system 默认 true */
  healthy: boolean;
}

/** 与后端 RuntimeProgress 一一对应 */
interface RuntimeProgress {
  runtime: 'node' | 'python';
  version: string;
  /** started | downloading | extracting | verifying | running | done | error */
  phase: string;
  /** 0..100；为 null 时显示 indeterminate 进度条 */
  progress: number | null;
  message: string | null;
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
  /** 当前活跃的进度条信息；null 表示无进行中的任务 */
  const [progress, setProgress] = useState<RuntimeProgress | null>(null);
  /** 历史日志（最多保留 N 行，仅展示最近的几行） */
  const [logLines, setLogLines] = useState<string[]>([]);
  const logRef = useRef<HTMLDivElement | null>(null);

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

  // 监听后端推送的安装进度事件
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    listen<RuntimeProgress>('runtime://progress', (event) => {
      const payload = event.payload;
      // 仅处理当前组件对应的运行时
      if (payload.runtime !== runtime) return;
      setProgress(payload);
      if (payload.message) {
        setLogLines((prev) => {
          const next = [...prev, payload.message as string];
          // 仅保留最近 200 行，避免内存膨胀
          return next.length > 200 ? next.slice(next.length - 200) : next;
        });
      }
    }).then((un) => {
      unlisten = un;
    });
    return () => {
      unlisten?.();
    };
  }, [runtime]);

  // 滚动日志到底部
  useEffect(() => {
    if (logRef.current) {
      logRef.current.scrollTop = logRef.current.scrollHeight;
    }
  }, [logLines]);

  const activeVersion = versions.find((v) => v.active);
  const selectedVersion = activeVersion?.version ?? 'system';

  const handleSelectChange = async (e: React.ChangeEvent<HTMLSelectElement>) => {
    const ver = e.target.value;
    const entry = versions.find((v) => v.version === ver);
    if (!entry) return;

    setActionLoading(true);
    setError(null);
    // 切换/安装 前重置进度状态
    setLogLines([]);
    setProgress(null);
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

  /**
   * 兜底重装：对当前选中（active）的版本强制重新安装，
   * 不切换 active，不修改用户配置。
   * 直接执行（不二次确认），后端会先清理目标目录后再下载，确保真正"重装"。
   */
  const handleReinstall = async (ver: string) => {
    if (ver === 'system') return;
    setActionLoading(true);
    setError(null);
    setLogLines([]);
    setProgress(null);
    try {
      await invoke(installCmd, { version: ver, force: true });
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
  const showReinstall = !!activeEntry && activeEntry.version !== 'system';
  const showUninstall =
    !!activeEntry &&
    activeEntry.version !== 'system' &&
    activeEntry.installed &&
    !activeEntry.active;
  // 当前选中的版本已安装但不健康 → 提示用户重装
  const isBroken =
    !!activeEntry &&
    activeEntry.version !== 'system' &&
    activeEntry.installed &&
    !activeEntry.healthy;

  return (
    // 外层改为纵向布局：error / 警告 独占一行，避免 w-full 把下方控件挤到 0 宽度
    <div className="flex flex-col gap-2">
      {error && (
        <div className="p-2 bg-red-50 border border-red-200 rounded text-xs text-red-700 break-all">
          {error}
        </div>
      )}

      {isBroken && !error && (
        <div className="p-2 bg-amber-50 border border-amber-200 rounded text-xs text-amber-700">
          ⚠️ {t('settings.runtimeBrokenWarning', { version: activeEntry!.version })}
        </div>
      )}

      {/* 安装/重装进度面板 */}
      {(actionLoading || (progress && progress.phase !== 'done' && progress.phase !== 'error')) && progress && (
        <div className="p-2 bg-blue-50 border border-blue-200 rounded text-xs text-blue-800 space-y-1.5">
          <div className="flex items-center justify-between">
            <span className="font-medium">
              {t(`settings.runtimePhase.${progress.phase}`, { defaultValue: progress.phase })}
              {' · v'}
              {progress.version}
            </span>
            <span className="tabular-nums text-blue-600">
              {progress.progress != null ? `${progress.progress}%` : ''}
            </span>
          </div>
          {/* 进度条：有百分比走 determinate，否则 indeterminate 动画 */}
          <div className="h-1.5 w-full bg-blue-100 rounded overflow-hidden">
            {progress.progress != null ? (
              <div
                className="h-full bg-blue-500 transition-all duration-150"
                style={{ width: `${Math.max(0, Math.min(100, progress.progress))}%` }}
              />
            ) : (
              <div className="h-full w-1/3 bg-blue-500 animate-pulse" />
            )}
          </div>
          {progress.message && (
            <div className="text-blue-700 break-all">{progress.message}</div>
          )}
          {logLines.length > 1 && (
            <div
              ref={logRef}
              className="mt-1 max-h-28 overflow-auto rounded bg-white/60 p-1 font-mono text-[11px] leading-snug text-gray-600 border border-blue-100"
            >
              {logLines.map((line, idx) => (
                <div key={idx} className="whitespace-pre-wrap break-all">
                  {line}
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      <div className="flex items-center gap-3">
        <div className="relative flex-1 min-w-0">
          <select
            value={selectedVersion}
            onChange={handleSelectChange}
            disabled={actionLoading}
            className="w-full py-1.5 pl-3 pr-8 border border-gray-300 rounded-md shadow-sm text-sm focus:outline-none focus:ring-blue-500 focus:border-blue-500 disabled:opacity-50 bg-white form-input"
          >
            {versions.map((v) => {
              let label: string;
              if (v.version === 'system') {
                label = t('settings.runtimeSystemDefault');
              } else if (v.installed && !v.healthy) {
                label = `v${v.version} — ⚠ ${t('settings.runtimeBroken')}`;
              } else if (v.installed) {
                label = `v${v.version} — ${t('settings.runtimeInstalled')}`;
              } else {
                label = `v${v.version}`;
              }
              return (
                <option key={v.version} value={v.version}>
                  {label}
                </option>
              );
            })}
          </select>
          {actionLoading && (
            <div className="absolute right-8 top-1/2 -translate-y-1/2">
              <div className="animate-spin h-3.5 w-3.5 border-2 border-blue-500 border-t-transparent rounded-full" />
            </div>
          )}
        </div>

        {/* 兜底重装按钮：只要选中的不是 system 就常驻显示，点击直接执行（force=true） */}
        {showReinstall && (
          <button
            type="button"
            onClick={() => handleReinstall(activeEntry!.version)}
            disabled={actionLoading}
            title={t('settings.runtimeReinstallTip') || ''}
            className="text-xs px-2.5 py-1.5 bg-white hover:bg-blue-50 disabled:opacity-50 text-blue-600 border border-blue-200 rounded font-medium transition-colors whitespace-nowrap"
          >
            {t('settings.runtimeReinstall')}
          </button>
        )}

        {/* 卸载按钮：仅当所选为已安装的非系统、非当前激活版本时显示 */}
        {showUninstall && (
          <button
            type="button"
            onClick={() => handleUninstall(activeEntry!.version)}
            disabled={actionLoading}
            className="text-xs px-2.5 py-1.5 bg-white hover:bg-red-50 disabled:opacity-50 text-red-600 border border-red-200 rounded font-medium transition-colors whitespace-nowrap"
          >
            {t('settings.runtimeUninstall')}
          </button>
        )}
      </div>
    </div>
  );
};

export default RuntimeVersionManager;
