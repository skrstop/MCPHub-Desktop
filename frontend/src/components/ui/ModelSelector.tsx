import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Check, ChevronDown, Download, Loader2 } from 'lucide-react';
import type { RagModelInfo } from '@/types';

/** Format a byte count as B/KB/MB (mirrors the RagPage helper). */
const formatSize = (bytes: number): string => {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
};

/** Format a duration in seconds as "Mm Ss" (>60s) or "Ss" (compact ETA). */
const formatEta = (secs: number): string => {
  if (secs <= 0) return '--';
  if (secs < 60) return `${secs}s`;
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return `${m}m ${s.toString().padStart(2, '0')}s`;
};

export interface ModelDownloadProgress {
  size: string;
  phase: string;
  downloaded: number;
  total: number;
  percent: number;
  speed: number;
  eta: number;
  fileCurrent: number;
  fileTotal: number;
  message?: string;
}

/** Local embedding model selector dropdown (used by the RAG page toolbar and
 *  the Settings "模型和向量" section). Lists every model size: ready sizes are
 *  selectable, downloadable ones show a Download button with a rich progress
 *  bar (percent / speed / ETA / file index) driven by `rag://model-download`
 *  events handled by the parent. */
const ModelSelector: React.FC<{
  models: RagModelInfo[];
  currentModel: string | null;
  modelDownload: ModelDownloadProgress | null;
  disabled: boolean;
  onSelect: (size: string) => void;
  onDownload: (size: string) => void;
  onRefresh: () => void;
}> = ({ models, currentModel, modelDownload, disabled, onSelect, onDownload, onRefresh }) => {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);

  // Close the panel on click-outside.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (wrapRef.current && !wrapRef.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open]);

  if (models.length === 0) {
    return (
      <button type="button" onClick={onRefresh} className="hub-icon-btn sm" title={t('pages.rag.refresh')}>
        <Loader2 size={12} />
      </button>
    );
  }

  const current = models.find((m) => m.size === currentModel);
  const triggerLabel = current ? current.label : t('pages.rag.modelSelect');

  return (
    <div className="relative min-w-[150px] shrink" ref={wrapRef}>
      {/* Trigger button: current model + chevron. Disabled only while the
          runtime is initializing/switching (the parent decides availability). */}
      <button
        type="button"
        disabled={disabled}
        onClick={() => setOpen((v) => !v)}
        className="hub-input flex items-center gap-1.5"
        style={{
          height: 28,
          fontSize: 12,
          padding: '0 6px',
          background: 'var(--hub-surface)',
          color: 'var(--hub-ink)',
          cursor: disabled ? 'not-allowed' : 'pointer',
          minWidth: 160,
        }}
        title={t('pages.rag.modelSelect')}
      >
        <span className="truncate flex-1 text-left">{triggerLabel}</span>
        <ChevronDown size={12} style={{ flexShrink: 0, opacity: 0.6 }} />
      </button>

      {open && (
        <div
          className="absolute z-50 mt-1 rounded-lg shadow-2xl border overflow-hidden"
          style={{
            background: 'var(--hub-surface)',
            borderColor: 'var(--hub-line-2)',
            minWidth: 'max(374px, 100%)',
            maxHeight: 400,
            overflowY: 'auto',
          }}
        >
          {models.map((m) => {
            const isCurrent = m.size === currentModel;
            const dl = modelDownload && modelDownload.size === m.size ? modelDownload : null;
            const downloading = !!dl && dl.phase === 'downloading';
            const fmtBadge =
              m.format === 'gguf' ? t('pages.rag.modelFormatGguf') : '';
            return (
              <div
                key={m.size}
                className="border-b last:border-b-0"
                style={{ borderColor: 'var(--hub-line)' }}
              >
                <div
                  className="flex items-center justify-between gap-2 px-2.5"
                  style={{ padding: '6px 10px' }}
                >
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-1.5">
                      <span
                        className="truncate text-[12.5px]"
                        style={{ color: 'var(--hub-ink)', fontWeight: isCurrent ? 600 : 400 }}
                      >
                        {m.label}
                      </span>
                      {fmtBadge && (
                        <span
                          className="hub-tag"
                          style={{ fontSize: 10, padding: '0 4px', flexShrink: 0 }}
                        >
                          {fmtBadge}
                        </span>
                      )}
                      {isCurrent && (
                        <Check size={12} style={{ color: 'var(--hub-accent)', flexShrink: 0 }} />
                      )}
                    </div>
                    <div
                      className="flex items-center gap-2 hub-mono"
                      style={{ fontSize: 10.5, color: 'var(--hub-ink-3)' }}
                    >
                      <span style={{ flexShrink: 0 }}>
                        {m.ready
                          ? m.fileSize
                            ? formatSize(m.fileSize)
                            : t('pages.rag.modelSizeUnknown')
                          : m.downloadable
                          ? t('pages.rag.modelDownloadable')
                          : t('pages.rag.modelSizeUnknown')}
                      </span>
                      {m.description && (
                        <span
                          className="truncate"
                          style={{ minWidth: 0, color: 'var(--hub-ink-3)' }}
                          title={m.description}
                        >
                          {m.description}
                        </span>
                      )}
                    </div>
                  </div>

                  {/* Right action: ready -> select (clickable row); downloadable
                      -> Download button. */}
                  {m.ready ? (
                    <button
                      type="button"
                      disabled={disabled || isCurrent}
                      onClick={() => {
                        if (!isCurrent) onSelect(m.size);
                        setOpen(false);
                      }}
                      className="hub-btn sm"
                      style={{ height: 24, fontSize: 11, opacity: isCurrent ? 0.5 : 1 }}
                      title={t('pages.rag.modelSelect')}
                    >
                      {isCurrent ? t('pages.rag.modelCurrent') : t('pages.rag.modelUse')}
                    </button>
                  ) : m.downloadable ? (
                    <button
                      type="button"
                      disabled={downloading}
                      onClick={() => onDownload(m.size)}
                      className="hub-btn sm"
                      style={{ height: 24, fontSize: 11 }}
                      title={t('pages.rag.modelDownloadHint', { name: m.label })}
                    >
                      {downloading ? <Loader2 size={11} className="animate-spin" /> : <Download size={11} />}
                      {t('pages.rag.modelDownload')}
                    </button>
                  ) : null}
                </div>

                {/* Rich progress bar under a downloading row: %, speed, ETA,
                    file index/total. */}
                {downloading && dl && (
                  <div style={{ padding: '0 10px 8px' }}>
                    <div
                      className="rounded-full overflow-hidden"
                      style={{ height: 6, background: 'var(--hub-line)' }}
                    >
                      <div
                        style={{
                          width: `${dl.percent}%`,
                          height: '100%',
                          background: 'var(--hub-accent)',
                          transition: 'width 0.2s',
                        }}
                      />
                    </div>
                    <div
                      className="flex items-center gap-2 mt-1 hub-mono"
                      style={{ fontSize: 10, color: 'var(--hub-ink-3)' }}
                    >
                      <span style={{ color: 'var(--hub-ink)' }}>{dl.percent}%</span>
                      {dl.speed > 0 && <span>{formatSize(dl.speed)}/s</span>}
                      {dl.eta > 0 && (
                        <span>
                          {formatEta(dl.eta)} {t('pages.rag.modelLeft')}
                        </span>
                      )}
                      {dl.fileTotal > 0 && (
                        <span>
                          {dl.fileCurrent}/{dl.fileTotal} {t('pages.rag.modelFiles')}
                        </span>
                      )}
                    </div>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
};

export default ModelSelector;
