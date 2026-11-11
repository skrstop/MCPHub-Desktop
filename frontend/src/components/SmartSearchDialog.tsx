import React, { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { X, Loader2, Sparkles, Wrench, AlertCircle } from 'lucide-react';
import { searchSmartRouting, fetchSmartRoutingPerformance, type SmartSearchResult } from '@/services/smartRoutingService';
import {
  localizeSmartSearchError,
  scopeForRequest,
  type SmartSearchScopeOption,
} from '@/utils/smartSearchScope';

/** Result cap sent with each search — the backend clamps it to the user's
 *  max_results setting (passed in as a prop), so the effective limit is the
 *  smaller of the two. When exactly that many rows come back the list was
 *  likely truncated, so we say so (see resultsTruncated hint). */
const SEARCH_LIMIT = 20;

/** Relevance search over the Smart Routing tool index: a scope selector
 *  (all / group / server) + query input; results carry a weighted score. */
const SmartSearchDialog: React.FC<{
  onClose: () => void;
  scopeOptions: SmartSearchScopeOption[];
  /** User's smart-routing max_results setting — the backend clamps the
   *  request limit to it, so the truncation hint must use the same value. */
  maxResults?: number;
}> = ({ onClose, scopeOptions, maxResults }) => {
  const { t } = useTranslation();
  const [scope, setScope] = useState('all');
  const [query, setQuery] = useState('');
  const [results, setResults] = useState<SmartSearchResult[]>([]);
  const [searching, setSearching] = useState(false);
  const [hasSearched, setHasSearched] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // True when a search returned 0 rows AND the vector store itself is empty
  // (index never built / wiped by a model swap) — distinct from a genuine
  // "no matches" and worth its own hint.
  const [indexEmpty, setIndexEmpty] = useState(false);
  // Latest-scope token: a response that arrives after the user changed the
  // scope is discarded (stale-scope results must never render).
  const scopeRef = useRef(scope);
  scopeRef.current = scope;
  // Restore focus to the trigger on close (CopyClientConfigDialog convention:
  // move focus in on open, hand it back on close so keyboard users are not
  // left behind on the page underneath). The previously-focused element is
  // captured BEFORE we focus the input ourselves — autoFocus would run first
  // and make the capture a no-op.
  const inputRef = useRef<HTMLInputElement | null>(null);
  const returnFocusRef = useRef<HTMLElement | null>(null);
  useEffect(() => {
    returnFocusRef.current = document.activeElement as HTMLElement | null;
    inputRef.current?.focus();
    return () => returnFocusRef.current?.focus();
  }, []);

  // Changing the scope invalidates the displayed results: they were produced
  // under the previous selection and would mislead under the new one.
  const handleScopeChange = (value: string) => {
    setScope(value);
    setResults([]);
    setHasSearched(false);
    setError(null);
    setIndexEmpty(false);
  };

  // Known backend error sentinels → localized messages (mapping lives in
  // utils/smartSearchScope.ts, unit tested + contract-pinned); anything
  // else (unexpected upstream text) falls back to the raw string.
  const localizeError = (message: string): string =>
    localizeSmartSearchError(message, t);

  // Effective result cap: the backend clamps the request limit to the user's
  // max_results setting, so the truncation hint must use the same value.
  const effectiveLimit = Math.max(1, Math.min(SEARCH_LIMIT, maxResults ?? SEARCH_LIMIT));

  const handleSearch = async () => {
    const q = query.trim();
    if (!q || searching) return;
    const requestedScope = scope;
    setSearching(true);
    setError(null);
    setHasSearched(true);
    try {
      // The scope value is sent verbatim (`group:{id}` / `server:{name}` /
      // `all`) — the backend parses the prefix explicitly.
      const rows = await searchSmartRouting(
        q,
        scopeForRequest(requestedScope),
        effectiveLimit,
      );
      // Discard the response if the scope changed while the request was in
      // flight — stale-scope results must not render under the new selection.
      if (scopeRef.current !== requestedScope) return;
      setResults(rows);
      if (rows.length === 0) {
        // Distinguish "index is empty" from "query matched nothing": if the
        // vector store has no rows at all, point the user at the reindex.
        setIndexEmpty(false);
        try {
          const perf = await fetchSmartRoutingPerformance();
          // Search only ever yields tool rows — server rows alone don't make
          // the index useful, so the hint keys on toolRows.
          if (scopeRef.current === requestedScope && perf.vectorStore.toolRows === 0) {
            setIndexEmpty(true);
          }
        } catch {
          // Status probe is best-effort; keep the generic empty state.
        }
      } else {
        setIndexEmpty(false);
      }
    } catch (e) {
      if (scopeRef.current !== requestedScope) return;
      setResults([]);
      setIndexEmpty(false);
      setError(localizeError(e instanceof Error ? e.message : String(e)));
    } finally {
      setSearching(false);
    }
  };

  // Escape closes the dialog (document-level listener per the SettingsPage
  // dialog precedent — the input's own keydown only handles Enter).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [onClose]);

  // If the selected scope dangles (its group/server was deleted or renamed
  // while the dialog was open, so no option matches anymore), fall back to
  // the global scope instead of sending a value the backend cannot resolve.
  useEffect(() => {
    if (scope !== 'all' && !scopeOptions.some((o) => o.value === scope)) {
      handleScopeChange('all');
    }
  }, [scopeOptions, scope]);

  return (
    <div className="fixed inset-0 bg-black/50 z-50 flex items-center justify-center p-4">
      <div
        role="dialog"
        aria-modal="true"
        aria-label={t('server.smartSearch.title')}
        className="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-2xl w-full mx-4 border border-gray-100 dark:border-gray-700 max-h-[90vh] flex flex-col"
      >
        <div className="flex items-center justify-between p-5 border-b border-[var(--hub-line-2)]">
          <h2 className="text-xl font-bold text-gray-900 dark:text-gray-100">
            {t('server.smartSearch.title')}
          </h2>
          <button onClick={onClose} className="hub-icon-btn sm" aria-label={t('common.close')}>
            <X size={16} />
          </button>
        </div>
        <div className="flex-1 overflow-y-auto p-5 space-y-4">
          <p className="text-[12px]" style={{ color: 'var(--hub-ink-3)' }}>
            {t('server.smartSearch.hint')}
          </p>
          <div className="flex gap-2">
            <select
              className="hub-input"
              style={{ width: 220, flexShrink: 0 }}
              value={scope}
              onChange={(e) => handleScopeChange(e.target.value)}
              aria-label={t('server.smartSearch.scope')}
            >
              {scopeOptions.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </select>
            <input
              className="hub-input flex-1"
              ref={inputRef}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') void handleSearch();
              }}
              placeholder={t('server.smartSearch.queryPlaceholder')}
            />
            <button
              onClick={handleSearch}
              className="hub-btn primary"
              disabled={searching || !query.trim()}
            >
              {searching ? (
                <Loader2 size={13} className="animate-spin" />
              ) : (
                <Sparkles size={13} />
              )}{' '}
              {t('server.smartSearch.searchBtn')}
            </button>
          </div>
          {error && (
            <div
              className="error-box flex items-center gap-2 p-3 text-[13px]"
            >
              <AlertCircle size={14} className="flex-shrink-0" />
              <span>{error}</span>
            </div>
          )}
          {hasSearched && !searching && !error && (
            <div className="space-y-2">
              <div className="hub-sect" style={{ color: 'var(--hub-ink-3)', fontSize: 11 }}>
                {t('server.smartSearch.results')} ({results.length})
                {results.length >= effectiveLimit && (
                  <span className="ml-2">
                    {t('server.smartSearch.resultsTruncated', { count: effectiveLimit })}
                  </span>
                )}
              </div>
              {results.length === 0 ? (
                <div
                  className="hub-card p-6 text-center text-[13px]"
                  style={{ color: 'var(--hub-ink-3)' }}
                >
                  {indexEmpty
                    ? t('server.smartSearch.indexEmpty')
                    : t('server.smartSearch.noResults')}
                </div>
              ) : (
                results.map((r, idx) => (
                  <div
                    key={`${r.contentId}-${idx}`}
                    className="hub-card"
                    style={{ padding: '10px 14px', background: 'var(--hub-surface)' }}
                  >
                    <div className="flex items-center justify-between gap-2">
                      <div className="flex items-center gap-2 min-w-0">
                        <Wrench size={13} style={{ color: 'var(--hub-ink-3)', flexShrink: 0 }} />
                        <span
                          className="truncate text-[13px] font-medium"
                          style={{ color: 'var(--hub-ink)' }}
                          title={r.toolName}
                        >
                          {r.toolName}
                        </span>
                        <span
                          className="hub-mono truncate"
                          style={{ fontSize: 11, color: 'var(--hub-ink-3)' }}
                          title={r.serverName}
                        >
                          {r.serverName}
                        </span>
                      </div>
                      <span
                        className="hub-tag accent hub-mono whitespace-nowrap flex-shrink-0"
                        style={{ fontSize: 11 }}
                        title={t('server.smartSearch.scoreHint')}
                      >
                        {t('server.smartSearch.score')}: {r.score.toFixed(2)}
                      </span>
                    </div>
                    {(r.metadata?.description || r.textContent) && (
                      <p
                        className="text-[12px] mt-1.5"
                        style={{
                          color: 'var(--hub-ink-3)',
                          display: '-webkit-box',
                          WebkitLineClamp: 2,
                          WebkitBoxOrient: 'vertical',
                          overflow: 'hidden',
                        }}
                      >
                        {r.metadata?.description || r.textContent}
                      </p>
                    )}
                  </div>
                ))
              )}
            </div>
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

export default SmartSearchDialog;
