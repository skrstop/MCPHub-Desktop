import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { BrainCircuit, ChevronLeft, ChevronRight, Eye, Loader2, Sparkles, Trash2, X } from 'lucide-react';
import { Switch } from '@/components/ui/ToggleGroup';
import ConfirmDialog from '@/components/ui/ConfirmDialog';
import {
  clearDecisionHistory,
  decisionSetEnabled,
  decisionStatus,
  getDecisionTools,
  listDecisionHistory,
  type DecisionHistoryEntry,
} from '@/services/decisionService';
import { useToast } from '@/contexts/ToastContext';
import { isTauri } from '@/utils/tauriClient';
import {
  decisionTypeLabelKey,
  formatProbability,
  formatScore,
    pageSlice,
} from '@/utils/decisionFormat';

const PAGE_SIZE = 50;

/** 决策支持页（桌面端专属）：决策运行开关 + 请求决策历史（按请求时间倒序）。
 *  每行可查看详情（弹框）；支持一键清空全部历史。历史同时由全局定时清理
 *  任务按 15 天保留期自动清理（与活动日志一致）。 */
const DecisionPage: React.FC = () => {
  const { t } = useTranslation();
  const { showToast } = useToast();

  // null until the first successful status read — a load failure must not
  // assert "已关闭" from the uninitialized default (the flag may really be ON).
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [toggling, setToggling] = useState(false);
  const [loading, setLoading] = useState(true);
  const [entries, setEntries] = useState<DecisionHistoryEntry[]>([]);
  // True while the backend still has rows beyond the loaded page (heuristic:
  // fetch PAGE_SIZE+1 rows — an extra row is the only reliable "maybe more"
  // signal short of a count query; it is sliced off before display).
  const [hasMore, setHasMore] = useState(false);
  // 1-based page number for the offset-paged history list.
  const [page, setPage] = useState(1);
  // Initial-load failure: distinct from "empty history" — the user must be
  // able to tell a backend/IPC error from a genuinely empty list, and retry.
  const [loadError, setLoadError] = useState(false);
  const [viewing, setViewing] = useState<DecisionHistoryEntry | null>(null);
  const [clearConfirm, setClearConfirm] = useState(false);
  const [clearing, setClearing] = useState(false);
  // "View tools" dialog — shows the MCP tools decision support exposes
  // (decision_decide), same pattern as the RAG page's tools dialog.
  const [showTools, setShowTools] = useState(false);
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

  // Load one history page by offset (page is 1-based). Fetches PAGE_SIZE+1
  // rows so an extra row can drive the next-page affordance.
  const loadPage = useCallback(async (p: number) => {
    const list = await listDecisionHistory(PAGE_SIZE + 1, (p - 1) * PAGE_SIZE);
    if (!mounted.current) return;
    const { rows, hasMore } = pageSlice(list, PAGE_SIZE);
    setEntries(rows);
    setHasMore(hasMore);
    setPage(p);
    setLoadError(false);
  }, []);

  const fetchAll = useCallback(async () => {
    setLoading(true);
    const gen = ++statusGenRef.current;
    try {
      const [st, list] = await Promise.all([
        decisionStatus(),
        listDecisionHistory(PAGE_SIZE + 1, 0),
      ]);
      if (!mounted.current) return;
      // Discard a stale status: a toggle that started after this fetch (or
      // completed while it was in flight) owns the enabled state now.
      if (gen === statusGenRef.current && !togglingRef.current) {
        setEnabled(st.enabled);
      }
      const { rows, hasMore } = pageSlice(list, PAGE_SIZE);
      setEntries(rows);
      setHasMore(hasMore);
      setPage(1);
      setLoadError(false);
    } catch (e) {
      console.warn('[decision] failed to load page data', e);
      if (mounted.current) setLoadError(true);
    } finally {
      if (mounted.current) setLoading(false);
    }
  }, []);

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
        setPage(1);
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
      {/* Header: title + beta tag + switch + status + view-tools — mirrors
          the RAG page header (interactive controls sit beside the title).
          The tools button stays enabled while decision support is off: the
          dialog documents the tool regardless of the enable flag. */}
      <div className="flex items-center gap-2.5 flex-wrap mb-1">
        <h1 className="text-xl font-semibold text-[var(--hub-ink)] flex items-center gap-2">
          <BrainCircuit size={20} className="text-[var(--hub-ink-2)]" />
          {t('pages.decision.title')}
          <span className="ml-1 inline-flex flex-shrink-0 items-center rounded-full border border-amber-200 bg-amber-50 px-1.5 py-0.5 text-[10px] font-medium leading-none text-amber-700 dark:border-amber-700/60 dark:bg-amber-900/20 dark:text-amber-300">
            {t('nav.beta')}
          </span>
        </h1>
        <div className="flex items-center gap-1.5">
          {toggling && <Loader2 size={14} className="animate-spin text-[var(--hub-ink-3)]" />}
          <Switch
            disabled={toggling}
            checked={enabled ?? false}
            onCheckedChange={handleToggle}
            aria-label={t('pages.decision.title')}
          />
          <span className="text-[12px] hub-mono" style={{ color: 'var(--hub-ink-3)' }}>
            {enabled === null ? '—' : enabled ? t('pages.decision.enabled') : t('pages.decision.disabled')}
          </span>
          <button
            type="button"
            onClick={() => setShowTools(true)}
            className="hub-btn sm"
            title={t('pages.decision.viewToolsHint')}
          >
            {t('pages.decision.viewTools')}
          </button>
        </div>
      </div>
      <p className="text-sm text-[var(--hub-ink-3)] mb-6">{t('pages.decision.subtitle')}</p>

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
          {(page > 1 || hasMore) && (
            <div className="flex items-center justify-center gap-3 py-3 border-t border-[var(--hub-line-2)]">
              <button
                type="button"
                className="hub-btn sm"
                disabled={page <= 1}
                aria-label={t('common.prevPage')}
                onClick={() => void loadPage(page - 1)}
              >
                <ChevronLeft size={12} /> {t('common.prevPage')}
              </button>
              <span className="text-xs text-[var(--hub-ink-3)]">
                {t('pages.decision.pageInfo', { page })}
              </span>
              <button
                type="button"
                className="hub-btn sm"
                disabled={!hasMore}
                aria-label={t('common.nextPage')}
                onClick={() => void loadPage(page + 1)}
              >
                {t('common.nextPage')} <ChevronRight size={12} />
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

      {/* View decision tools dialog — shows the MCP tools decision support
          exposes (decision_decide): name, description, input schema. */}
      {showTools && <ToolsDialog onClose={() => setShowTools(false)} enabled={enabled} />}
    </div>
  );
};

/** Dialog showing the MCP tools decision support exposes (decision_decide) —
 *  name, description, and input schema. Read-only; mirrors the RAG page's
 *  tools dialog. */
/** Exported for tests (DecisionToolsDialog.test.tsx). `enabled` mirrors the
 *  page switch: only a KNOWN-off flag (`false`) shows the "not exposed to
 *  MCP clients" hint — `null` (status not loaded yet) must not claim off.
 *  The catalog is always viewable per product req. */
export const ToolsDialog: React.FC<{ onClose: () => void; enabled?: boolean | null }> = ({
  onClose,
  enabled,
}) => {
  const { t } = useTranslation();
  const [tools, setTools] = useState<Record<string, unknown>[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');

  useEffect(() => {
    let alive = true;
    getDecisionTools()
      .then((ts) => {
        if (alive) setTools(ts);
      })
      .catch((e) => {
        if (alive) setError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => {
        if (alive) setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, []);

  return (
    <div className="fixed inset-0 bg-black/50 z-50 flex items-center justify-center p-4">
      <div className="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-2xl w-full mx-4 border border-gray-100 dark:border-gray-700 max-h-[90vh] flex flex-col">
        <div className="flex items-center justify-between p-5 border-b border-[var(--hub-line-2)]">
          <h2 className="text-xl font-bold text-gray-900 dark:text-gray-100">{t('pages.decision.viewTools')}</h2>
          <button onClick={onClose} className="hub-icon-btn sm">
            <X size={16} />
          </button>
        </div>
        <div className="flex-1 overflow-y-auto p-5 space-y-3">
          {enabled === false && (
            <p
              className="text-[12px] font-medium"
              style={{ color: 'var(--hub-warn, #d97706)' }}
            >
              {t('pages.decision.toolsDisabledHint')}
            </p>
          )}
          {loading ? (
            <div className="flex items-center gap-2 text-[13px]" style={{ color: 'var(--hub-ink-3)' }}>
              <Loader2 size={14} className="animate-spin" /> {t('common.loading')}
            </div>
          ) : error ? (
            <div className="text-[13px]" style={{ color: 'var(--hub-err)' }}>
              {error}
            </div>
          ) : tools.length === 0 ? (
            <div className="text-[13px]" style={{ color: 'var(--hub-ink-3)' }}>
              {t('pages.decision.toolsEmpty')}
            </div>
          ) : (
            tools.map((tool, idx) => {
              const name = String(tool.name ?? '');
              const desc = String(tool.description ?? '');
              const schema = tool.inputSchema as Record<string, unknown> | undefined;
              const props = (schema?.properties ?? {}) as Record<string, Record<string, unknown>>;
              const required = (schema?.required ?? []) as string[];
              return (
                <div key={`${name}-${idx}`} className="hub-card" style={{ padding: '12px 14px', background: 'var(--hub-surface)' }}>
                  <div className="flex items-center gap-2 mb-1.5">
                    <Sparkles size={14} style={{ color: 'var(--hub-accent)' }} />
                    <span className="hub-mono text-[13px] font-medium" style={{ color: 'var(--hub-ink)' }}>
                      {name}
                    </span>
                  </div>
                  <p className="text-[12.5px] mb-2" style={{ color: 'var(--hub-ink-2)' }}>
                    {desc}
                  </p>
                  {Object.keys(props).length > 0 && (
                    <div className="space-y-1">
                      {Object.entries(props).map(([k, v]) => (
                        <div key={k} className="flex items-center gap-2 text-[11.5px]">
                          <span className="hub-mono" style={{ color: 'var(--hub-ink)' }}>
                            {k}
                            {required.includes(k) && <span style={{ color: 'var(--hub-err)' }}>*</span>}
                          </span>
                          <span style={{ color: 'var(--hub-ink-3)' }}>
                            ({String(v.type ?? 'any')}) {String(v.description ?? '')}
                          </span>
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              );
            })
          )}
        </div>
        <div className="flex items-center justify-end gap-2 p-5 border-t border-[var(--hub-line-2)]">
          <button onClick={onClose} className="hub-btn">
            {t('common.close')}
          </button>
        </div>
      </div>
    </div>
  );
};

export default isTauri() ? DecisionPage : () => null;
