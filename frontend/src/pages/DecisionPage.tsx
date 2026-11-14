import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { BrainCircuit, Eye, Loader2, Trash2 } from 'lucide-react';
import { Switch } from '@/components/ui/ToggleGroup';
import ConfirmDialog from '@/components/ui/ConfirmDialog';
import {
  clearDecisionHistory,
  decisionSetEnabled,
  decisionStatus,
  listDecisionHistory,
  type DecisionHistoryEntry,
} from '@/services/decisionService';
import { useToast } from '@/contexts/ToastContext';
import { isTauri } from '@/utils/tauriClient';
import {
  decisionTypeLabelKey,
  formatProbability,
  formatScore,
  mergeHistoryPage,
} from '@/utils/decisionFormat';

const PAGE_SIZE = 50;

/** 决策支持页（桌面端专属）：决策运行开关 + 请求决策历史（按请求时间倒序）。
 *  每行可查看详情（弹框）；支持一键清空全部历史。历史同时由全局定时清理
 *  任务按 15 天保留期自动清理（与活动日志一致）。 */
const DecisionPage: React.FC = () => {
  const { t } = useTranslation();
  const { showToast } = useToast();

  const [enabled, setEnabled] = useState(false);
  const [toggling, setToggling] = useState(false);
  const [loading, setLoading] = useState(true);
  const [entries, setEntries] = useState<DecisionHistoryEntry[]>([]);
  const [loadingMore, setLoadingMore] = useState(false);
  // True while the backend still has rows beyond the loaded page (heuristic:
  // a full page came back — the backend clamps limit to 500, so a full page
  // is the only reliable "maybe more" signal short of a count query).
  const [hasMore, setHasMore] = useState(false);
  // Initial-load failure: distinct from "empty history" — the user must be
  // able to tell a backend/IPC error from a genuinely empty list, and retry.
  const [loadError, setLoadError] = useState(false);
  const [viewing, setViewing] = useState<DecisionHistoryEntry | null>(null);
  const [clearConfirm, setClearConfirm] = useState(false);
  const [clearing, setClearing] = useState(false);
  const mounted = useRef(true);
  // True while a toggle is in flight — fetchAll consults it so a stale
  // pre-toggle status can't revert an accepted toggle.
  const togglingRef = useRef(false);
  // Monotonic generation for status fetches: a toggle bumps it, so an
  // in-flight fetch that resolves AFTER the toggle completes (guard window
  // already closed) still discards its stale pre-toggle status.
  const statusGenRef = useRef(0);
  // Element that opened the detail dialog — focus is restored to it on close.
  const viewTriggerRef = useRef<HTMLElement | null>(null);

  const fetchAll = useCallback(async () => {
    setLoading(true);
    const gen = ++statusGenRef.current;
    try {
      const [st, list] = await Promise.all([decisionStatus(), listDecisionHistory(PAGE_SIZE, 0)]);
      if (!mounted.current) return;
      // Discard a stale status: a toggle that started after this fetch (or
      // completed while it was in flight) owns the enabled state now.
      if (gen === statusGenRef.current && !togglingRef.current) {
        setEnabled(st.enabled);
      }
      setEntries(list);
      setHasMore(list.length >= PAGE_SIZE);
      setLoadError(false);
    } catch (e) {
      console.warn('[decision] failed to load page data', e);
      if (mounted.current) setLoadError(true);
    } finally {
      if (mounted.current) setLoading(false);
    }
  }, []);

  const loadMore = async () => {
    setLoadingMore(true);
    try {
      // Refetch the whole loaded prefix (offset 0) instead of paging by
      // offset: rows inserted between page fetches shift the window and
      // offset paging would permanently skip them. The backend clamps limit
      // to 500, so the prefix refetch doubles as the browsing depth cap.
      const nextLimit = Math.min(entries.length + PAGE_SIZE, 500);
      const list = await listDecisionHistory(nextLimit, 0);
      if (!mounted.current) return;
      setEntries((prev) => mergeHistoryPage(prev, list));
      setHasMore(list.length >= nextLimit && nextLimit < 500);
    } catch (e) {
      showToast(e instanceof Error ? e.message : String(e), 'error');
    } finally {
      if (mounted.current) setLoadingMore(false);
    }
  };

  useEffect(() => {
    mounted.current = true;
    void fetchAll();
    return () => {
      mounted.current = false;
    };
  }, [fetchAll]);

  const handleToggle = async (on: boolean) => {
    setToggling(true);
    togglingRef.current = true;
    statusGenRef.current += 1; // invalidate any in-flight status fetch
    try {
      await decisionSetEnabled(on);
      if (mounted.current) {
        setEnabled(on);
        showToast(
          on ? t('pages.decision.enableToast') : t('pages.decision.disableToast'),
          'success',
        );
      }
    } catch (e) {
      showToast(e instanceof Error ? e.message : String(e), 'error');
    } finally {
      togglingRef.current = false;
      if (mounted.current) setToggling(false);
    }
  };

  const handleClear = async () => {
    setClearing(true);
    try {
      const n = await clearDecisionHistory();
      if (mounted.current) {
        setEntries([]);
        setHasMore(false);
        setClearConfirm(false);
        showToast(t('pages.decision.clearedToast', { count: n }), 'success');
      }
    } catch (e) {
      showToast(e instanceof Error ? e.message : String(e), 'error');
      if (mounted.current) setClearConfirm(false);
    } finally {
      if (mounted.current) setClearing(false);
    }
  };

  const typeLabel = (ty: string) => t(decisionTypeLabelKey(ty));

  // Escape closes the TOPMOST dialog only (document-level listener — the
  // repo's established dialog pattern; overlay onKeyDown would miss when
  // focus sits outside the dialog). The clear-confirm can stack above the
  // detail dialog: it closes first, the detail dialog on the next Escape.
  useEffect(() => {
    if (!viewing && !clearConfirm) return;
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key !== 'Escape') return;
      if (clearConfirm) {
        setClearConfirm(false);
        return;
      }
      setViewing(null);
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [viewing, clearConfirm]);

  // Restore focus to the trigger that opened the detail dialog when it
  // closes (keyboard users would otherwise be dropped at <body>).
  useEffect(() => {
    if (!viewing) {
      viewTriggerRef.current?.focus();
      viewTriggerRef.current = null;
    }
  }, [viewing]);

  return (
    <div className="p-6 max-w-6xl mx-auto" data-testid="decision-page">
      {/* Header: title + enable switch */}
      <div className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-xl font-semibold text-[var(--hub-ink)] flex items-center gap-2">
            <BrainCircuit size={20} className="text-[var(--hub-ink-2)]" />
            {t('pages.decision.title')}
          </h1>
          <p className="text-sm text-[var(--hub-ink-3)] mt-1">{t('pages.decision.subtitle')}</p>
        </div>
        <div className="flex items-center gap-2">
          {toggling && <Loader2 size={14} className="animate-spin text-[var(--hub-ink-3)]" />}
          <Switch
            disabled={toggling}
            checked={enabled}
            onCheckedChange={handleToggle}
            aria-label={t('pages.decision.title')}
          />
        </div>
      </div>

      {/* History list */}
      <div className="hub-card">
        <div className="flex items-center justify-between py-3 px-5 border-b border-[var(--hub-line-2)]">
          <h2 className="font-medium text-[var(--hub-ink)]">{t('pages.decision.historyTitle')}</h2>
          <button
            type="button"
            className="hub-btn sm"
            disabled={clearing || entries.length === 0}
            onClick={() => setClearConfirm(true)}
          >
            {clearing ? <Loader2 size={12} className="animate-spin" /> : <Trash2 size={12} />}
            {t('pages.decision.clearAll')}
          </button>
        </div>
        {loading ? (
          <div className="flex items-center justify-center py-12 text-[var(--hub-ink-3)]">
            <Loader2 size={16} className="animate-spin mr-2" />
            {t('common.loading')}
          </div>
        ) : loadError ? (
          <div className="py-12 text-center text-sm text-[var(--hub-ink-3)]">
            {t('pages.decision.loadFailed')}
            <div className="mt-3">
              <button type="button" className="hub-btn sm" onClick={() => void fetchAll()}>
                {t('pages.decision.retry')}
              </button>
            </div>
          </div>
        ) : entries.length === 0 ? (
          <div className="py-12 text-center text-sm text-[var(--hub-ink-3)]">
            {t('pages.decision.empty')}
          </div>
        ) : (
          <>
          <ul className="divide-y divide-[var(--hub-line-2)]">
            {entries.map((e) => (
              <li key={e.id} className="flex items-center gap-3 py-2.5 px-5">
                <div className="flex-1 min-w-0">
                  <div className="flex items-center gap-2 flex-wrap">
                    <span className="hub-tag">{typeLabel(e.decisionType)}</span>
                    <span
                      className="text-[13px] text-[var(--hub-ink)] truncate"
                      style={{ maxWidth: 420 }}
                      title={e.question}
                    >
                      {e.question}
                    </span>
                  </div>
                  <div className="text-[11.5px] text-[var(--hub-ink-3)] mt-0.5 flex gap-3">
                    <span>{e.createdAt}</span>
                    <span>
                      {t('pages.decision.selected')}: {e.selectedOption}
                    </span>
                    {e.durationMs != null && <span>{e.durationMs}ms</span>}
                    {e.source && <span>{e.source}</span>}
                  </div>
                </div>
                <button
                  type="button"
                  className="hub-btn sm"
                  style={{ flexShrink: 0 }}
                  onClick={(ev) => {
                    viewTriggerRef.current = ev.currentTarget;
                    setViewing(e);
                  }}
                >
                  <Eye size={12} /> {t('pages.decision.view')}
                </button>
              </li>
            ))}
          </ul>
          {hasMore && (
            <div className="flex justify-center py-3 border-t border-[var(--hub-line-2)]">
              <button type="button" className="hub-btn sm" disabled={loadingMore} onClick={loadMore}>
                {loadingMore && <Loader2 size={12} className="animate-spin" />}
                {t('pages.decision.loadMore')}
              </button>
            </div>
          )}
          </>
        )}
      </div>

      {/* Detail dialog */}
      {viewing && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center"
          style={{ background: 'rgba(0,0,0,0.45)' }}
          onClick={() => setViewing(null)}
          role="dialog"
          aria-modal="true"
          aria-labelledby="decision-detail-title"
        >
          <div
            className="hub-card w-full max-w-2xl max-h-[80vh] overflow-y-auto"
            style={{ margin: '0 16px' }}
            onClick={(ev) => ev.stopPropagation()}
          >
            <div className="flex items-center justify-between py-3 px-5 border-b border-[var(--hub-line-2)]">
              <h3 id="decision-detail-title" className="font-medium text-[var(--hub-ink)]">
                {t('pages.decision.detailTitle')}
              </h3>
              <button type="button" className="hub-btn sm" autoFocus onClick={() => setViewing(null)}>
                {t('common.close')}
              </button>
            </div>
            <div className="px-5 py-4 space-y-3 text-[13px]">
              <div>
                <div className="text-[var(--hub-ink-3)] mb-1">{t('pages.decision.fieldType')}</div>
                <span className="hub-tag">{typeLabel(viewing.decisionType)}</span>
              </div>
              <div>
                <div className="text-[var(--hub-ink-3)] mb-1">{t('pages.decision.fieldTime')}</div>
                <div className="text-[var(--hub-ink)]">{viewing.createdAt}</div>
              </div>
              {viewing.state != null && viewing.state !== '' && (
                <div>
                  <div className="text-[var(--hub-ink-3)] mb-1">{t('pages.decision.fieldState')}</div>
                  <pre
                    className="whitespace-pre-wrap break-all rounded p-2 text-[12px]"
                    style={{ background: 'var(--hub-item)', color: 'var(--hub-ink)' }}
                  >
                    {viewing.state}
                  </pre>
                </div>
              )}
              <div>
                <div className="text-[var(--hub-ink-3)] mb-1">{t('pages.decision.fieldQuestion')}</div>
                <div className="text-[var(--hub-ink)] break-all">{viewing.question}</div>
              </div>
              <div>
                <div className="text-[var(--hub-ink-3)] mb-1">{t('pages.decision.fieldOptions')}</div>
                <ul className="space-y-1.5">
                  {viewing.options.map((opt, i) => {
                    const prob = viewing.probabilities[i];
                    const score = viewing.scores[i];
                    const selected = i === viewing.selectedIndex;
                    return (
                      <li
                        key={i}
                        className="flex items-center gap-2 rounded px-2 py-1.5"
                        style={{
                          background: selected ? 'var(--hub-accent-soft)' : 'var(--hub-item)',
                          outline: selected ? '1px solid var(--hub-accent)' : 'none',
                        }}
                      >
                        <span className="flex-1 min-w-0 truncate text-[var(--hub-ink)]" title={opt}>
                          {opt}
                        </span>
                        {typeof score === 'number' && (
                          <span className="text-[11.5px] text-[var(--hub-ink-3)]">
                            {t('pages.decision.score')}: {formatScore(score)}
                          </span>
                        )}
                        {typeof prob === 'number' && (
                          <span className="text-[11.5px] text-[var(--hub-ink-3)]">
                            {t('pages.decision.probability')}: {formatProbability(prob)}
                          </span>
                        )}
                        {selected && (
                          <span className="hub-tag" style={{ color: 'var(--hub-accent)' }}>
                            {t('pages.decision.selectedTag')}
                          </span>
                        )}
                      </li>
                    );
                  })}
                </ul>
              </div>
              <div className="flex gap-4 text-[11.5px] text-[var(--hub-ink-3)]">
                {viewing.durationMs != null && (
                  <span>
                    {t('pages.decision.fieldDuration')}: {viewing.durationMs}ms
                  </span>
                )}
                {viewing.source && (
                  <span>
                    {t('pages.decision.fieldSource')}: {viewing.source}
                  </span>
                )}
              </div>
            </div>
          </div>
        </div>
      )}

      {clearConfirm && (
        <ConfirmDialog
          isOpen
          title={t('pages.decision.clearConfirmTitle')}
          message={
            hasMore
              ? t('pages.decision.clearConfirmAll')
              : t('pages.decision.clearConfirmMessage', { count: entries.length })
          }
          confirmText={t('pages.decision.clearAll')}
          onConfirm={handleClear}
          onClose={() => setClearConfirm(false)}
        />
      )}
    </div>
  );
};

export default isTauri() ? DecisionPage : () => null;
