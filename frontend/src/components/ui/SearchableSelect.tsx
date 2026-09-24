import React, { useCallback, useEffect, useRef, useState } from 'react';

export interface SearchableSelectProps {
  /** 候选加载器：返回当前页 options + total。search 为防抖后的输入值 */
  loadOptions: (search: string, page: number, pageSize: number) => Promise<{ options: string[]; total: number }>;
  /** 当前选中值（'' = 未选） */
  value: string;
  onChange: (value: string) => void;
  /** 空值占位文案 */
  placeholder?: string;
  /** aria-label */
  ariaLabel?: string;
  /** 每页条数（默认 50） */
  pageSize?: number;
  /** 无匹配文案 */
  emptyText?: string;
  className?: string;
  /** 是否显示搜索输入框（默认 true；false 时为纯下拉，打开即加载全部候选） */
  searchable?: boolean;
}

/**
 * 可搜索的分页单选下拉框（§8 活动日志筛选）。
 * - 输入防抖 300ms 后重新拉第一页
 * - 滚动到底自动加载下一页（竞态守卫：以请求序号为准，过期响应丢弃）
 * - 选中后展示选中值，可一键清除
 * - 点击外部关闭
 */
const SearchableSelect: React.FC<SearchableSelectProps> = ({
  loadOptions,
  value,
  onChange,
  placeholder,
  ariaLabel,
  pageSize = 50,
  emptyText,
  className = '',
  searchable = true,
}) => {
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState('');
  const [options, setOptions] = useState<string[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(false);
  const [highlighted, setHighlighted] = useState(-1);

  const rootRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const reqSeqRef = useRef(0);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const fetchPage = useCallback(
    async (searchText: string, pageNum: number) => {
      const seq = ++reqSeqRef.current;
      setLoading(true);
      try {
        const res = await loadOptions(searchText, pageNum, pageSize);
        if (seq !== reqSeqRef.current) return; // 竞态守卫：过期响应丢弃
        setOptions((prev) => (pageNum > 1 ? [...prev, ...res.options] : res.options));
        setTotal(res.total);
      } catch (err) {
        if (seq === reqSeqRef.current) {
          console.error('[SearchableSelect] loadOptions failed:', err);
          if (pageNum === 1) setOptions([]);
        }
      } finally {
        if (seq === reqSeqRef.current) setLoading(false);
      }
    },
    [loadOptions, pageSize],
  );

  // 打开时拉第一页；输入防抖 300ms 后重拉第一页
  useEffect(() => {
    if (!open) return;
    if (debounceRef.current) clearTimeout(debounceRef.current);
    if (search === '') {
      fetchPage('', 1);
    } else {
      debounceRef.current = setTimeout(() => fetchPage(search, 1), 300);
    }
    return () => {
      if (debounceRef.current) clearTimeout(debounceRef.current);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, search]);

  // 点击外部关闭
  useEffect(() => {
    if (!open) return;
    const onDocClick = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener('mousedown', onDocClick);
    return () => document.removeEventListener('mousedown', onDocClick);
  }, [open]);

  // 打开时重置搜索态
  useEffect(() => {
    if (open) {
      setSearch('');
      setHighlighted(-1);
    }
  }, [open]);

  const hasMore = options.length < total;
  const loadingMore = loading && options.length > 0;

  // 滚动到底自动加载下一页
  const onScroll = useCallback(() => {
    const el = listRef.current;
    if (!el || loading || !hasMore) return;
    if (el.scrollHeight - el.scrollTop - el.clientHeight < 40) {
      fetchPage(search, Math.ceil(options.length / pageSize) + 1);
    }
  }, [loading, hasMore, fetchPage, search, options.length, pageSize]);

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (!open) {
      if (e.key === 'Enter' || e.key === 'ArrowDown') {
        e.preventDefault();
        setOpen(true);
      }
      return;
    }
    if (e.key === 'Escape') {
      e.preventDefault();
      setOpen(false);
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      setHighlighted((h) => Math.min(h + 1, options.length - 1));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setHighlighted((h) => Math.max(h - 1, 0));
    } else if (e.key === 'Enter') {
      e.preventDefault();
      if (highlighted >= 0 && highlighted < options.length) {
        onChange(options[highlighted]);
        setOpen(false);
      }
    }
  };

  return (
    <div ref={rootRef} className={`relative ${className}`}>
      <button
        type="button"
        role="combobox"
        aria-expanded={open}
        aria-label={ariaLabel}
        onClick={() => setOpen((o) => !o)}
        onKeyDown={handleKeyDown}
        className="hub-input flex items-center justify-between w-full text-left"
      >
        <span className={`truncate ${value ? '' : 'text-gray-400 dark:text-gray-500'}`}>
          {value || placeholder}
        </span>
        <span className="flex items-center gap-1 shrink-0 ml-2">
          {value && (
            <span
              role="button"
              aria-label="clear"
              tabIndex={0}
              onClick={(e) => {
                e.stopPropagation();
                onChange('');
              }}
              onKeyDown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.stopPropagation();
                  onChange('');
                }
              }}
              className="text-gray-400 hover:text-gray-600 dark:hover:text-gray-300"
            >
              <svg xmlns="http://www.w3.org/2000/svg" className="h-3.5 w-3.5" viewBox="0 0 20 20" fill="currentColor">
                <path
                  fillRule="evenodd"
                  d="M4.293 4.293a1 1 0 011.414 0L10 8.586l4.293-4.293a1 1 0 111.414 1.414L11.414 10l4.293 4.293a1 1 0 01-1.414 1.414L10 11.414l-4.293 4.293a1 1 0 01-1.414-1.414L8.586 10 4.293 5.707a1 1 0 010-1.414z"
                  clipRule="evenodd"
                />
              </svg>
            </span>
          )}
          <svg
            className={`h-4 w-4 text-gray-400 transition-transform ${open ? 'rotate-180' : ''}`}
            viewBox="0 0 20 20"
            fill="currentColor"
          >
            <path
              fillRule="evenodd"
              d="M5.293 7.293a1 1 0 011.414 0L10 10.586l3.293-3.293a1 1 0 111.414 1.414l-4 4a1 1 0 01-1.414 0l-4-4a1 1 0 010-1.414z"
              clipRule="evenodd"
            />
          </svg>
        </span>
      </button>

      {open && (
        <div className="absolute z-30 mt-1 w-full rounded-md border border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-800 shadow-lg">
          {searchable && (
            <div className="p-2 border-b border-gray-100 dark:border-gray-700">
              <input
                autoFocus
                type="text"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                onKeyDown={handleKeyDown}
                placeholder={placeholder}
                className="w-full text-sm rounded border border-gray-200 dark:border-gray-600 bg-transparent px-2 py-1.5 focus:outline-none focus:ring-1 focus:ring-blue-500"
              />
            </div>
          )}
          <div
            ref={listRef}
            onScroll={onScroll}
            style={{ maxHeight: 240, overflowY: 'auto', overscrollBehavior: 'contain' }}
            role="listbox"
          >
            {options.length === 0 && !loading && (
              <div className="px-3 py-4 text-sm text-gray-400 text-center">{emptyText ?? '—'}</div>
            )}
            {options.map((opt, idx) => (
              <div
                key={opt}
                role="option"
                aria-selected={opt === value}
                onMouseEnter={() => setHighlighted(idx)}
                onClick={() => {
                  onChange(opt);
                  setOpen(false);
                }}
                className={`px-3 py-1.5 text-sm cursor-pointer truncate ${
                  opt === value
                    ? 'bg-blue-50 dark:bg-blue-900/40 text-blue-700 dark:text-blue-300 font-medium'
                    : idx === highlighted
                      ? 'bg-gray-100 dark:bg-gray-700'
                      : ''
                }`}
              >
                {opt}
              </div>
            ))}
            {loadingMore && (
              <div className="px-3 py-2 text-xs text-gray-400 text-center">…</div>
            )}
          </div>
        </div>
      )}
    </div>
  );
};

export default SearchableSelect;
