import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Boxes, Check, ChevronDown, Database, Loader2 } from 'lucide-react';
import { listen } from '@tauri-apps/api/event';
import ModelSelector, { type ModelDownloadProgress } from '@/components/ui/ModelSelector';
import { useSettings } from '@/contexts/SettingsContext';
import { useToast } from '@/contexts/ToastContext';
import {
  ragStatus,
  listRagModels,
  currentRagModel,
  selectRagModel,
  downloadRagModel,
  reindexAllRag,
} from '@/services/ragService';
import { apiGet, apiPut } from '@/utils/fetchInterceptor';
import type { RagModelInfo, RagStatus } from '@/types';

type MvDevice = 'auto' | 'gpu' | 'cpu';
const DEVICES: MvDevice[] = ['auto', 'gpu', 'cpu'];

/** 设置页【模型和向量】卡片（桌面端专属）：共享的本地 embedding 模型与向量库
 *  运行时的统一管理入口。RAG 与 Smart Routing 是当前的两个消费方——任一开启
 *  即自动加载模型（全局单实例），全部关闭后释放。Phase 1 仅 UI + 配置持久化；
 *  运行状态判定暂用 ragStatus.enabled（Phase 2 改为共享运行时状态）。 */
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
  const [reindexing, setReindexing] = useState(false);
  const [sectionOpen, setSectionOpen] = useState(false);
  const deviceWrapRef = useRef<HTMLDivElement>(null);
  const mounted = useRef(true);

  // Phase 2：mvRunning = 共享运行时存活（RAG 或 Smart Routing 任一驱动）；
  // 回退 status.enabled 兼容旧后端。维度来自模型加载时读的真实 embed_dim。
  const mvRunning = status?.mvRunning ?? !!status?.enabled;
  // RAG feature toggle (server-side is_enabled) — distinct from the mv
  // runtime running state; the RAG tag reflects the switch, not the model.
  // ragStatus here is the fetch FUNCTION (the prop slot is a misnomer); the
  // fetched status lands in `status`, whose `enabled` field is the toggle.
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
    } catch (e) {
      console.warn('[mv] failed to load model/vector status', e);
    }
  };

  useEffect(() => {
    mounted.current = true;
    void fetchAll();
    // 运行设备：读共享配置 mv.device（deep-merge 持久化；后端 Phase 2 才消费）。
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
      if (p.phase === 'done' || p.phase === 'error') {
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
    setSwitching(true);
    try {
      const st = await selectRagModel(size);
      if (!mounted.current) return;
      setCurrentModel(size);
      setStatus(st);
      if (st.needsReindex) {
        setReindexConfirmOpen(true);
      } else {
        showToast(t('settings.mvModelSwitched'), 'success');
      }
    } catch (e) {
      showToast(e instanceof Error ? e.message : String(e), 'error');
    } finally {
      if (mounted.current) setSwitching(false);
    }
  };

  const handleConfirmReindex = async () => {
    setReindexing(true);
    try {
      await reindexAllRag();
      if (mounted.current) {
        setReindexConfirmOpen(false);
        showToast(t('settings.mvReindexDone'), 'success');
        void fetchAll();
      }
    } catch (e) {
      showToast(e instanceof Error ? e.message : String(e), 'error');
    } finally {
      if (mounted.current) setReindexing(false);
    }
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
  // 维度来自后端真实值（模型加载时读 GGUF embed_dim；未加载时无）。Phase 2
  // mv::status() 落地后同一字段由共享运行时提供，前端逻辑不变。
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
          {/* Capability tags: green when the feature is enabled, grayed when
              off — consistent with the running-status dot above. */}
          <span
            className="hub-tag"
            style={
              smartRoutingConfig.enabled
                ? { background: 'oklch(0.66 0.15 145 / 0.15)', color: 'var(--hub-ok)', borderColor: 'transparent' }
                : { opacity: 0.45 }
            }
          >
            {t('settings.mvSourceSmartRouting')}
          </span>
          <span
            className="hub-tag"
            style={
              ragEnabled
                ? { background: 'oklch(0.66 0.15 145 / 0.15)', color: 'var(--hub-ok)', borderColor: 'transparent' }
                : { opacity: 0.45 }
            }
          >
            RAG
          </span>
          {status?.initializing && <Loader2 size={12} className="animate-spin text-[var(--hub-ink-3)]" />}
        </div>
        <p style={{ fontSize: 12, color: 'var(--hub-ink-3)' }}>{t('settings.mvHint')}</p>

        {/* 模型切换（唯一入口；未启动时禁用） */}
        <div className={mvRunning ? '' : 'mv-disabled'}>
          <div className="flex items-center gap-2 mb-2">
            <Database size={13} className="text-[var(--hub-ink-3)]" />
            <h3 className="text-[13px] font-medium" style={{ color: 'var(--hub-ink)' }}>
              {t('settings.mvModelSwitch')}
            </h3>
          </div>
          <ModelSelector
            models={models}
            currentModel={currentModel}
            modelDownload={modelDownload}
            disabled={!mvRunning || status?.initializing || switching}
            onSelect={handleSelectModel}
            onDownload={(size) => {
              downloadRagModel(size).catch((e) =>
                showToast(e instanceof Error ? e.message : String(e), 'error'),
              );
            }}
            onRefresh={fetchAll}
          />
          {modelDownload && modelDownload.phase !== 'downloading' && (
            <p style={{ fontSize: 11.5, color: 'var(--hub-ink-3)', marginTop: 6 }}>
              {modelDownload.phase === 'error'
                ? modelDownload.message || t('settings.mvDownloadFailed')
                : t('settings.mvDownloadDone')}
            </p>
          )}
        </div>

        {/* 运行设备（AUTO/GPU/CPU）：Phase 1 仅持久化 mv.device，后端 Phase 2 消费。
            样式与 ModelSelector 触发框对齐（同高/同字号/自定义箭头）。 */}
        <div className={mvRunning ? '' : 'mv-disabled'}>
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
                cursor: !mvRunning ? 'not-allowed' : 'pointer',
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
                      if (d !== device) void handleDeviceChange(d);
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

      {/* 模型切换导致维度变化 → RAG 文档需重嵌确认（与 RagPage 同语义） */}
      {reindexConfirmOpen && (
        <div className="fixed inset-0 bg-black/50 z-[60] flex items-center justify-center p-4" style={{ pointerEvents: 'auto' }}>
          <div
            className="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-md w-full mx-4 border"
            style={{ borderColor: 'var(--hub-line-2)' }}
          >
            <div
              className="flex items-center justify-between p-5 border-b"
              style={{ borderColor: 'var(--hub-line-2)' }}
            >
              <h2 className="text-lg font-bold" style={{ color: 'var(--hub-ink)' }}>
                {t('pages.rag.reindexConfirmTitle')}
              </h2>
            </div>
            <div className="p-5 space-y-3">
              <p className="text-[13px] leading-relaxed" style={{ color: 'var(--hub-ink-3)' }}>
                {t('pages.rag.reindexConfirmMessage')}
              </p>
            </div>
            <div
              className="flex justify-end gap-2 p-5 pt-3 border-t"
              style={{ borderColor: 'var(--hub-line-2)' }}
            >
              <button onClick={() => setReindexConfirmOpen(false)} className="hub-btn" disabled={reindexing}>
                {t('pages.rag.cancel')}
              </button>
              <button onClick={handleConfirmReindex} className="hub-btn primary" disabled={reindexing}>
                {reindexing ? <Loader2 size={13} className="animate-spin" /> : null}
                {t('pages.rag.reindexConfirmButton')}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
};

export default ModelVectorSettings;
