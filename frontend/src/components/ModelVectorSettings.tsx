import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Boxes, BrainCircuit, Check, ChevronDown, Database, Languages, Loader2, RefreshCw } from 'lucide-react';
import { listen } from '@tauri-apps/api/event';
import ModelSelector, { type ModelDownloadProgress } from '@/components/ui/ModelSelector';
import { SimpleSelect } from '@/components/ui/SimpleSelect';
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
  openModelDir,
} from '@/services/ragService';
import {
  cancelDecisionModelDownload,
  decisionModelDownloadStatus,
  decisionStatus,
  downloadDecisionModel,
  listDecisionModels,
  selectDecisionModel,
  type DecisionStatus,
} from '@/services/decisionService';
import { decisionStatusKey } from '@/utils/decisionFormat';
import { apiGet, apiPut } from '@/utils/fetchInterceptor';
import type { RagModelInfo, RagStatus } from '@/types';

type MvDevice = 'auto' | 'gpu' | 'cpu';
const DEVICES: MvDevice[] = ['auto', 'gpu', 'cpu'];

/** MCP 内置工具描述语言选项（'auto' 跟随界面语言）。语言名按惯例用本语言
 *  书写，不参与翻译。 */
const MCP_TOOL_LANGUAGES: { value: string; label: string }[] = [
  { value: 'auto', label: '' }, // label = t('settings.mcpToolLanguageAuto') at render
  { value: 'en', label: 'English' },
  { value: 'zh', label: '中文' },
  { value: 'fr', label: 'Français' },
  { value: 'tr', label: 'Türkçe' },
  { value: 'vi', label: 'Tiếng Việt' },
  { value: 'es', label: 'Español' },
];

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
  // MCP 内置工具描述语言（system_config.mcp.toolLanguage，默认 en）。
  const [toolLanguage, setToolLanguage] = useState<string>('en');
  // ── 决策支持（合并自原独立【决策模型】卡片）──
  const [decision, setDecision] = useState<DecisionStatus | null>(null);
  const [decisionModels, setDecisionModels] = useState<RagModelInfo[]>([]);
  const [decisionDownload, setDecisionDownload] = useState<ModelDownloadProgress | null>(null);
  const [decisionSelecting, setDecisionSelecting] = useState(false);
  const deviceWrapRef = useRef<HTMLDivElement>(null);
  const mounted = useRef(true);
  // Generation for decision status fetches: a successful select bumps it so
  // an in-flight fetch (started earlier, resolved later) cannot clobber the
  // optimistic post-select state with stale selectionLost data.
  const decisionGenRef = useRef(0);

  // RAG feature toggle (server-side is_enabled) — distinct from the mv
  // runtime running state; the RAG tag reflects the switch, not the model.
  const mvRunning = status?.mvRunning ?? !!status?.enabled;
  const ragEnabled = status?.enabled ?? mvRunning;
  const decisionEnabled = decision?.enabled ?? false;

  // ── 决策支持：状态/模型列表加载（与向量模型同构，独立命名空间）──
  const fetchDecision = async () => {
    const gen = ++decisionGenRef.current;
    try {
      const [st, m] = await Promise.all([decisionStatus(), listDecisionModels()]);
      if (!mounted.current || gen !== decisionGenRef.current) return;
      setDecision(st);
      setDecisionModels(m);
      // Restore-on-mount guard: never overwrite a terminal snapshot with a
      // stale "downloading" read (the bar would stick forever). The single
      // download slot can belong to ANY size — including the lost selection
      // being re-downloaded (which is NOT st.model, the effective fallback),
      // so probe every candidate until the in-flight one is found.
      const candidates = [
        ...new Set([st.model, st.selectionLost, ...m.map((x) => x.size)].filter(Boolean)),
      ] as string[];
      for (const size of candidates) {
        const p = await decisionModelDownloadStatus(size);
        if (!mounted.current || gen !== decisionGenRef.current) return;
        if (p && p.phase === 'downloading') {
          setDecisionDownload((c) => (c && c.phase !== 'downloading' ? c : p));
          break;
        }
      }
    } catch (e) {
      // A superseded fetch's failure must not toast: the newer fetch already
      // landed fresh state — the error channel is reserved for the latest.
      if (!mounted.current || gen !== decisionGenRef.current) return;
      console.warn('[mv] failed to load decision status', e);
      // A silent failure would dim the 决策支持 tag while decision support
      // may actually be ON — surface the failure loudly instead.
      showToast(t('common.operationFailed'), 'error');
    }
  };

  const handleDecisionSelect = async (model: string) => {
    if (decisionSelecting) return;
    setDecisionSelecting(true);
    try {
      await selectDecisionModel(model);
      if (mounted.current) {
        // Invalidate in-flight status fetches: one started before this select
        // would otherwise land later and resurrect the stale selectionLost.
        decisionGenRef.current += 1;
        // selectionLost: null — the new selection is ready (the Rust command
        // validates readiness before persisting); keeping the old lost name
        // would leave a stale warning contradicting the dropdown.
        setDecision((s) => (s ? { ...s, model, modelReady: true, selectionLost: null } : s));
        showToast(t('settings.decisionModelSwitched'), 'success');
      }
    } catch (e) {
      showToast(e instanceof Error ? e.message : String(e), 'error');
    } finally {
      if (mounted.current) setDecisionSelecting(false);
    }
  };

  const handleToolLanguageChange = async (v: string) => {
    try {
      // 后端在 update_system_config 里刷新缓存并按需重建 builtin 索引
      // （描述参与 smart 索引嵌入）。
      await apiPut('system-config', { mcp: { toolLanguage: v } });
      if (mounted.current) setToolLanguage(v);
    } catch (e) {
      showToast(e instanceof Error ? e.message : String(e), 'error');
    }
  };

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
        if (!mounted.current) return;
        // Same stale-restore guard as DecisionModelSettings: never overwrite
        // a terminal snapshot with an in-flight "downloading" read.
        if (p && p.phase === 'downloading') {
          setModelDownload((c) => (c && c.phase !== 'downloading' ? c : p));
        }
      }
    } catch (e) {
      console.warn('[mv] failed to load model/vector status', e);
    }
  };

  useEffect(() => {
    mounted.current = true;
    void fetchAll();
    void fetchDecision();
    // 运行设备：读共享配置 mv.device（deep-merge 持久化）。
    apiGet<Record<string, any>>('config')
      .then((res) => {
        if (!mounted.current) return;
        const d = res?.data?.systemConfig?.mv?.device;
        if (d === 'gpu' || d === 'cpu' || d === 'auto') setDevice(d);
        const tl = res?.data?.systemConfig?.mcp?.toolLanguage;
        if (typeof tl === 'string' && MCP_TOOL_LANGUAGES.some((o) => o.value === tl)) {
          setToolLanguage(tl);
        }
      })
      .catch(() => undefined);
    // 模型下载进度（与 RagPage 同一事件源；两处监听互不影响）。事件按
    // kind 分流：embedding 进度进向量模型状态，decision 进度进决策模型
    // 状态（absent kind = legacy embedding-era payload）。
    const unlistenP = listen<ModelDownloadProgress>('rag://model-download', (event) => {
      const p = event.payload;
      if (!p || !mounted.current) return;
      if (p.kind === 'decision') {
        setDecisionDownload({ ...p });
        if (p.phase === 'done' || p.phase === 'error' || p.phase === 'cancelled') {
          void fetchDecision();
          if (p.phase === 'done') {
            // Identity-guarded clear: a new download within the window keeps
            // its own progress bar. The sentinel must be the SAME object put
            // into state (a copy of p, not p itself).
            const snapshot = { ...p };
            setDecisionDownload(snapshot);
            setTimeout(() => {
              if (!mounted.current) return;
              setDecisionDownload((c) => (c === snapshot ? null : c));
            }, 1500);
          }
        }
        return;
      }
      if (p.phase === 'done' || p.phase === 'error' || p.phase === 'cancelled') {
        // Terminal phases MUST reach state (the render shows the error/
        // cancelled message and ModelSelector's bar keys off phase) — set
        // the snapshot for all three, with the identity-guarded timer only
        // for done.
        const snapshot = { ...p };
        setModelDownload(snapshot);
        void fetchAll();
        if (p.phase === 'done') {
          // Identity-guarded clear: a new download within the window keeps
          // its own progress bar. The sentinel must be the SAME object put
          // into state (a copy of p, not p itself).
          setTimeout(() => {
            if (!mounted.current) return;
            setModelDownload((c) => (c === snapshot ? null : c));
          }, 1500);
        }
        return;
      }
      setModelDownload({ ...p });
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

  // 决策模型运行状态行（与 RAG 的 statusText 同构）：运行中显示模型名，
  // 启用但模型丢失/未运行分别给可操作文案（纯函数映射，单测覆盖）。
  // decision === null = 状态未加载/加载失败——显示「—」不得断言「未运行」
  // （flag 可能实际是开的，与 DecisionPage 的三态处理一致）。
  const decisionCurrentInfo = decisionModels.find((m) => m.size === decision?.model);
  const decisionModelLabel = decisionCurrentInfo?.label || decision?.model || '—';
  // 丢失模型用显示名（label）而非原始 size 名（"1"），与状态行/列表一致。
  // 「重新下载」承诺的可达性门：目录里有该 size 且真的渲染下载动作
  // （downloadable——deploy.json 无 downloadUrls 的条目在列表中无任何按钮，
  // ready 条目也不需要下载）。不满足时走 Gone 变体文案，只承诺可达出路。
  const decisionLostInCatalog = !!(
    decision?.selectionLost &&
    decisionModels.some(
      (m) => m.size === decision.selectionLost && (m.ready || m.downloadable),
    )
  );
  const decisionLostLabel = decision?.selectionLost
    ? decisionModels.find((m) => m.size === decision.selectionLost)?.label || decision.selectionLost
    : '';
  const decisionRunning = decisionEnabled && (decision?.modelReady ?? false);
  const decisionStatusText =
    decision === null
      ? '—'
      : t(decisionStatusKey(decisionEnabled, decision?.modelReady ?? false), {
          model: decisionModelLabel,
        });

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
            title={t('settings.mvSourceRag')}
            style={ragEnabled
              ? { background: 'oklch(0.66 0.15 145 / 0.15)', color: 'var(--hub-ok)', borderColor: 'transparent' }
              : { opacity: 0.45 }}
          >
            {t('settings.mvSourceRag')}
          </span>
          <span
            className="hub-tag"
            title={t('settings.decisionTitle')}
            style={decisionEnabled
              ? { background: 'oklch(0.66 0.15 145 / 0.15)', color: 'var(--hub-ok)', borderColor: 'transparent' }
              : { opacity: 0.45 }}
          >
            {t('settings.decisionTitle')}
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

        {/* MCP 工具语言：内置工具（rag_*、decision_decide、smart_route_* 等）
            的描述语言；默认英文，「跟随界面语言」时由前端同步的 uiLanguage
            解析。作用于 tools/list 与两个「查看工具」弹框。 */}
        <div className="rounded-lg p-4 bg-gray-50 dark:bg-[var(--hub-item)]">
          <div className="flex items-center gap-2 mb-2">
            <Languages size={13} className="text-[var(--hub-ink-3)]" />
            <h3 className="text-[13px] font-medium flex-1" style={{ color: 'var(--hub-ink)' }}>
              {t('settings.mcpToolLanguage')}
            </h3>
          </div>
          <SimpleSelect
            value={toolLanguage}
            onChange={handleToolLanguageChange}
            options={MCP_TOOL_LANGUAGES.map((o) => ({
              value: o.value,
              label: o.value === 'auto' ? t('settings.mcpToolLanguageAuto') : o.label,
            }))}
            ariaLabel={t('settings.mcpToolLanguage')}
          />
          <p style={{ fontSize: 11.5, color: 'var(--hub-ink-3)', marginTop: 6 }}>
            {t('settings.mcpToolLanguageHint')}
          </p>
        </div>

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
            onOpenFolder={(size) => {
              openModelDir(size).catch((e) =>
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

        {/* ── 决策模型（独立分段，border-t 与向量模型分区）：仅模型管理——
            启用开关在决策支持页面标题旁（与 RAG 页同构）。与向量模型完全
            同构（同一套 deploy.json 契约与下载机制），仅模型根目录
            （decision/）与选择键（decision.model）不同；切换模型无需重建
            索引（决策模型无向量库），即时生效。 */}
        <div className="pt-4 space-y-3 border-t" style={{ borderColor: 'var(--hub-line-2)' }}>
          {/* 运行状态（决策模型）：与 RAG 运行状态行同构。 */}
          <div className="flex items-center gap-2 flex-wrap">
            <span className="hub-status" data-state={decisionRunning ? 'on' : 'off'}>
              <span
                className="hub-dot"
                style={{
                  background: decisionRunning ? 'var(--hub-ok)' : 'var(--hub-ink-3)',
                  boxShadow: decisionRunning ? '0 0 0 3px oklch(0.66 0.15 145 / 0.15)' : 'none',
                }}
              />
              <span
                style={{
                  fontSize: 12.5,
                  color: decisionRunning ? 'var(--hub-ink)' : 'var(--hub-ink-3)',
                }}
              >
                {decisionStatusText}
              </span>
            </span>
            {decisionSelecting && <Loader2 size={12} className="animate-spin text-[var(--hub-ink-3)]" />}
          </div>
          <p style={{ fontSize: 12, color: 'var(--hub-ink-3)' }}>{t('settings.decisionHint')}</p>

          {/* 模型丢失警告仅在启用时显示——未启用时下拉被 mv-disabled 置灰，
              「重新下载」不可达，此时提示只会给出无法执行的动作。丢失态由
              selectionLost 报告（decide 路径的 ready 回退会把它藏进 model）。 */}
          {decisionEnabled && decision?.selectionLost && (
            <p className="text-[12px] font-medium" style={{ color: 'var(--hub-warn, #d97706)' }}>
              {/* 目录里仍有该 size 才承诺「重新下载」——size 目录被移除时
                  下载按钮与下载源都不存在，只给可达的出路（选别的模型）。 */}
              {t(
                decisionLostInCatalog
                  ? 'settings.decisionModelMissing'
                  : 'settings.decisionModelGone',
                { model: decisionLostLabel },
              )}
            </p>
          )}

          <div className={`rounded-lg p-4 bg-gray-50 dark:bg-[var(--hub-item)] ${decisionEnabled ? '' : 'mv-disabled'}`}>
            <div className="flex items-center gap-2 mb-2">
              <BrainCircuit size={13} className="text-[var(--hub-ink-3)]" />
              <h3 className="text-[13px] font-medium flex-1" style={{ color: 'var(--hub-ink)' }}>
                {t('settings.decisionTitle')}
              </h3>
            </div>
            <ModelSelector
              models={decisionModels}
              // Lost selection → keep the picker empty so the running
              // fallback model is adoptable (ModelSelector disables the
              // 使用 button on the "current" entry, which would dead-end
              // the recovery path when current ≠ persisted selection).
              currentModel={decision?.selectionLost ? null : (decision?.model ?? null)}
              modelDownload={decisionDownload}
              disabled={!decisionEnabled || decisionSelecting}
              onSelect={handleDecisionSelect}
              onDownload={(model, source) => {
                downloadDecisionModel(model, source).catch((e) =>
                  showToast(e instanceof Error ? e.message : String(e), 'error'),
                );
              }}
              onCancel={(model) => {
                cancelDecisionModelDownload(model).catch((e) =>
                  showToast(e instanceof Error ? e.message : String(e), 'error'),
                );
              }}
              onRefresh={fetchDecision}
            />
            {decisionDownload && decisionDownload.phase !== 'downloading' && (
              <p style={{ fontSize: 11.5, color: 'var(--hub-ink-3)', marginTop: 6 }}>
                {decisionDownload.phase === 'error'
                  ? decisionDownload.message || t('settings.mvDownloadFailed')
                  : decisionDownload.phase === 'cancelled'
                  ? t('settings.mvDownloadCancelled')
                  : t('settings.mvDownloadDone')}
              </p>
            )}
            {/* 与上方警告同门控 + modelReady（确有回退模型在跑）：未启用时
                动作不可达；无回退模型时「代为决策」为假——两种情形下这行
                都是失实/不可执行的指引。 */}
            {decisionEnabled && decision?.selectionLost && decision?.modelReady && (
              <p style={{ fontSize: 11.5, color: 'var(--hub-ink-3)', marginTop: 6 }}>
                {t(
                  decisionLostInCatalog
                    ? 'settings.decisionLostHint'
                    : 'settings.decisionLostHintGone',
                )}
              </p>
            )}
          </div>
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
