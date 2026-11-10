import React, { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { X, Loader2, Sparkles, Wrench } from 'lucide-react';
import { searchSmartRouting, type SmartSearchResult } from '@/services/smartRoutingService';

export interface SmartSearchScopeOption {
  /** "all" | `group:{id}` | `server:{name}` */
  value: string;
  label: string;
  kind: 'all' | 'group' | 'server';
}

/** Relevance search over the Smart Routing tool index: a scope selector
 *  (all / group / server) + query input; results carry a weighted score. */
const SmartSearchDialog: React.FC<{
  onClose: () => void;
  scopeOptions: SmartSearchScopeOption[];
}> = ({ onClose, scopeOptions }) => {
  const { t } = useTranslation();
  const [scope, setScope] = useState('all');
  const [query, setQuery] = useState('');
  const [results, setResults] = useState<SmartSearchResult[]>([]);
  const [searching, setSearching] = useState(false);
  const [hasSearched, setHasSearched] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleSearch = async () => {
    const q = query.trim();
    if (!q || searching) return;
    setSearching(true);
    setError(null);
    setHasSearched(true);
    try {
      // `group:{id}` / `server:{name}` prefixes carry the selector kind; the
      // backend resolves group ids/names first, then falls back to server.
      const raw = scope === 'all' ? 'all' : scope.replace(/^(group|server):/, '');
      setResults(await searchSmartRouting(q, raw));
    } catch (e) {
      setResults([]);
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSearching(false);
    }
  };

  return (
    <div className="fixed inset-0 bg-black/50 z-50 flex items-center justify-center p-4">
      <div className="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-2xl w-full mx-4 border border-gray-100 dark:border-gray-700 max-h-[90vh] flex flex-col">
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
              onChange={(e) => setScope(e.target.value)}
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
            <div className="hub-card p-3 text-[13px]" style={{ color: 'oklch(0.4 0.18 25)' }}>
              {error}
            </div>
          )}
          {hasSearched && !searching && !error && (
            <div className="space-y-2">
              <div className="hub-sect" style={{ color: 'var(--hub-ink-3)', fontSize: 11 }}>
                {t('server.smartSearch.results')} ({results.length})
              </div>
              {results.length === 0 ? (
                <div
                  className="hub-card p-6 text-center text-[13px]"
                  style={{ color: 'var(--hub-ink-3)' }}
                >
                  {t('server.smartSearch.noResults')}
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
