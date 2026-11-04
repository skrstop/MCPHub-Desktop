import React, { useState, useRef, useEffect, useLayoutEffect } from 'react';
import { createPortal } from 'react-dom';

interface SimpleSelectProps {
  value: string;
  onChange: (value: string) => void;
  options: { value: string; label: string }[];
  disabled?: boolean;
  ariaLabel?: string;
  className?: string;
}

/**
 * 单选下拉框（自定义浮层）。原生 `<select>` 的弹出列表由操作系统渲染，
 * 无法用 CSS 定制（暗色模式下出现系统蓝高亮/双箭头，与 hub 设计语言
 * 不符）——本组件复用活动查询 SearchableSelect 的触发框与弹层样式，
 * 并像 MultiSelect 一样 portal 到 body（避免 overflow 容器裁剪）。
 */
export const SimpleSelect: React.FC<SimpleSelectProps> = ({
  value,
  onChange,
  options,
  disabled = false,
  ariaLabel,
  className = '',
}) => {
  const [isOpen, setIsOpen] = useState(false);
  const [highlighted, setHighlighted] = useState(-1);
  const [menuRect, setMenuRect] = useState<{ top: number; left: number; width: number } | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  const syncMenuRect = () => {
    const el = rootRef.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const h = menuRef.current?.offsetHeight ?? 0;
    const below = window.innerHeight - r.bottom;
    // Flip when the menu doesn't fit below but fits better above.
    const up = below < Math.min(h + 4, 240) && r.top > below;
    let top = up ? r.top - h - 4 : r.bottom + 4;
    // Clamp inside the viewport so a tall menu never spills off-screen.
    top = Math.max(8, Math.min(top, window.innerHeight - h - 8));
    setMenuRect({ top, left: r.left, width: r.width });
  };

  useEffect(() => {
    if (!isOpen) return;
    const onDocClick = (e: MouseEvent) => {
      const t = e.target as Node;
      if (
        rootRef.current &&
        !rootRef.current.contains(t) &&
        !(menuRef.current && menuRef.current.contains(t))
      ) {
        setIsOpen(false);
      }
    };
    document.addEventListener('mousedown', onDocClick);
    return () => document.removeEventListener('mousedown', onDocClick);
  }, [isOpen]);

  // Keep the keyboard-highlighted option visible in the scrolling menu.
  useEffect(() => {
    if (!isOpen || highlighted < 0) return;
    const menu = menuRef.current;
    const el = menu?.querySelectorAll('[role="option"]')[highlighted] as
      | HTMLElement
      | undefined;
    el?.scrollIntoView({ block: 'nearest' });
  }, [isOpen, highlighted, options]);

  useLayoutEffect(() => {
    if (!isOpen) return;
    syncMenuRect();
    const onReposition = () => syncMenuRect();
    window.addEventListener('scroll', onReposition, true);
    window.addEventListener('resize', onReposition);
    const menu = menuRef.current;
    let ro: ResizeObserver | undefined;
    if (menu && typeof ResizeObserver !== 'undefined') {
      ro = new ResizeObserver(() => syncMenuRect());
      ro.observe(menu);
    }
    return () => {
      window.removeEventListener('scroll', onReposition, true);
      window.removeEventListener('resize', onReposition);
      ro?.disconnect();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isOpen]);

  const selectedLabel = options.find((o) => o.value === value)?.label ?? value;

  return (
    <div ref={rootRef} className={`relative ${className}`}>
      <button
        type="button"
        role="combobox"
        aria-expanded={isOpen}
        aria-label={ariaLabel}
        disabled={disabled}
        onClick={() => {
          if (disabled) return;
          if (!isOpen) syncMenuRect();
          // Reopening highlights the current value (combobox convention);
          // a stale highlight from a previous session must not survive.
          setHighlighted(options.findIndex((o) => o.value === value));
          setIsOpen(!isOpen);
        }}
        onKeyDown={(e) => {
          if (disabled) return;
          if (!isOpen) {
            if (e.key === 'Enter' || e.key === 'ArrowDown') {
              e.preventDefault();
              syncMenuRect();
              setHighlighted(options.findIndex((o) => o.value === value));
              setIsOpen(true);
            }
            return;
          }
          if (e.key === 'Escape') {
            e.preventDefault();
            setIsOpen(false);
          } else if (e.key === 'ArrowDown') {
            e.preventDefault();
            setHighlighted((h) => Math.min(h + 1, options.length - 1));
          } else if (e.key === 'ArrowUp') {
            e.preventDefault();
            setHighlighted((h) => Math.max(h - 1, 0));
          } else if (e.key === 'Enter') {
            e.preventDefault();
            // No highlight yet: keep the current value (just close) instead of
            // silently selecting the first option.
            const opt =
              highlighted >= 0
                ? options[highlighted]
                : options.find((o) => o.value === value);
            if (opt) {
              onChange(opt.value);
            }
            setIsOpen(false);
          }
        }}
        className={`hub-input flex items-center justify-between w-full text-left ${
          disabled ? 'opacity-50 cursor-not-allowed' : ''
        }`}
      >
        <span className="truncate">{selectedLabel}</span>
        <svg
          className={`h-4 w-4 text-gray-400 shrink-0 ml-2 transition-transform ${isOpen ? 'rotate-180' : ''}`}
          viewBox="0 0 20 20"
          fill="currentColor"
        >
          <path
            fillRule="evenodd"
            d="M5.293 7.293a1 1 0 011.414 0L10 10.586l3.293-3.293a1 1 0 111.414 1.414l-4 4a1 1 0 01-1.414 0l-4-4a1 1 0 010-1.414z"
            clipRule="evenodd"
          />
        </svg>
      </button>

      {isOpen && !disabled && menuRect && createPortal(
        <div
          ref={menuRef}
          style={{
            position: 'fixed',
            top: menuRect.top,
            left: menuRect.left,
            width: menuRect.width,
            zIndex: 1000,
          }}
          className="rounded-md border border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-800 shadow-lg overflow-hidden max-h-72 overflow-y-auto"
          role="listbox"
          onKeyDown={(e) => {
            if (e.key === 'Escape') {
              e.preventDefault();
              setIsOpen(false);
            }
          }}
        >
          {options.map((option, idx) => {
            const isSelected = option.value === value;
            return (
              <div
                key={option.value}
                role="option"
                aria-selected={isSelected}
                onMouseEnter={() => setHighlighted(idx)}
                onClick={() => {
                  if (disabled) return;
                  onChange(option.value);
                  setIsOpen(false);
                }}
                className={`px-3 py-1.5 text-sm cursor-pointer truncate ${
                  isSelected
                    ? 'bg-blue-50 dark:bg-blue-900/40 text-blue-700 dark:text-blue-300 font-medium'
                    : idx === highlighted
                      ? 'bg-gray-100 dark:bg-gray-700 text-gray-900 dark:text-gray-100'
                      : 'hover:bg-gray-100 dark:hover:bg-gray-700 text-gray-900 dark:text-gray-100'
                }`}
              >
                {option.label}
              </div>
            );
          })}
        </div>,
        document.body
      )}
    </div>
  );
};
