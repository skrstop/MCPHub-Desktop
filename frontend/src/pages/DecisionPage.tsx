import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { BrainCircuit, Eye, Loader2, RefreshCw, RotateCcw, Sparkles, Trash2, X } from 'lucide-react';
import { Switch } from '@/components/ui/ToggleGroup';
import ConfirmDialog from '@/components/ui/ConfirmDialog';
import Pagination from '@/components/ui/Pagination';
import {
  clearDecisionHistory,
  countDecisionHistory,
  decideDecision,
  decisionSetEnabled,
  decisionStatus,
  getDecisionTools,
  listDecisionHistory,
  listDecisionModels,
  type DecisionHistoryEntry,
} from '@/services/decisionService';
import { useToast } from '@/contexts/ToastContext';
import { isTauri } from '@/utils/tauriClient';
import {
  decisionTypeLabelKey,
  formatProbability,
  formatScore,
} from '@/utils/decisionFormat';

const PAGE_SIZE = 10;

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
  // Total history rows (from the backend COUNT query) — drives the RAG-style
  // pagination footer (showing x–x of total + page-number buttons).
  const [total, setTotal] = useState(0);
  // Rows per page for the offset-paged history list.
  const [pageSize, setPageSize] = useState(PAGE_SIZE);
  // 1-based page number for the offset-paged history list.
  const [page, setPage] = useState(1);
  // Initial-load failure: distinct from "empty history" — the user must be
  // able to tell a backend/IPC error from a genuinely empty list, and retry.
  const [loadError, setLoadError] = useState(false);
  const [pageLoading, setPageLoading] = useState(false);
  const [viewing, setViewing] = useState<DecisionHistoryEntry | null>(null);
  // Replay: the entry id currently being re-run (button spinner), or null.
  const [replayingId, setReplayingId] = useState<string | null>(null);
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
  // History rows record the model selection key ("<family>/<size>", e.g.
  // "julia/1") — map it to the friendly dropdown label for display.
  const [modelLabels, setModelLabels] = useState<Record<string, string>>({});
  useEffect(() => {
    listDecisionModels()
      .then((models) => {
        if (mounted.current) {
          setModelLabels(Object.fromEntries(models.map((m) => [m.key ?? m.size, m.label])));
        }
      })
      .catch(() => {});
  }, []);
  const modelLabel = useCallback(
    (key?: string | null) => (key ? modelLabels[key] ?? key : key),
    [modelLabels],
  );

  // Load one history page by offset (page is 1-based), plus the total row
  // count for the pagination footer. `size` overrides the page size when the
  // per-page selector changes (always jumps back to page 1).
  const loadPage = useCallback(async (p: number, size?: number) => {
    const ps = size ?? pageSize;
    setPageLoading(true);
    try {
      const [list, count] = await Promise.all([
        listDecisionHistory(ps, (p - 1) * ps),
        countDecisionHistory(),
      ]);
      if (!mounted.current) return;
      setEntries(list);
      setTotal(count);
      setPage(p);
      if (size != null) setPageSize(size);
      setLoadError(false);
    } catch (e) {
      console.warn('[decision] failed to load history page', e);
      if (mounted.current) setLoadError(true);
    } finally {
      if (mounted.current) setPageLoading(false);
    }
  }, [pageSize]);

  const fetchAll = useCallback(async () => {
    setLoading(true);
    const gen = ++statusGenRef.current;
    try {
      const [st, list, count] = await Promise.all([
        decisionStatus(),
        listDecisionHistory(PAGE_SIZE, 0),
        countDecisionHistory(),
      ]);
      if (!mounted.current) return;
      // Discard a stale status: a toggle that started after this fetch (or
      // completed while it was in flight) owns the enabled state now.
      if (gen === statusGenRef.current && !togglingRef.current) {
        setEnabled(st.enabled);
      }
      setEntries(list);
      setTotal(count);
      setPage(1);
      setPageSize(PAGE_SIZE);
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
        setTotal(0);
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

  // Replay: re-run the original request with the CURRENTLY selected model
  // (do NOT switch back to the entry's recorded model — the user may have
  // deliberately chosen another model), then refresh the list so the new
  // entry shows at the top.
  const handleReplay = async (e: DecisionHistoryEntry) => {
    if (replayingId) return;
    setReplayingId(e.id);
    try {
      // DecisionOutput serializes camelCase: { decisionType, scores,
      // probabilities, index, option } — the selected-option probability is
      // probabilities[index] (NOT selectedIndex, which doesn't exist on the
      // output and made the toast always show "概率 —").
      const out = await decideDecision({
        decisionType: e.decisionType,
        state: e.state ?? '',
        question: e.question,
        options: e.options,
      }) as { index?: number; probabilities?: number[]; option?: string } | null;
      if (mounted.current) {
        await loadPage(1);
        const prob = out?.probabilities?.[out.index ?? -1];
        showToast(
          t('pages.decision.replayDone', {
            option: out?.option ?? e.selectedOption,
            prob: prob != null ? formatProbability(prob) : '—',
          }),
          'success',
        );
      }
    } catch (err) {
      showToast(
        err instanceof Error ? err.message : t('pages.decision.replayFailed'),
        'error',
      );
    } finally {
      if (mounted.current) setReplayingId(null);
    }
  };

  const typeLabel = (ty: string) => t(decisionTypeLabelKey(ty));

  // Friendly localized name for the recorded source ("ui" / "mcp");
  // unknown values fall through verbatim.
  const sourceLabel = (s: string) =>
    s === 'ui' ? t('pages.decision.sourceUi') : s === 'mcp' ? t('pages.decision.sourceMcp') : s;

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
      <div className="flex items-center justify-between gap-4 mb-1 flex-wrap">
        <div className="flex items-center gap-2.5 min-w-0 flex-wrap">
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
        {/* Top-right action buttons（RAG 页同款布局：justify-between 推到右上角，
            flex-wrap：窄窗口换行不压缩按钮文字） */}
        <div className="flex items-center gap-2 flex-wrap flex-shrink-0">
          <button
            type="button"
            className="hub-btn sm"
            disabled={loading || pageLoading}
            title={t('pages.decision.refresh')}
            onClick={() => void fetchAll()}
          >
            <RefreshCw size={12} className={loading || pageLoading ? 'animate-spin' : ''} />
            {t('pages.decision.refresh')}
          </button>
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
      </div>
      <p className="text-sm text-[var(--hub-ink-3)] mb-6">{t('pages.decision.subtitle')}</p>

      {/* History list */}
      <div className="hub-card">
        <div className="flex items-center justify-between py-3 px-5 border-b border-[var(--hub-line-2)]">
          <h2 className="font-medium text-[var(--hub-ink)]">{t('pages.decision.historyTitle')}</h2>
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
                    {e.model && <span>{modelLabel(e.model)}</span>}
                    {e.source && <span>{sourceLabel(e.source)}</span>}
                  </div>
                </div>
                <button
                  type="button"
                  className="hub-btn sm"
                  style={{ flexShrink: 0 }}
                  disabled={replayingId !== null}
                  title={t('pages.decision.replay')}
                  onClick={() => void handleReplay(e)}
                >
                  {replayingId === e.id ? (
                    <Loader2 size={12} className="animate-spin" />
                  ) : (
                    <RotateCcw size={12} />
                  )}
                  {replayingId === e.id
                    ? t('pages.decision.replaying')
                    : t('pages.decision.replay')}
                </button>
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
          {entries.length > 0 && (
            <div
              className="flex items-center flex-wrap gap-y-1 text-[12px]"
              style={{ color: 'var(--hub-ink-3)', borderTop: '1px solid var(--hub-line-2)', padding: '8px 16px' }}
            >
              <div className="flex-[2] flex items-center gap-2 min-w-[200px]">
                <span className="min-w-0">
                  {t('common.showing', {
                    start: total === 0 ? 0 : (page - 1) * pageSize + 1,
                    end: Math.min(page * pageSize, total),
                    total,
                  })}
                </span>
              </div>
              <div className="flex-[4] flex justify-center min-w-0">
                {Math.ceil(total / pageSize) > 1 && (
                  <Pagination
                    currentPage={page}
                    totalPages={Math.ceil(total / pageSize)}
                    onPageChange={(p) => void loadPage(p)}
                    disabled={pageLoading}
                  />
                )}
              </div>
              <div className="flex-[2] flex items-center justify-end gap-2 flex-shrink-0">
                <label htmlFor="decisionPerPage">{t('common.itemsPerPage')}:</label>
                <select
                  id="decisionPerPage"
                  value={pageSize}
                  onChange={(e) => void loadPage(1, Number(e.target.value))}
                  disabled={pageLoading}
                  className="hub-input"
                  style={{ height: 26, width: 70, padding: '0 6px', fontSize: 12 }}
                >
                  <option value={10}>10</option>
                  <option value={20}>20</option>
                  <option value={50}>50</option>
                  <option value={100}>100</option>
                </select>
              </div>
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
              {viewing.durationMs != null && (
                <div>
                  <div className="text-[var(--hub-ink-3)] mb-1">{t('pages.decision.fieldDuration')}</div>
                  <div className="text-[var(--hub-ink)]">{viewing.durationMs}ms</div>
                </div>
              )}
              {viewing.model && (
                <div>
                  <div className="text-[var(--hub-ink-3)] mb-1">{t('pages.decision.fieldModel')}</div>
                  <div className="text-[var(--hub-ink)] hub-mono text-[12px]">{modelLabel(viewing.model)}</div>
                </div>
              )}
              {viewing.source && (
                <div>
                  <div className="text-[var(--hub-ink-3)] mb-1">{t('pages.decision.fieldSource')}</div>
                  <div className="text-[var(--hub-ink)]">{sourceLabel(viewing.source)}</div>
                </div>
              )}
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
                        {typeof prob === 'number' && (
                          <span
                            className="text-[12px] font-medium"
                            style={{ color: selected ? 'var(--hub-accent)' : 'var(--hub-ink-2)' }}
                          >
                            {formatProbability(prob)}
                          </span>
                        )}
                        {typeof score === 'number' && (
                          <span
                            className="text-[11px] text-[var(--hub-ink-3)] cursor-help"
                            title={t('pages.decision.scoreHint')}
                          >
                            {t('pages.decision.score')}: {formatScore(score)}
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
            </div>
          </div>
        </div>
      )}

      {clearConfirm && (
        <ConfirmDialog
          isOpen
          title={t('pages.decision.clearConfirmTitle')}
          message={
            total > entries.length
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
