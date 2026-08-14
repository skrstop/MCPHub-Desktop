import React, { useState, useRef, useEffect } from 'react';
import { Copy, Check, FileJson, Link as LinkIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useToast } from '@/contexts/ToastContext';
import { cn } from '@/utils/cn';

export const copyText = async (value: string): Promise<boolean> => {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(value);
      return true;
    }
  } catch {
    // fall through to fallback
  }
  try {
    const el = document.createElement('textarea');
    el.value = value;
    el.style.position = 'fixed';
    el.style.left = '-9999px';
    document.body.appendChild(el);
    el.focus();
    el.select();
    const ok = document.execCommand('copy');
    document.body.removeChild(el);
    return ok;
  } catch {
    return false;
  }
};

interface EndpointCopyProps {
  url: string;
  label?: string;
  prefix?: string;
  className?: string;
  /** Optionally override the value placed on the clipboard. */
  copyValue?: string;
  ariaLabel?: string;
  /** When provided, renders a second button (beside the URL copy) that copies a
   *  complete MCP client config (mcpServers JSON) pointing at this endpoint —
   *  ready to paste into Claude Desktop / Cursor. */
  configValue?: () => string;
  /** Toast shown after the config copy (defaults to the generic copy success). */
  configCopiedMessage?: string;
}

export const EndpointCopy: React.FC<EndpointCopyProps> = ({
  url,
  label,
  prefix,
  className,
  copyValue,
  ariaLabel,
  configValue,
  configCopiedMessage,
}) => {
  const { t } = useTranslation();
  const { showToast } = useToast();
  const [copied, setCopied] = useState(false);
  const [showDropdown, setShowDropdown] = useState(false);
  const dropdownRef = useRef<HTMLDivElement>(null);

  // Close the dropdown on outside click (mirrors GroupCard's copy dropdown).
  useEffect(() => {
    const handle = (e: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setShowDropdown(false);
      }
    };
    document.addEventListener('mousedown', handle);
    return () => document.removeEventListener('mousedown', handle);
  }, []);

  const doCopy = async (text: string, message?: string) => {
    const ok = await copyText(text);
    if (ok) {
      setCopied(true);
      setShowDropdown(false);
      showToast(message || t('common.copySuccess') || 'Copied to clipboard', 'success');
      setTimeout(() => setCopied(false), 1200);
    } else {
      showToast(t('common.copyFailed') || 'Copy failed', 'error');
    }
  };

  const hasConfig = !!configValue;

  // Single button click: if there's no config option, copy the URL directly
  // (no dropdown needed); otherwise open the dropdown (URL + config options).
  const onButtonClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    if (hasConfig) {
      setShowDropdown((v) => !v);
    } else {
      void doCopy(copyValue ?? url);
    }
  };

  return (
    <div className={cn('hub-endpoint', className)} role="group" aria-label={ariaLabel || url}>
      {label && <div className="hub-endpoint-label">{label}</div>}
      <div className="hub-endpoint-url" title={url}>
        {prefix && <span style={{ color: 'var(--hub-ink-3)' }}>{prefix}</span>}
        {url}
      </div>
      <div className="flex items-stretch relative h-full" ref={dropdownRef}>
        <button
          type="button"
          onClick={onButtonClick}
          className={cn('hub-endpoint-copy', copied ? 'copied' : '')}
          title={t('common.copy') || 'Copy'}
          aria-label={t('common.copy') || 'Copy'}
        >
          {copied ? <Check size={13} /> : <Copy size={13} />}
        </button>
        {showDropdown && hasConfig && (
          <div
            className="absolute top-full right-0 mt-1 z-[40] hub-card"
            style={{ minWidth: 160, padding: 4 }}
          >
            <button
              onClick={(e) => {
                e.stopPropagation();
                void doCopy(copyValue ?? url);
              }}
              className="flex items-center gap-2 w-full px-2.5 py-1.5 text-[13px] rounded-md hover:bg-[var(--hub-surface-hover)] text-left"
            >
              <LinkIcon size={12} /> {t('common.copyUrl') || 'Copy URL'}
            </button>
            <button
              onClick={(e) => {
                e.stopPropagation();
                const json = configValue?.();
                if (json) void doCopy(json, configCopiedMessage);
              }}
              className="flex items-center gap-2 w-full px-2.5 py-1.5 text-[13px] rounded-md hover:bg-[var(--hub-surface-hover)] text-left"
            >
              <FileJson size={12} /> {t('pages.dashboard.copyMcpConfig') || 'Copy MCP Config'}
            </button>
          </div>
        )}
      </div>
    </div>
  );
};

interface MonoCopyProps {
  text: string;
  className?: string;
  copyValue?: string;
  title?: string;
}

/** Inline monospace value with hover-to-copy icon. */
export const MonoCopy: React.FC<MonoCopyProps> = ({ text, className, copyValue, title }) => {
  const { t } = useTranslation();
  const { showToast } = useToast();
  const [copied, setCopied] = useState(false);

  const onCopy = async (e: React.MouseEvent) => {
    e.stopPropagation();
    const ok = await copyText(copyValue ?? text);
    if (!ok) {
      showToast(t('common.copyFailed') || 'Copy failed', 'error');
      return;
    }
    setCopied(true);
    showToast(t('common.copySuccess') || 'Copied to clipboard', 'success');
    setTimeout(() => setCopied(false), 1200);
  };

  return (
    <span
      className={cn(
        'hub-mono inline-flex items-center gap-1.5 group cursor-pointer text-[12.5px]',
        className,
      )}
      onClick={onCopy}
      title={title || text}
      role="button"
    >
      <span className="truncate">{text}</span>
      {copied ? (
        <Check size={12} className="text-[var(--hub-ok)] flex-shrink-0" />
      ) : (
        <Copy
          size={12}
          className="text-[var(--hub-ink-3)] opacity-0 group-hover:opacity-100 transition-opacity flex-shrink-0"
        />
      )}
    </span>
  );
};
