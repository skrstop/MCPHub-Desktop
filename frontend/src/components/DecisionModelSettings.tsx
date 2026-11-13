import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { BrainCircuit, Loader2 } from 'lucide-react';
import { listen } from '@tauri-apps/api/event';
import { Switch } from '@/components/ui/ToggleGroup';
import ModelSelector, { type ModelDownloadProgress } from '@/components/ui/ModelSelector';
import {
  cancelDecisionModelDownload,
  decisionModelDownloadStatus,
  decisionSetEnabled,
  decisionStatus,
  downloadDecisionModel,
  listDecisionModels,
  selectDecisionModel,
  type DecisionStatus,
} from '@/services/decisionService';
import { useToast } from '@/contexts/ToastContext';
import type { RagModelInfo } from '@/types';

/** 设置页【决策模型】卡片（桌面端专属）：决策支持开关 + 决策模型管理
 *  （列表/选择/下载/取消），与【向量模型】完全同构——同一套 deploy.json
 *  契约与下载机制，仅模型根目录（decision/）与选择键（decision.model）
 *  不同。切换模型无需重建索引（决策模型无向量库），即时生效。 */
const DecisionModelSettings: React.FC = () => {
  const { t } = useTranslation();
  const { showToast } = useToast();

  const [status, setStatus] = useState<DecisionStatus | null>(null);
  const [models, setModels] = useState<RagModelInfo[]>([]);
  const [modelDownload, setModelDownload] = useState<ModelDownloadProgress | null>(null);
  const [toggling, setToggling] = useState(false);
  const [sectionOpen, setSectionOpen] = useState(false);
  const mounted = useRef(true);

  const fetchAll = async () => {
    try {
      const [st, m] = await Promise.all([decisionStatus(), listDecisionModels()]);
      if (!mounted.current) return;
      setStatus(st);
      setModels(m);
      if (st.model) {
        const p = await decisionModelDownloadStatus(st.model);
        if (mounted.current && p && p.phase === 'downloading') setModelDownload(p);
      }
    } catch (e) {
      console.warn('[decision] failed to load status', e);
    }
  };

  useEffect(() => {
    mounted.current = true;
    void fetchAll();
    const unlistenP = listen<ModelDownloadProgress>('rag://model-download', (event) => {
      const p = event.payload;
      if (!p || !mounted.current) return;
      // The download event bus is shared with the embedding settings page;
      // only react to decision-kind downloads (absent kind = legacy
      // embedding-era payload).
      if ((p.kind ?? 'embedding') !== 'decision') return;
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

  const handleToggle = async (enabled: boolean) => {
    setToggling(true);
    try {
      await decisionSetEnabled(enabled);
      if (mounted.current) {
        setStatus((s) => (s ? { ...s, enabled } : s));
        showToast(
          enabled ? t('settings.decisionEnabledToast') : t('settings.decisionDisabledToast'),
          'success',
        );
      }
    } catch (e) {
      showToast(e instanceof Error ? e.message : String(e), 'error');
    } finally {
      if (mounted.current) setToggling(false);
    }
  };

  const handleSelectModel = async (model: string) => {
    try {
      await selectDecisionModel(model);
      if (mounted.current) {
        setStatus((s) => (s ? { ...s, model, modelReady: true } : s));
        showToast(t('settings.decisionModelSwitched'), 'success');
      }
    } catch (e) {
      showToast(e instanceof Error ? e.message : String(e), 'error');
    }
  };

  const enabled = status?.enabled ?? false;

  return (
    <div className="hub-card mb-6 overflow-visible">
      <div
        className="flex justify-between items-center cursor-pointer transition-colors hover:bg-[var(--hub-surface-hover)] py-3 px-5"
        onClick={() => setSectionOpen((v) => !v)}
      >
        <div className="flex items-center gap-2.5">
          <BrainCircuit size={15} className="text-[var(--hub-ink-2)]" />
          <h2 className="font-medium text-[var(--hub-ink)]">{t('settings.decisionTitle')}</h2>
          <span
            className="hub-tag"
            style={enabled
              ? { background: 'oklch(0.66 0.15 145 / 0.15)', color: 'var(--hub-ok)', borderColor: 'transparent' }
              : { opacity: 0.45 }}
          >
            {enabled ? t('pages.decision.enabledTag') : t('pages.decision.disabledTag')}
          </span>
        </div>
        <span className="text-[var(--hub-ink-3)]">{sectionOpen ? '−' : '+'}</span>
      </div>

      {sectionOpen && (
        <div className="px-5 py-4 border-t space-y-4" style={{ borderColor: 'var(--hub-line-2)' }}>
          <div className="flex items-center justify-between p-3 bg-gray-50 dark:bg-[var(--hub-item)] rounded-md">
            <div>
              <h3 className="font-medium text-gray-700">{t('settings.decisionEnable')}</h3>
              <p className="text-sm text-gray-500">{t('settings.decisionEnableDescription')}</p>
            </div>
            <div className="flex items-center gap-2">
              {toggling && <Loader2 size={13} className="animate-spin text-[var(--hub-ink-3)]" />}
              <Switch disabled={toggling} checked={enabled} onCheckedChange={handleToggle} />
            </div>
          </div>
          <p style={{ fontSize: 12, color: 'var(--hub-ink-3)' }}>{t('settings.decisionHint')}</p>

          <div className={`rounded-lg p-4 bg-gray-50 dark:bg-[var(--hub-item)] ${enabled ? '' : 'mv-disabled'}`}>
            <div className="flex items-center gap-2 mb-2">
              <h3 className="text-[13px] font-medium flex-1" style={{ color: 'var(--hub-ink)' }}>
                {t('settings.decisionModelSwitch')}
              </h3>
            </div>
            <ModelSelector
              models={models}
              currentModel={status?.model ?? null}
              modelDownload={modelDownload}
              disabled={!enabled || toggling}
              onSelect={handleSelectModel}
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
        </div>
      )}
    </div>
  );
};

export default DecisionModelSettings;
