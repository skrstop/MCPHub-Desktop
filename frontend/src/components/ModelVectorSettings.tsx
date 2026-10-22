import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Boxes, Check, ChevronDown, Database, Loader2, RefreshCw } from 'lucide-react';
import { listen } from '@tauri-apps/api/event';
import ModelSelector, { type ModelDownloadProgress } from '@/components/ui/ModelSelector';
import ReindexDialog from '@/components/ReindexDialog';
import { reindexSmartRouting } from '@/services/smartRoutingService';
import { useSettings } from '@/contexts/SettingsContext';
import { useToast } from '@/contexts/ToastContext';
import {
  ragStatus,
  listRagModels,
  currentRagModel,
  selectRagModel,
  downloadRagModel,
  modelDownloadStatus,
  cancelModelDownload,
  reindexAllRag,
} from '@/services/ragService';
import { apiGet, apiPut } from '@/utils/fetchInterceptor';
import type { RagModelInfo, RagStatus } from '@/types';

type MvDevice = 'auto' | 'gpu' | 'cpu';
const DEVICES: MvDevice[] = ['auto', 'gpu', 'cpu'];

/** 设置页【模型和向量】卡片（桌面端专属）：共享的本地 embedding 模型与向量库
 *  运行时的统一管理入口。RAG 与 Smart Routing 是当前的两个消费方——任一开启
 *  即自动加载模型（全局单实例），全部关闭后释放。 */
const ModelVectorSettings: React.FC = () => {
  const { t } = useTranslation();
  const { showToast } = useToast();
  const { smartRoutingConfig } = useSettings();

  const [models, setModels] = useState<RagModelInfo[]>([]);
  const [currentModel, setCurrentModel] = useState<string | null>(null);
  const [modelDownload, setModelDownload] = useState<ModelDownloadProgress | null>(null);
  const [status, setStatus] = useState<RagStatus | null>(null);
  const [device, setDevice] = useState<MvDevice>('auto');
  const [deviceDirty, setDeviceDirty] = useState(false);
  const [deviceOpen, setDeviceOpen] = useState(false);
  const [switching, setSwitching] = useState(false);
  const [reindexConfirmOpen, setReindexConfirmOpen] = useState(false);
  // Model picked in the selector: parked until 开始重载 (no switch happens
  // on pick; cancel just drops the parked choice with zero side effects).
  const [pendingModel, setPendingModel] = useState<string | null>(null);
  // Dialog copy mode: 'switch' (model swap) vs 'reload' (plain re-embed with
  // the current model) — same shared ReindexDialog, different lead/done text.
  const [reindexMode, setReindexMode] = useState<'switch' | 'reload'>('switch');
  const [sectionOpen, setSectionOpen] = useState(false);
  const deviceWrapRef = useRef<HTMLDivElement>(null);
  const mounted = useRef(true);

  // RAG feature toggle (server-side is_enabled) — distinct from the mv
  // runtime running state; the RAG tag reflects the switch, not the model.
  const mvRunning = status?.mvRunning ?? !!status?.enabled;
  const ragEnabled = status?.enabled ?? mvRunning;

  // 设备自定义下拉的点击外部关闭。
  useEffect(() => {
    if (!deviceOpen) return;
    const onDown = (e: MouseEvent) => {
      if (deviceWrapRef.current && !deviceWrapRef.current.contains(e.target as Node)) {
        setDeviceOpen(false);
      }
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [deviceOpen]);

  const fetchAll = async () => {
    try {
      const [m, cur, st] = await Promise.all([
        listRagModels(),
        currentRagModel(),
        ragStatus(),
      ]);
      if (!mounted.current) return;
      setModels(m);
      setCurrentModel(cur);
      setStatus(st);
      // 恢复后台下载进度（下载在后台进行，设置页可被切走；进度缓存在
      // 后端内存中，切回来时查询恢复）。
      if (cur) {
        const p = await modelDownloadStatus(cur);
        if (mounted.current && p && p.phase === 'downloading') setModelDownload(p);
      }
    } catch (e) {
      console.warn('[mv] failed to load model/vector status', e);
    }
  };

  useEffect(() => {
    mounted.current = true;
    void fetchAll();
    // 运行设备：读共享配置 mv.device（deep-merge 持久化）。
    apiGet<Record<string, any>>('config')
      .then((res) => {
        if (!mounted.current) return;
        const d = res?.data?.systemConfig?.mv?.device;
        if (d === 'gpu' || d === 'cpu' || d === 'auto') setDevice(d);
      })
      .catch(() => undefined);
    // 模型下载进度（与 RagPage 同一事件源；两处监听互不影响）。
    const unlistenP = listen<ModelDownloadProgress>('rag://model-download', (event) => {
      const p = event.payload;
      if (!p || !mounted.current) return;
      setModelDownload({ ...p });
      if (p.phase === 'done' || p.phase === 'error' || p.phase === 'cancelled') {
        void fetchAll();
        if (p.phase === 'done') {
          setTimeout(() => {
            if (mounted.current) setModelDownload(null);
          }, 1500);
        }
      }
    });
    return () => {
      mounted.current = false;
      unlistenP.then((un) => un());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const handleSelectModel = async (size: string) => {
    if (size === currentModel) return;
    setPendingModel(size);
    setReindexMode('switch');
    setReindexConfirmOpen(true);
  };

  // 重新载入 button: same dialog, 'reload' mode — no model switch, the
  // current model re-embeds everything (pendingModel stays null so
  // handleStartReindex skips the switch block).
  const handleReloadClick = () => {
    setPendingModel(null);
    setReindexMode('reload');
    setReindexConfirmOpen(true);
  };

  const handleStartReindex = async (): Promise<'clean' | 'reindexed'> => {
    const target = pendingModel;
    setPendingModel(null);
    setSwitching(true);
    try {
      if (target) {
        const st = await selectRagModel(target);
        if (!mounted.current) return 'clean';
        setCurrentModel(target);
        setStatus(st);
        if (!st.needsReindex) {
          setReindexConfirmOpen(false);
          showToast(t('settings.mvModelSwitched'), 'success');
          void fetchAll();
          return 'clean';
        }
      }
      await reindexAllRag();
      if (smartRoutingConfig.enabled) await reindexSmartRouting();
      if (mounted.current) {
        showToast(t('settings.mvReindexDone'), 'success');
        void fetchAll();
      }
      return 'reindexed';
    } catch (e) {
      showToast(e instanceof Error ? e.message : String(e), 'error');
      setReindexConfirmOpen(false);
      return 'clean';
    } finally {
      if (mounted.current) setSwitching(false);
    }
  };

  const handleReindexCancel = () => {
    setReindexConfirmOpen(false);
    setPendingModel(null);
    setReindexMode('switch');
  };

  const handleDeviceChange = async (d: MvDevice) => {
    setDevice(d);
    try {
      const res = await apiPut('system-config', { mv: { device: d } });
      if (!res?.success) throw new Error(res?.message || 'Failed to save device');
      if (mounted.current) setDeviceDirty(mvRunning && d !== 'auto');
      showToast(t('settings.mvDeviceSaved'), 'success');
    } catch (e) {
      showToast(e instanceof Error ? e.message : String(e), 'error');
    }
  };

  const currentInfo = models.find((m) => m.size === currentModel);
  // 维度来自后端真实值（模型加载时读 GGUF embed_dim；未加载时无）。
  const statusText = mvRunning
    ? t('settings.mvStatusRunning', {
        model: currentInfo?.label || currentModel || '—',
        dim: status?.embedDim ?? '—',
        device: t(`settings.mvDevice${device.charAt(0).toUpperCase()}${device.slice(1)}`),
      })
    : t('settings.mvStatusStopped');

  return (
    <div className="hub-card mb-6 overflow-visible">
      {/* 可折叠头部（与其他设置区一致的交互） */}
      <div
        className="flex justify-between items-center cursor-pointer transition-colors hover:bg-[var(--hub-surface-hover)] py-3 px-5"
        onClick={() => setSectionOpen((v) => !v)}
      >
        <div className="flex items-center gap-2.5">
          <Boxes size={15} className="text-[var(--hub-ink-2)]" />
          <h2 className="font-medium text-[var(--hub-ink)]">{t('settings.mvTitle')}</h2>
          {/* 消费方徽章移到标题行（原在展开内容首行，收起时不可见） */}
          <span
            className="hub-tag"
            title={t('settings.mvSourceSmartRouting')}
            style={smartRoutingConfig.enabled
              ? { background: 'oklch(0.66 0.15 145 / 0.15)', color: 'var(--hub-ok)', borderColor: 'transparent' }
              : { opacity: 0.45 }}
          >
            {t('settings.mvSourceSmartRouting')}
          </span>
          <span
            className="hub-tag"
            title="RAG"
            style={ragEnabled
              ? { background: 'oklch(0.66 0.15 145 / 0.15)', color: 'var(--hub-ok)', borderColor: 'transparent' }
              : { opacity: 0.45 }}
          >
            RAG
          </span>
        </div>
        <span className="text-[var(--hub-ink-3)]">{sectionOpen ? '−' : '+'}</span>
      </div>

      {sectionOpen && (
      <div className="px-5 py-4 border-t space-y-4" style={{ borderColor: 'var(--hub-line-2)' }}>
        {/* 运行状态（只读）：单实例共享运行时 + 启用来源徽章 */}
        <div className="flex items-center gap-2 flex-wrap">
          <span className="hub-status" data-state={mvRunning ? 'on' : 'off'}>
            <span
              className="hub-dot"
              style={{
                background: mvRunning ? 'var(--hub-ok)' : 'var(--hub-ink-3)',
                boxShadow: mvRunning ? '0 0 0 3px oklch(0.66 0.15 145 / 0.15)' : 'none',
              }}
            />
            <span style={{ fontSize: 12.5, color: mvRunning ? 'var(--hub-ink)' : 'var(--hub-ink-3)' }}>
              {statusText}
            </span>
          </span>
          {status?.initializing && (
            <Loader2 size={12} className="animate-spin text-[var(--hub-ink-3)]" />
          )}
          <button
            type="button"
            className="hub-btn sm"
            style={{ marginLeft: 'auto' }}
            disabled={(!smartRoutingConfig.enabled && !ragEnabled) || !mvRunning || status?.initializing || switching}
            onClick={handleReloadClick}
            title={t('pages.rag.reloadButtonHint')}
          >
            <RefreshCw size={12} /> {t('pages.rag.reloadButton')}
          </button>
        </div>
        <p style={{ fontSize: 12, color: 'var(--hub-ink-3)' }}>{t('settings.mvHint')}</p>

        {/* 模型切换（唯一入口；未启动时禁用） */}
        <div className={`rounded-lg p-4 bg-gray-50 dark:bg-[var(--hub-item)] ${mvRunning ? '' : 'mv-disabled'}`}>
          <div className="flex items-center gap-2 mb-2">
            <Database size={13} className="text-[var(--hub-ink-3)]" />
            <h3 className="text-[13px] font-medium flex-1" style={{ color: 'var(--hub-ink)' }}>
              {t('settings.mvModelSwitch')}
            </h3>
          </div>
          <ModelSelector
            models={models}
            currentModel={currentModel}
            modelDownload={modelDownload}
            disabled={!mvRunning || status?.initializing || switching}
            onSelect={handleSelectModel}
            onDownload={(size, source) => {
              downloadRagModel(size, source).catch((e) =>
                showToast(e instanceof Error ? e.message : String(e), 'error'),
              );
            }}
            onCancel={(size) => {
              cancelModelDownload(size).catch((e) =>
                showToast(e instanceof Error ? e.message : String(e), 'error'),
              );
            }}
            onRefresh={fetchAll}
          />
          {modelDownload && modelDownload.phase !== 'downloading' && (
            <p style={{ fontSize: 11.5, color: 'var(--hub-ink-3)', marginTop: 6 }}>
              {modelDownload.phase === 'error'
                ? modelDownload.message || t('settings.mvDownloadFailed')
                : modelDownload.phase === 'cancelled'
                ? t('settings.mvDownloadCancelled')
                : t('settings.mvDownloadDone')}
            </p>
          )}
        </div>

        {/* 运行设备（AUTO/GPU/CPU）：样式与 ModelSelector 触发框对齐。 */}
        <div className={`rounded-lg p-4 bg-gray-50 dark:bg-[var(--hub-item)] ${mvRunning ? '' : 'mv-disabled'}`}>
          <div className="flex items-center gap-2 mb-2">
            <h3 className="text-[13px] font-medium" style={{ color: 'var(--hub-ink)' }}>
              {t('settings.mvDevice')}
            </h3>
          </div>
          <div className="relative w-full" ref={deviceWrapRef}>
            <button
              type="button"
              disabled={!mvRunning}
              onClick={() => setDeviceOpen((v) => !v)}
              className="hub-input flex items-center gap-1.5 w-full"
              style={{
                height: 28,
                fontSize: 12,
                padding: '0 6px',
                background: 'var(--hub-surface)',
                color: 'var(--hub-ink)',
                cursor: mvRunning ? 'pointer' : 'not-allowed',
              }}
            >
              <span className="flex-1 text-left truncate">
                {t(`settings.mvDevice${device.charAt(0).toUpperCase()}${device.slice(1)}`)}
              </span>
              <ChevronDown size={12} style={{ flexShrink: 0, opacity: 0.6 }} />
            </button>
            {deviceOpen && (
              <div
                className="absolute z-50 mt-1 rounded-lg shadow-2xl border overflow-hidden w-full"
                style={{
                  background: 'var(--hub-surface)',
                  borderColor: 'var(--hub-line-2)',
                  maxHeight: 200,
                  overflowY: 'auto',
                }}
              >
                {DEVICES.map((d) => (
                  <button
                    key={d}
                    type="button"
                    onClick={() => {
                      setDeviceOpen(false);
                      if (d !== device) handleDeviceChange(d);
                    }}
                    className="flex items-center justify-between w-full text-left"
                    style={{
                      padding: '6px 10px',
                      fontSize: 12.5,
                      color: d === device ? 'var(--hub-accent)' : 'var(--hub-ink)',
                      background: d === device ? 'var(--hub-accent-soft)' : 'transparent',
                    }}
                    onMouseEnter={(e) => {
                      if (d !== device) e.currentTarget.style.background = 'var(--hub-surface-hover)';
                    }}
                    onMouseLeave={(e) => {
                      if (d !== device) e.currentTarget.style.background = 'transparent';
                    }}
                  >
                    <span>{t(`settings.mvDevice${d.charAt(0).toUpperCase()}${d.slice(1)}`)}</span>
                    {d === device && <Check size={12} style={{ color: 'var(--hub-accent)' }} />}
                  </button>
                ))}
              </div>
            )}
          </div>
          {deviceDirty && (
            <p style={{ fontSize: 11.5, color: 'var(--hub-ink-3)', marginTop: 6 }}>
              {t('settings.mvDeviceRestartHint')}
            </p>
          )}
        </div>
      </div>
      )}
      {reindexConfirmOpen && (
        <ReindexDialog
          open
          mode={reindexMode}
          targetModel={models.find((m) => m.size === (pendingModel ?? currentModel))?.label ?? pendingModel ?? currentModel ?? ''}
          smartEnabled={smartRoutingConfig.enabled}
          onClose={handleReindexCancel}
          onStart={handleStartReindex}
        />
      )}
    </div>
  );
};

export default ModelVectorSettings;
