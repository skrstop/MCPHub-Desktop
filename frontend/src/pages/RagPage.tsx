import React, { useState, useMemo, useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import {
  Upload,
  SlidersHorizontal,
  Trash2,
  Eye,
  Layers,
  Info,
  X,
  Loader2,
  FileText,
  FolderOpen,
  Search,
  Sparkles,
  Tag,
  Download,
  Check,
  ChevronDown,
  Code,
  Plus,
  Minus,
  RefreshCw,
} from 'lucide-react';
import { Switch } from '@/components/ui/ToggleGroup';
import FileTypeRenderer from '@/components/ui/FileTypeRenderer';
import { useToast } from '@/contexts/ToastContext';
import { useRagData } from '@/hooks/useRagData';
import { getRagTools } from '@/services/ragService';
import { RagDoc, RagDocInfo, RagModelInfo, RagPickedFile, RagSettings } from '@/types';

const formatSize = (bytes: number): string => {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
};

const RagPage: React.FC = () => {
  const { t } = useTranslation();
  const { showToast } = useToast();
  const {
    ragDocs,
    enabled,
    initializing,
    togglingTo,
    switchingModel,
    settings,
    modelLimits,
    viewedDoc,
    viewLoading,
    searchResults,
    searching,
    uploading,
    uploadProgress,
    charProgress,
    reindexing,
    updatingDoc,
    reindexConfirm,
    confirmReindex,
    cancelReindex,
    models,
    currentModel,
    modelDownload,
    fetchModels,
    selectModel,
    downloadModel,
    toggleEnabled,
    upload,
    updateDoc,
    remove,
    removeMany,
    view,
    closeView,
    chunksDoc,
    chunksList,
    chunksLoading,
    viewChunks,
    closeChunks,
    openLocation,
    search,
    setTags,
    pickFiles,
    updateSettings,
  } = useRagData();

  const [showUpload, setShowUpload] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [showVectorSearch, setShowVectorSearch] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<RagDocInfo | null>(null);
  const [pickedFiles, setPickedFiles] = useState<RagPickedFile[]>([]);
  const [uploadTags, setUploadTags] = useState<string[]>([]);
  const [fileNameSearch, setFileNameSearch] = useState('');
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [showBatchTags, setShowBatchTags] = useState(false);
  const [batchMode, setBatchMode] = useState<'add' | 'remove'>('add');
  const [showTools, setShowTools] = useState(false);
  const [showBatchDelete, setShowBatchDelete] = useState(false);
  const [batchDeleting, setBatchDeleting] = useState(false);

  // disabled = OFF (default) OR initializing OR switching model. The switch
  // itself stays interactive (so the user can cancel a slow init by toggling
  // off) except during a model switch, where toggling would race the swap.
  const disabled = (!enabled && !initializing && !switchingModel) || initializing || switchingModel;

  const toggleSelect = (id: string) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const handleToggle = async (next: boolean) => {
    if (initializing) return;
    try {
      await toggleEnabled(next);
    } catch (err) {
      showToast(err instanceof Error ? err.message : t('pages.rag.memoryInsufficient'), 'error');
    }
  };

  const handlePick = async () => {
    const picked = await pickFiles();
    if (picked.length === 0) return;
    setPickedFiles((prev) => {
      const existing = new Set(prev.map((f) => f.path));
      const fresh = picked.filter((f) => !existing.has(f.path));
      return [...prev, ...fresh];
    });
  };

  const removeFile = (idx: number) => {
    setPickedFiles((prev) => prev.filter((_, i) => i !== idx));
  };

  const handleUploadConfirm = async () => {
    if (pickedFiles.length === 0) {
      showToast(t('pages.rag.noFileSelected'), 'error');
      return;
    }
    const { success, failed } = await upload(pickedFiles, uploadTags);
    // Only claim success when nothing failed - per-file errors are already
    // toasted inside upload. Showing a green "success" while files actually
    // failed (and thus aren't in the list) was misleading.
    if (failed === 0) {
      showToast(t('pages.rag.uploadConfirm') + ' ✓', 'success');
    } else if (success === 0) {
      showToast(t('pages.rag.uploadAllFailed'), 'error');
    } else {
      showToast(t('pages.rag.uploadPartialFailed', { success, failed }), 'error');
    }
    setPickedFiles([]);
    setUploadTags([]);
    setShowUpload(false);
  };

  // Batch add/remove tags: for each selected doc, compute new tags and persist.
  const handleBatchTags = async (tags: string[]) => {
    const cleaned = tags.map((x) => x.trim()).filter((x) => x.length > 0);
    if (cleaned.length === 0) {
      showToast(t('pages.rag.tagPlaceholder'), 'error');
      return;
    }
    for (const doc of ragDocs.filter((d) => selectedIds.has(d.id))) {
      const set = new Set(doc.tags || []);
      if (batchMode === 'add') cleaned.forEach((x) => set.add(x));
      else cleaned.forEach((x) => set.delete(x));
      await setTags(doc.id, Array.from(set));
    }
    showToast(t('pages.rag.tagsSaved') + ' ✓', 'success');
    setShowBatchTags(false);
  };

  const handleDeleteConfirm = async () => {
    if (!deleteTarget) return;
    await remove(deleteTarget.id);
    showToast(t('pages.rag.delete') + ' ✓', 'success');
    setDeleteTarget(null);
  };

  // Batch delete: remove every selected doc, then clear the selection.
  const handleBatchDelete = async () => {
    const ids = Array.from(selectedIds);
    if (ids.length === 0) {
      setShowBatchDelete(false);
      return;
    }
    setBatchDeleting(true);
    try {
      await removeMany(ids);
      showToast(t('pages.rag.delete') + ' ✓', 'success');
      setSelectedIds(new Set());
      setShowBatchDelete(false);
    } catch (err) {
      showToast(err instanceof Error ? err.message : t('pages.rag.delete'), 'error');
    } finally {
      setBatchDeleting(false);
    }
  };

  const handleOpenFolder = async (doc: RagDocInfo) => {
    try {
      await openLocation(doc.id);
    } catch (err) {
      showToast(t('pages.rag.openFolderNotReady'), 'error');
    }
  };

  // Update an existing document in place: open the OS file picker, then
  // overwrite the doc's content + meta + vectors with the picked file (id +
  // tags preserved). Drives the upload-progress overlay (file-level 1/1 +
  // char-level from reindex_doc) the same way `upload` does, so the user sees
  // both progress bars. `updatingId` drives the row's update-button spinner.
  // Requires RAG enabled; the button is disabled when RAG is off.
  const [updatingId, setUpdatingId] = useState<string | null>(null);
  const handleUpdate = async (doc: RagDocInfo) => {
    const picked = await pickFiles();
    if (picked.length === 0) return;
    setUpdatingId(doc.id);
    try {
      await updateDoc(doc.id, picked[0].path, doc.name);
      showToast(t('pages.rag.updateDone'), 'success');
    } catch (err) {
      showToast(err instanceof Error ? err.message : t('pages.rag.updateFailed'), 'error');
    } finally {
      setUpdatingId(null);
    }
  };

  // Fuzzy filename search (client-side, case-insensitive).
  const filteredDocs = useMemo(() => {
    const q = fileNameSearch.trim().toLowerCase();
    if (!q) return ragDocs;
    return ragDocs.filter((d) => d.name.toLowerCase().includes(q));
  }, [ragDocs, fileNameSearch]);

  // Names already in the library — to warn about overwrites in the upload dialog.
  const existingNames = useMemo(() => new Set(ragDocs.map((d) => d.name)), [ragDocs]);

  return (
    <>
    <div className={disabled ? 'opacity-60 pointer-events-none' : ''}>
      {/* Header: title + switch + memory warning on the right of the title */}
      <div className="flex items-end justify-between gap-4 mb-6">
        <div className="flex items-center gap-2.5 min-w-0">
          <h1 className="hub-h1">{t('pages.rag.title')}</h1>
          {/* Switch group — always interactive even when page is disabled */}
          <div className="flex items-center gap-1.5" style={{ pointerEvents: 'auto' }}>
            <Switch
              checked={enabled}
              onCheckedChange={handleToggle}
              disabled={initializing || switchingModel}
              aria-label={t('pages.rag.title')}
            />
            <span className="text-[12px] hub-mono" style={{ color: 'var(--hub-ink-3)' }}>
              {switchingModel ? t('pages.rag.switchingModel') : initializing ? (togglingTo === 'off' ? t('pages.rag.closing') : t('pages.rag.opening')) : enabled ? t('pages.rag.enabled') : t('pages.rag.disabled')}
            </span>
            <span
              className="inline-flex items-center justify-center cursor-help"
              style={{ color: 'var(--hub-err)' }}
              title={t('pages.rag.memoryWarn')}
            >
              <Info size={15} />
            </span>
            <button
              type="button"
              onClick={() => setShowTools(true)}
              disabled={!enabled}
              className="hub-btn sm"
              style={{ pointerEvents: 'auto' }}
              title={t('pages.rag.viewToolsHint')}
            >
              {t('pages.rag.viewTools')}
            </button>
            {/* Model size selector - next to the switch. Lists all sizes (ready
                are selectable -> auto-restart RAG with the new model; not-ready
                are shown with a Download button + progress). */}
            <ModelSelector
              models={models}
              currentModel={currentModel}
              modelDownload={modelDownload}
              disabled={initializing || switchingModel}
              onSelect={selectModel}
              onDownload={downloadModel}
              onRefresh={fetchModels}
            />
          </div>
        </div>

        {/* Top-right action buttons */}
        <div className="flex items-center gap-2" style={{ pointerEvents: 'auto' }}>
          <button onClick={() => setShowVectorSearch(true)} className="hub-btn primary" disabled={disabled}>
            <Sparkles size={13} /> {t('pages.rag.vectorSearch')}
          </button>
          <button onClick={() => setShowUpload(true)} className="hub-btn primary" disabled={disabled}>
            <Upload size={13} /> {t('pages.rag.upload')}
          </button>
          <button onClick={() => setShowSettings(true)} className="hub-btn" disabled={disabled}>
            <SlidersHorizontal size={13} /> {t('pages.rag.searchSettings')}
          </button>
        </div>
      </div>

      {/* Toolbar: filename fuzzy search */}
      <div className="flex items-center gap-2 mb-4" style={{ pointerEvents: 'auto' }}>
        <div
          className="hub-card flex items-center gap-2 px-2.5 flex-1"
          style={{ height: 30, background: 'var(--hub-surface)', maxWidth: 360 }}
        >
          <Search size={13} style={{ color: 'var(--hub-ink-3)' }} />
          <input
            value={fileNameSearch}
            onChange={(e) => setFileNameSearch(e.target.value)}
            placeholder={t('pages.rag.searchPlaceholder')}
            className="flex-1 bg-transparent outline-none text-[13px]"
            style={{ color: 'var(--hub-ink)' }}
          />
          {fileNameSearch && (
            <button onClick={() => setFileNameSearch('')} className="hub-icon-btn sm">
              <X size={11} />
            </button>
          )}
        </div>
        {selectedIds.size > 0 && (
          <div className="flex items-center gap-2">
            <span className="hub-mono text-[12px]" style={{ color: 'var(--hub-ink-3)' }}>
              {selectedIds.size}
            </span>
            <button
              className="hub-btn"
              disabled={disabled}
              onClick={() => {
                setBatchMode('add');
                setShowBatchTags(true);
              }}
            >
              <Tag size={13} /> {t('pages.rag.batchAddTags')}
            </button>
            <button
              className="hub-btn"
              disabled={disabled}
              onClick={() => {
                setBatchMode('remove');
                setShowBatchTags(true);
              }}
            >
              <Tag size={13} /> {t('pages.rag.batchRemoveTags')}
            </button>
            <button
              className="hub-btn"
              disabled={disabled}
              onClick={() => setShowBatchDelete(true)}
              style={{ color: 'var(--hub-err)' }}
            >
              <Trash2 size={13} /> {t('pages.rag.batchDelete')}
            </button>
            <button className="hub-icon-btn sm" onClick={() => setSelectedIds(new Set())} title={t('pages.rag.cancel')}>
              <X size={13} />
            </button>
          </div>
        )}
      </div>
      {filteredDocs.length === 0 ? (
        <div className="hub-card p-10 text-center" style={{ color: 'var(--hub-ink-3)' }}>
          <FileText size={20} className="mx-auto mb-2" />
          <div>{ragDocs.length === 0 ? t('pages.rag.empty') : t('pages.rag.noResults')}</div>
        </div>
      ) : (
        <div className="hub-card overflow-hidden">
          {/* Column header */}
          <div
            className="flex items-center"
            style={{
              padding: '8px 16px',
              borderBottom: '1px solid var(--hub-line-2)',
              fontSize: 11,
              color: 'var(--hub-ink-3)',
            }}
          >
            <span className="flex-1">{t('pages.rag.columnName')}</span>
            <span style={{ width: 90 }}>{t('pages.rag.columnSize')}</span>
            <span style={{ width: 64 }}>{t('pages.rag.columnChunks')}</span>
            <span style={{ width: 160 }}>{t('pages.rag.columnTime')}</span>
            <span style={{ width: 170 }} />
          </div>
          {filteredDocs.map((doc, idx) => {
            const checked = selectedIds.has(doc.id);
            return (
            <div
              key={doc.id}
              className="flex items-center transition-colors hover:bg-[var(--hub-surface-hover)]"
              style={{
                padding: '10px 16px',
                borderTop: idx === 0 ? 0 : '1px solid var(--hub-line-2)',
                background: checked ? 'var(--hub-surface)' : undefined,
              }}
            >
              <div className="flex flex-col gap-1 flex-1 min-w-0">
                <div className="flex items-center gap-2 min-w-0">
                  <input
                    type="checkbox"
                    checked={checked}
                    onChange={() => toggleSelect(doc.id)}
                    className="h-4 w-4 rounded flex-shrink-0"
                    style={{ accentColor: 'var(--hub-accent)' }}
                  />
                  <FileText size={14} style={{ color: 'var(--hub-ink-3)', flexShrink: 0 }} />
                  <span className="truncate text-[13px]" style={{ color: 'var(--hub-ink)' }} title={doc.name}>
                    {doc.name}
                  </span>
                  {doc.fileType && (
                    <span className="hub-tag flex-shrink-0" style={{ fontSize: 10 }}>
                      {doc.fileType}
                    </span>
                  )}
                  {doc.version > 1 && (
                    <span className="hub-tag flex-shrink-0" title={t('pages.rag.versionTitle', { v: doc.version })} style={{ fontSize: 10 }}>
                      v{doc.version}
                    </span>
                  )}
                </div>
                {(doc.tags || []).length > 0 && (
                  <div className="flex items-center gap-1 flex-wrap" style={{ paddingLeft: 26 }}>
                    {(doc.tags || []).map((tag) => (
                      <span key={tag} className="hub-tag" style={{ fontSize: 10 }}>
                        {tag}
                      </span>
                    ))}
                  </div>
                )}
                {doc.fileName && doc.fileName !== doc.name && (
                  <div
                    className="hub-mono truncate"
                    style={{ paddingLeft: 26, fontSize: 10, color: 'var(--hub-ink-3)' }}
                    title={doc.fileName}
                  >
                    {doc.fileName}
                  </div>
                )}
              </div>
              <span className="hub-mono text-[12px]" style={{ width: 90, color: 'var(--hub-ink-3)' }}>
                {formatSize(doc.size)}
              </span>
              <span className="hub-mono text-[12px]" style={{ width: 64, color: 'var(--hub-ink-3)' }}>
                {doc.chunkCount ?? 0}
              </span>
              <span className="hub-mono text-[12px]" style={{ width: 160, color: 'var(--hub-ink-3)' }}>
                {doc.uploadedAt || '-'}
              </span>
              <div className="flex items-center gap-1" style={{ width: 170 }}>
                <button
                  className="hub-icon-btn sm"
                  onClick={() => view(doc.id)}
                  title={t('pages.rag.view')}
                  disabled={disabled}
                >
                  {viewLoading ? <Loader2 size={13} className="animate-spin" /> : <Eye size={13} />}
                </button>
                <button
                  className="hub-icon-btn sm"
                  onClick={() => viewChunks(doc)}
                  title={t('pages.rag.viewChunks')}
                  disabled={disabled || (doc.chunkCount ?? 0) === 0}
                >
                  <Layers size={13} />
                </button>
                <button
                  className="hub-icon-btn sm"
                  onClick={() => handleUpdate(doc)}
                  title={t('pages.rag.update')}
                  disabled={disabled || updatingId === doc.id}
                >
                  {updatingId === doc.id ? <Loader2 size={13} className="animate-spin" /> : <RefreshCw size={13} />}
                </button>
                <button
                  className="hub-icon-btn sm"
                  onClick={() => handleOpenFolder(doc)}
                  title={t('pages.rag.openFolder')}
                  disabled={disabled}
                >
                  <FolderOpen size={13} />
                </button>
                <button
                  className="hub-icon-btn sm"
                  onClick={() => setDeleteTarget(doc)}
                  title={t('pages.rag.delete')}
                  disabled={disabled}
                  style={{ color: 'var(--hub-err)' }}
                >
                  <Trash2 size={13} />
                </button>
              </div>
            </div>
          );
          })}
        </div>
      )}

      {/* Upload dialog */}
      {showUpload && (
        <UploadDialog
          onClose={() => {
            setShowUpload(false);
            setPickedFiles([]);
            setUploadTags([]);
          }}
          pickedFiles={pickedFiles}
          existingNames={existingNames}
          onPick={handlePick}
          onRemoveFile={removeFile}
          onConfirm={handleUploadConfirm}
          tags={uploadTags}
          onTagsChange={setUploadTags}
        />
      )}

      {/* Search settings dialog */}
      {showSettings && (
        <SearchSettingsDialog
          initial={settings}
          maxContext={modelLimits.maxContext}
          recommendedChunkSize={modelLimits.chunkSize}
          recommendedChunkOverlap={modelLimits.chunkOverlap}
          onClose={() => setShowSettings(false)}
          onSave={(s) => {
            updateSettings(s);
            showToast(t('pages.rag.save') + ' ✓', 'success');
            setShowSettings(false);
          }}
        />
      )}

      {/* Vector search dialog */}
      {showVectorSearch && (
        <VectorSearchDialog
          onClose={() => setShowVectorSearch(false)}
          onSearch={search}
          results={searchResults}
          searching={searching}
        />
      )}

      {/* Batch tags dialog */}
      {showBatchTags && (
        <BatchTagsDialog
          mode={batchMode}
          count={selectedIds.size}
          onClose={() => setShowBatchTags(false)}
          onConfirm={async (tags) => {
            await handleBatchTags(tags);
          }}
        />
      )}

      {/* View dialog */}
      {viewedDoc && (
        <ViewDialog
          doc={viewedDoc}
          onClose={closeView}
          onSaveTags={async (tags) => {
            await setTags(viewedDoc.id, tags);
            await view(viewedDoc.id);
          }}
        />
      )}

      {/* View chunks dialog - lists all chunks (index + text) of a doc */}
      {chunksDoc && (
        <div className="fixed inset-0 bg-black/50 z-50 flex items-center justify-center p-4">
          <div className="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-2xl w-full mx-4 border border-gray-100 dark:border-gray-700 max-h-[85vh] flex flex-col">
            <div className="flex items-center justify-between p-5 border-b border-[var(--hub-line-2)]">
              <h2 className="text-xl font-bold text-gray-900 dark:text-gray-100">
                {t('pages.rag.chunksTitle', { name: chunksDoc.name })}
              </h2>
              <button onClick={closeChunks} className="hub-icon-btn sm">
                <X size={16} />
              </button>
            </div>
            <div className="flex-1 overflow-y-auto p-5 space-y-3">
              {chunksLoading ? (
                <div className="flex items-center justify-center py-12">
                  <Loader2 size={22} className="animate-spin" style={{ color: 'var(--hub-ink-3)' }} />
                </div>
              ) : chunksList.length === 0 ? (
                <p className="text-[13px] text-center py-12" style={{ color: 'var(--hub-ink-3)' }}>
                  {t('pages.rag.chunksEmpty')}
                </p>
              ) : (
                chunksList.map((c) => (
                  <div
                    key={c.chunkIndex}
                    className="rounded-lg border p-3"
                    style={{ borderColor: 'var(--hub-line-2)', background: 'var(--hub-bg-2)' }}
                  >
                    <div className="flex items-center justify-between mb-2">
                      <span
                        className="hub-mono text-[11px] px-2 py-0.5 rounded"
                        style={{ background: 'var(--hub-line)', color: 'var(--hub-ink-2)' }}
                      >
                        #{c.chunkIndex + 1}
                      </span>
                      <span className="hub-mono text-[11px]" style={{ color: 'var(--hub-ink-3)' }}>
                        {t('pages.rag.chunkTokens', { count: c.chunkText.length })}
                      </span>
                    </div>
                    <pre
                      className="text-[12px] whitespace-pre-wrap break-words"
                      style={{ color: 'var(--hub-ink)', fontFamily: 'inherit', margin: 0 }}
                    >
                      {c.chunkText}
                    </pre>
                  </div>
                ))
              )}
            </div>
            <div className="flex items-center justify-between p-5 border-t border-[var(--hub-line-2)]">
              <span className="text-[12px] hub-mono" style={{ color: 'var(--hub-ink-3)' }}>
                {t('pages.rag.chunksCount', { count: chunksList.length })}
              </span>
              <button onClick={closeChunks} className="hub-btn">
                {t('pages.rag.close')}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Delete confirm dialog */}
      {deleteTarget && (
        <div className="fixed inset-0 bg-black/50 z-50 flex items-center justify-center p-4">
          <div className="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-md w-full mx-4 border border-gray-100 dark:border-gray-700">
            <div className="flex items-center justify-between p-5 border-b border-[var(--hub-line-2)]">
              <h2 className="text-xl font-bold text-gray-900 dark:text-gray-100">{t('pages.rag.delete')}</h2>
              <button onClick={() => setDeleteTarget(null)} className="hub-icon-btn sm">
                <X size={16} />
              </button>
            </div>
            <div className="p-5">
              <p className="text-[13px]" style={{ color: 'var(--hub-ink-2)' }}>
                {t('pages.rag.deleteConfirm')}
              </p>
              <div className="mt-3 hub-card flex items-center gap-2" style={{ padding: '8px 12px', background: 'var(--hub-surface)' }}>
                <FileText size={14} style={{ color: 'var(--hub-ink-3)' }} />
                <span className="truncate text-[13px]" style={{ color: 'var(--hub-ink)' }} title={deleteTarget.name}>
                  {deleteTarget.name}
                </span>
              </div>
            </div>
            <div className="flex items-center justify-end gap-2 p-5 border-t border-[var(--hub-line-2)]">
              <button onClick={() => setDeleteTarget(null)} className="hub-btn">
                {t('pages.rag.cancel')}
              </button>
              <button onClick={handleDeleteConfirm} className="hub-btn danger">
                {t('pages.rag.delete')}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Batch delete confirm dialog */}
      {showBatchDelete && (
        <div className="fixed inset-0 bg-black/50 z-50 flex items-center justify-center p-4">
          <div className="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-md w-full mx-4 border border-gray-100 dark:border-gray-700">
            <div className="flex items-center justify-between p-5 border-b border-[var(--hub-line-2)]">
              <h2 className="text-xl font-bold text-gray-900 dark:text-gray-100">{t('pages.rag.batchDelete')}</h2>
              <button onClick={() => setShowBatchDelete(false)} className="hub-icon-btn sm">
                <X size={16} />
              </button>
            </div>
            <div className="p-5">
              <p className="text-[13px]" style={{ color: 'var(--hub-ink-2)' }}>
                {t('pages.rag.batchDeleteConfirm', { count: selectedIds.size })}
              </p>
            </div>
            <div className="flex items-center justify-end gap-2 p-5 border-t border-[var(--hub-line-2)]">
              <button onClick={() => setShowBatchDelete(false)} className="hub-btn" disabled={batchDeleting}>
                {t('pages.rag.cancel')}
              </button>
              <button onClick={handleBatchDelete} className="hub-btn danger" disabled={batchDeleting}>
                {batchDeleting ? <Loader2 size={13} className="animate-spin" /> : <Trash2 size={13} />} {t('pages.rag.delete')}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* View RAG tools dialog — shows the MCP tools RAG exposes (rag_search /
          rag_get / rag_tag_search) with their description + input schema. */}
      {showTools && <ToolsDialog onClose={() => setShowTools(false)} />}

      {/* Reindex confirmation: a model swap changed the embedding dim, so the
          old vector table was dropped (old embeddings gone). Ask the user
          before re-embedding all docs (expensive) instead of auto-running. */}
      {reindexConfirm && (
        <div className="fixed inset-0 bg-black/50 z-[60] flex items-center justify-center p-4">
          <div
            className="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-md w-full mx-4 border"
            style={{ borderColor: 'var(--hub-line-2)' }}
          >
            <div className="flex items-center justify-between p-5 border-b" style={{ borderColor: 'var(--hub-line-2)' }}>
              <h2 className="text-lg font-bold" style={{ color: 'var(--hub-ink)' }}>
                {t('pages.rag.reindexConfirmTitle')}
              </h2>
            </div>
            <div className="p-5 space-y-3">
              <p className="text-[13px] leading-relaxed" style={{ color: 'var(--hub-ink-3)' }}>
                {t('pages.rag.reindexConfirmMessage')}
              </p>
            </div>
            <div className="flex justify-end gap-2 p-5 pt-3 border-t" style={{ borderColor: 'var(--hub-line-2)' }}>
              <button onClick={cancelReindex} className="hub-btn">
                {t('pages.rag.cancel')}
              </button>
              <button onClick={confirmReindex} className="hub-btn primary">
                {t('pages.rag.reindexConfirmButton')}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Upload progress overlay */}
      {uploading && uploadProgress && (
        <div className="fixed inset-0 bg-black/40 z-[60] flex items-center justify-center p-4">
          <div
            className="hub-card w-full max-w-sm p-6 flex flex-col items-center gap-4 shadow-2xl"
            style={{ background: 'var(--hub-surface)' }}
          >
            <Loader2 size={22} className="animate-spin" style={{ color: 'var(--hub-ink-2)' }} />
            <div className="text-[13px] text-center" style={{ color: 'var(--hub-ink)' }}>
              {reindexing
                ? uploadProgress.name
                  ? t('pages.rag.reindexingFile', {
                      current: uploadProgress.current + 1,
                      total: uploadProgress.total,
                      name: uploadProgress.name,
                    })
                  : t('pages.rag.reindexingDone', { total: uploadProgress.total })
                : updatingDoc
                ? uploadProgress.name
                  ? t('pages.rag.updatingFile', { name: uploadProgress.name })
                  : t('pages.rag.update')
                : uploadProgress.name
                ? t('pages.rag.uploadingFile', {
                    current: uploadProgress.current + 1,
                    total: uploadProgress.total,
                    name: uploadProgress.name,
                  })
                : t('pages.rag.uploadingDone', { total: uploadProgress.total })}
            </div>
            {/* file-level progress bar (current/total files) */}
            <div className="w-full" style={{ height: 6, borderRadius: 3, background: 'var(--hub-line)', overflow: 'hidden' }}>
              <div
                style={{
                  width: `${uploadProgress.total > 0 ? (uploadProgress.current / uploadProgress.total) * 100 : 0}%`,
                  height: '100%',
                  background: 'var(--hub-ink)',
                  transition: 'width 0.2s ease',
                }}
              />
            </div>
            <div className="hub-mono text-[11px]" style={{ color: 'var(--hub-ink-3)' }}>
              {uploadProgress.current}/{uploadProgress.total}
            </div>

            {/* Per-document (character-based) progress bar — always shown while a
                file is actively being indexed (uploadProgress.name is non-empty),
                so the second bar is visible immediately rather than waiting for
                the first backend tick. The percentage/char counts come from the
                `rag://upload-progress` events (charProgress); until the first
                tick lands the bar sits at 0%. The fill uses --hub-accent with a
                --hub-ink fallback (the accent var is undefined in dark mode). */}
            {uploadProgress.name &&
              (() => {
                const matches = charProgress && charProgress.name === uploadProgress.name;
                const charsDone = matches ? charProgress!.charsDone : 0;
                const charsTotal = matches ? charProgress!.charsTotal : 0;
                const pct =
                  charsTotal > 0
                    ? Math.min(100, Math.round((charsDone / charsTotal) * 100))
                    : 0;
                return (
                  <div className="w-full flex flex-col gap-1" style={{ marginTop: 2 }}>
                    <div className="flex items-center justify-between">
                      <span className="text-[11px]" style={{ color: 'var(--hub-ink-3)' }}>
                        {t('pages.rag.docProgress')}
                      </span>
                      <span className="hub-mono text-[11px]" style={{ color: 'var(--hub-ink-3)' }}>
                        {pct}%
                      </span>
                    </div>
                    <div
                      style={{
                        height: 6,
                        borderRadius: 3,
                        background: 'var(--hub-line)',
                        overflow: 'hidden',
                      }}
                    >
                      <div
                        style={{
                          width: `${pct}%`,
                          height: '100%',
                          background: 'var(--hub-accent, var(--hub-ink))',
                          transition: 'width 0.15s ease',
                        }}
                      />
                    </div>
                    <div className="hub-mono text-[10px]" style={{ color: 'var(--hub-ink-3)' }}>
                      {charsTotal > 0
                        ? `${charsDone.toLocaleString()} / ${charsTotal.toLocaleString()} ${t('pages.rag.chars')}`
                        : t('pages.rag.docProgressPreparing')}
                    </div>
                  </div>
                );
              })()}
          </div>
        </div>
      )}

    </div>

      {/* Model loading overlay. A sibling of the `opacity-60` disabled
          wrapper (NOT a child) so it isn't dimmed by the parent's opacity —
          `opacity` creates a stacking context, so a dimmed child can't escape
          via z-index. Rendered at z-[60] with its own bg-black/40 scrim, full
          screen, matching the upload overlay's style. Shows a large spinning
          Loader2 + "switching"/"opening" label while the backend loads a model
          (toggle on) or swaps models (selectModel). */}
      {(initializing || switchingModel) && (
        <div className="fixed inset-0 bg-black/40 z-[60] flex items-center justify-center p-4">
          <div
            className="hub-card w-full max-w-sm p-6 flex flex-col items-center gap-4 shadow-2xl"
            style={{ background: 'var(--hub-surface)' }}
          >
            <Loader2 size={26} className="animate-spin" style={{ color: 'var(--hub-ink-2)' }} />
            <div className="text-[13px] text-center" style={{ color: 'var(--hub-ink)' }}>
              {switchingModel ? t('pages.rag.switchingModel') : togglingTo === 'off' ? t('pages.rag.closing') : t('pages.rag.opening')}
            </div>
          </div>
        </div>
      )}
    </>
  );
};

/** Model size selector - sits next to the RAG switch. Lists all sizes scanned
 *  from `runtimes/rag/model/<family>/<size>/`: ready sizes are selectable
 *  (switching auto-restarts RAG with the new model); not-ready-but-downloadable
 *  sizes show a Download button + inline progress. Refreshes the list on
 *  download completion (driven by the `rag://model-download` listener in
 *  useRagData). */
const ModelSelector: React.FC<{
  models: RagModelInfo[];
  currentModel: string | null;
  modelDownload: {
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
  } | null;
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
    <div className="relative" ref={wrapRef}>
      {/* Trigger button: current model + chevron. Selectable even when RAG is
          off (only `disabled` = initializing disables it). */}
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
            minWidth: 374,
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

/** Format a duration in seconds as "Mm Ss" (>60s) or "Ss" (compact ETA). */
const formatEta = (secs: number): string => {
  if (secs <= 0) return '--';
  if (secs < 60) return `${secs}s`;
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return `${m}m ${s.toString().padStart(2, '0')}s`;
};

/** Upload dialog with a custom file-picker button (i18n) + a list of selected
 *  files below it. The native input is hidden; a labeled button triggers it. */
const UploadDialog: React.FC<{
  onClose: () => void;
  pickedFiles: RagPickedFile[];
  existingNames: Set<string>;
  onPick: () => void;
  onRemoveFile: (idx: number) => void;
  onConfirm: () => void;
  tags: string[];
  onTagsChange: (tags: string[]) => void;
}> = ({ onClose, pickedFiles, existingNames, onPick, onRemoveFile, onConfirm, tags, onTagsChange }) => {
  const { t } = useTranslation();
  return (
    <div className="fixed inset-0 bg-black/50 z-50 flex items-center justify-center p-4">
      <div className="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-lg w-full mx-4 border border-gray-100 dark:border-gray-700 max-h-[90vh] flex flex-col">
        <div className="flex items-center justify-between p-5 border-b border-[var(--hub-line-2)]">
          <h2 className="text-xl font-bold text-gray-900 dark:text-gray-100">{t('pages.rag.uploadDialogTitle')}</h2>
          <button onClick={onClose} className="hub-icon-btn sm">
            <X size={16} />
          </button>
        </div>
        <div className="flex-1 overflow-y-auto p-5 space-y-3">
          <p className="text-[13px]" style={{ color: 'var(--hub-ink-3)' }}>
            {t('pages.rag.uploadHint')}
          </p>
          {/* OS file picker (Tauri dialog) — backend reads from disk by path,
              no bytes/base64 over IPC. */}
          <button type="button" onClick={onPick} className="hub-btn">
            <Upload size={13} /> {t('pages.rag.uploadSelect')}
          </button>
          {pickedFiles.length > 0 && (
            <div className="space-y-1">
              {pickedFiles.map((file, idx) => {
                const exists = existingNames.has(file.name);
                return (
                <div
                  key={`${file.path}-${idx}`}
                  className="flex items-center justify-between gap-2 hub-card"
                  style={{ padding: '6px 10px', background: 'var(--hub-surface)' }}
                >
                  <div className="flex items-center gap-2 min-w-0">
                    <FileText size={13} style={{ color: 'var(--hub-ink-3)', flexShrink: 0 }} />
                    <div className="flex flex-col min-w-0">
                      <span className="truncate text-[12.5px]" style={{ color: 'var(--hub-ink)' }} title={file.path}>
                        {file.name}
                      </span>
                      {exists && (
                        <span className="text-[11px]" style={{ color: 'var(--hub-err)' }}>
                          {t('pages.rag.nameExists')}
                        </span>
                      )}
                    </div>
                  </div>
                  <button className="hub-icon-btn sm" onClick={() => onRemoveFile(idx)} title={t('pages.rag.removeFile')}>
                    <X size={13} />
                  </button>
                </div>
                );
              })}
              <div className="text-[12px] hub-mono" style={{ color: 'var(--hub-ink-3)' }}>
                {t('pages.rag.filesSelected', { count: pickedFiles.length })}
              </div>
            </div>
          )}
          {/* Tags applied to every uploaded document in this batch */}
          <div>
            <div className="flex items-center gap-1.5 mb-1.5">
              <Tag size={13} style={{ color: 'var(--hub-ink-3)' }} />
              <label className="text-[13px] font-medium" style={{ color: 'var(--hub-ink)' }}>
                {t('pages.rag.tags')}
              </label>
            </div>
            <div className="hub-card" style={{ padding: '6px 10px', background: 'var(--hub-surface)' }}>
              <TagEditor tags={tags} onChange={onTagsChange} />
            </div>
          </div>
        </div>
        <div className="flex items-center justify-end gap-2 p-5 border-t border-[var(--hub-line-2)]">
          <button onClick={onClose} className="hub-btn">
            {t('pages.rag.cancel')}
          </button>
          <button onClick={onConfirm} className="hub-btn primary">
            {t('pages.rag.uploadConfirm')}
          </button>
        </div>
      </div>
    </div>
  );
};

/** Vector search dialog: a large text input + a search button. Results
 *  (document name, snippet, similarity score) are shown below after search. */
const VectorSearchDialog: React.FC<{
  onClose: () => void;
  onSearch: (query: string, tags: string[]) => Promise<void>;
  results: { docId: string; docName: string; title: string; snippet: string; score: number }[];
  searching: boolean;
}> = ({ onClose, onSearch, results, searching }) => {
  const { t } = useTranslation();
  const [query, setQuery] = useState('');
  const [tags, setTags] = useState<string[]>([]);
  const [hasSearched, setHasSearched] = useState(false);
  // Snippet is hidden by default in the result list (cluttered the overview);
  // a per-result button opens a modal showing the full snippet rendered.
  const [viewSnippet, setViewSnippet] = useState<{ docName: string; snippet: string } | null>(null);

  const handleSearch = async () => {
    if (!query.trim()) return;
    setHasSearched(true);
    await onSearch(query, tags);
  };

  return (
    <div className="fixed inset-0 bg-black/50 z-50 flex items-center justify-center p-4">
      <div className="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-2xl w-full mx-4 border border-gray-100 dark:border-gray-700 max-h-[90vh] flex flex-col">
        <div className="flex items-center justify-between p-5 border-b border-[var(--hub-line-2)]">
          <h2 className="text-xl font-bold text-gray-900 dark:text-gray-100">
            {t('pages.rag.vectorSearchDialogTitle')}
          </h2>
          <button onClick={onClose} className="hub-icon-btn sm">
            <X size={16} />
          </button>
        </div>
        <div className="flex-1 overflow-y-auto p-5 space-y-4">
          <p className="text-[12px]" style={{ color: 'var(--hub-ink-3)' }}>
            {t('pages.rag.vectorSearchHint')}
          </p>
          <textarea
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t('pages.rag.searchQueryPlaceholder')}
            rows={5}
            className="hub-input w-full resize-y"
            style={{ background: 'var(--hub-bg-2)', height: 'auto', minHeight: 107 }}
          />
          {/* Optional tag filter */}
          <div>
            <div className="flex items-center gap-1.5 mb-1">
              <Tag size={13} style={{ color: 'var(--hub-ink-3)' }} />
              <label className="text-[13px] font-medium" style={{ color: 'var(--hub-ink)' }}>
                {t('pages.rag.filterByTags')}
              </label>
            </div>
            <p className="text-[11px] mb-1.5" style={{ color: 'var(--hub-ink-3)' }}>
              {t('pages.rag.filterByTagsHint')}
            </p>
            <div className="hub-card" style={{ padding: '6px 10px', background: 'var(--hub-surface)' }}>
              <TagEditor tags={tags} onChange={setTags} />
            </div>
          </div>
          <div className="flex items-center justify-end gap-2">
            <button onClick={handleSearch} className="hub-btn primary" disabled={searching || !query.trim()}>
              {searching ? <Loader2 size={13} className="animate-spin" /> : <Sparkles size={13} />} {t('pages.rag.searchBtn')}
            </button>
          </div>
          {/* Results */}
          {hasSearched && !searching && (
            <div className="space-y-2">
              <div className="hub-sect" style={{ color: 'var(--hub-ink-3)', fontSize: 11 }}>
                {t('pages.rag.searchResults')} ({results.length})
              </div>
              {results.length === 0 ? (
                <div className="hub-card p-6 text-center text-[13px]" style={{ color: 'var(--hub-ink-3)' }}>
                  {t('pages.rag.noResults')}
                </div>
              ) : (
                results.map((r, idx) => (
                  <div
                    key={`${r.docId}-${idx}`}
                    className="hub-card"
                    style={{ padding: '10px 14px', background: 'var(--hub-surface)' }}
                  >
                    <div className="flex items-center justify-between gap-2">
                      <div className="flex items-center gap-2 min-w-0">
                        <FileText size={13} style={{ color: 'var(--hub-ink-3)', flexShrink: 0 }} />
                        <span className="truncate text-[13px] font-medium" style={{ color: 'var(--hub-ink)' }} title={r.title}>
                          {r.title}
                        </span>
                        {r.title !== r.docName && (
                          <span className="hub-mono truncate" style={{ fontSize: 11, color: 'var(--hub-ink-3)' }} title={r.docName}>
                            {r.docName}
                          </span>
                        )}
                      </div>
                      <div className="flex items-center gap-1.5 flex-shrink-0">
                        <span className="hub-tag accent hub-mono whitespace-nowrap" style={{ fontSize: 11 }}>
                          {t('pages.rag.resultScore')}: {r.score.toFixed(2)}
                        </span>
                        <button
                          type="button"
                          onClick={() => setViewSnippet({ docName: r.docName, snippet: r.snippet })}
                          className="hub-btn sm inline-flex items-center gap-1"
                          title={t('pages.rag.viewSnippet')}
                        >
                          <Eye size={12} />
                        </button>
                      </div>
                    </div>
                  </div>
                ))
              )}
            </div>
          )}
        </div>
        <div className="flex items-center justify-end gap-2 p-5 border-t border-[var(--hub-line-2)]">
          <button onClick={onClose} className="hub-btn">
            {t('pages.rag.close')}
          </button>
        </div>
      </div>
      {/* Snippet detail modal - rendered on top of the search dialog (z-[60])
          so the user can read the full snippet without it cluttering the list. */}
      {viewSnippet && (
        <div className="fixed inset-0 bg-black/60 z-[60] flex items-center justify-center p-4">
          <div className="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-3xl w-full mx-4 border border-gray-100 dark:border-gray-700 max-h-[85vh] flex flex-col">
            <div className="flex items-center justify-between p-5 border-b border-[var(--hub-line-2)]">
              <h2 className="text-base font-bold text-gray-900 dark:text-gray-100 truncate" title={viewSnippet.docName}>
                {viewSnippet.docName}
              </h2>
              <button onClick={() => setViewSnippet(null)} className="hub-icon-btn sm">
                <X size={16} />
              </button>
            </div>
            <div className="flex-1 overflow-y-auto p-5">
              <FileTypeRenderer content={viewSnippet.snippet} fileName={viewSnippet.docName} />
            </div>
            <div className="flex items-center justify-end gap-2 p-5 border-t border-[var(--hub-line-2)]">
              <button onClick={() => setViewSnippet(null)} className="hub-btn">
                {t('pages.rag.close')}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
};

/** Search settings dialog with linked vector/keyword weight sliders + max
 *  results. The two weights are linked: their sum cannot exceed 1.0. */
const SearchSettingsDialog: React.FC<{
  initial: RagSettings;
  maxContext: number;
  recommendedChunkSize?: number;
  recommendedChunkOverlap?: number;
  onClose: () => void;
  onSave: (s: RagSettings) => void;
}> = ({ initial, maxContext, recommendedChunkSize, recommendedChunkOverlap, onClose, onSave }) => {
  const { t } = useTranslation();
  const [vectorWeight, setVectorWeight] = useState(initial.vectorWeight);
  const [keywordWeight, setKeywordWeight] = useState(initial.keywordWeight);
  const [maxResults, setMaxResults] = useState(initial.maxResults);
  const [scoreThreshold, setScoreThreshold] = useState(initial.scoreThreshold);
  // chunk_size / chunk_overlap: `0` = "auto" (use the loaded model's
  // deploy.json-recommended values; the backend caps by max_context). A positive
  // value is an explicit override. `resolvedSize`/`resolvedOverlap` are the
  // defaults the backend applies when auto is on (deploy.json `chunkSize`/
  // `chunkOverlap`, else 1024/100, capped by max_context): shown in the
  // disabled sliders so the user sees what Auto does, and used to seed the
  // sliders when switching to manual.
  const resolvedSize = Math.max(1, Math.min(maxContext, recommendedChunkSize ?? 1024));
  const resolvedOverlap = recommendedChunkOverlap ?? 100;
  const [chunkSize, setChunkSize] = useState(initial.chunkSize === 0 ? resolvedSize : initial.chunkSize);
  const [chunkOverlap, setChunkOverlap] = useState(initial.chunkOverlap === 0 ? resolvedOverlap : initial.chunkOverlap);
  const [chunkAuto, setChunkAuto] = useState(initial.chunkSize === 0);
  const sum = vectorWeight + keywordWeight;

  const handleVectorChange = (v: number) => {
    setVectorWeight(v);
    setKeywordWeight(Math.max(0, Math.min(1, 1 - v)));
  };
  const handleKeywordChange = (v: number) => {
    setKeywordWeight(v);
    setVectorWeight(Math.max(0, Math.min(1, 1 - v)));
  };

  return (
    <div className="fixed inset-0 bg-black/50 z-50 flex items-center justify-center p-4">
      <div className="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-md w-full mx-4 border border-gray-100 dark:border-gray-700 max-h-[90vh] flex flex-col">
        <div className="flex items-center justify-between p-5 border-b border-[var(--hub-line-2)]">
          <h2 className="text-xl font-bold text-gray-900 dark:text-gray-100">{t('pages.rag.searchDialogTitle')}</h2>
          <button onClick={onClose} className="hub-icon-btn sm">
            <X size={16} />
          </button>
        </div>
        <div className="flex-1 overflow-y-auto p-5 space-y-5">
          <p className="text-[12px]" style={{ color: 'var(--hub-ink-3)' }}>
            {t('pages.rag.searchHint')}
          </p>
          <WeightSlider label={t('pages.rag.vectorWeight')} value={vectorWeight} onChange={handleVectorChange} />
          <WeightSlider label={t('pages.rag.keywordWeight')} value={keywordWeight} onChange={handleKeywordChange} />
          <div className="text-[12px] hub-mono" style={{ color: 'var(--hub-ink-3)' }}>
            {t('pages.rag.weightSumHint', { sum: sum.toFixed(2) })}
          </div>
          <div>
            <NumericSlider
              label={t('pages.rag.maxResults')}
              value={maxResults}
              min={1}
              max={100}
              onChange={setMaxResults}
            />
            <p className="mt-1 text-[12px]" style={{ color: 'var(--hub-ink-3)' }}>
              {t('pages.rag.maxResultsHint')}
            </p>
          </div>
          <div>
            <WeightSlider label={t('pages.rag.scoreThreshold')} value={scoreThreshold} onChange={setScoreThreshold} />
            <p className="mt-1 text-[12px]" style={{ color: 'var(--hub-ink-3)' }}>
              {t('pages.rag.scoreThresholdHint')}
            </p>
          </div>
          <div className="border-t border-[var(--hub-line-2)] pt-4 space-y-4">
            <p className="text-[12px] font-medium" style={{ color: 'var(--hub-ink-2)' }}>
              {t('pages.rag.chunkingSection')}
            </p>
            <label className="flex items-center gap-2 text-[13px] cursor-pointer" style={{ color: 'var(--hub-ink)' }}>
              <input
                type="checkbox"
                checked={chunkAuto}
                onChange={(e) => {
                  const auto = e.target.checked;
                  setChunkAuto(auto);
                  if (!auto) {
                    // Switching to manual: seed the sliders from the model's
                    // recommended values (clamped to maxContext; overlap capped at
                    // size-1) so the user starts from a sensible base.
                    const size = Math.max(1, Math.min(maxContext, resolvedSize));
                    setChunkSize(size);
                    setChunkOverlap(Math.max(0, Math.min(size - 1, resolvedOverlap)));
                  }
                }}
              />
              {t('pages.rag.chunkAuto')}
            </label>
            <p className="text-[12px]" style={{ color: 'var(--hub-ink-3)' }}>
              {chunkAuto ? t('pages.rag.chunkAutoHint') : t('pages.rag.chunkManualHint')}
            </p>
            <div>
              <NumericSlider
                label={t('pages.rag.chunkSize')}
                value={chunkAuto ? resolvedSize : chunkSize}
                min={1}
                max={maxContext}
                unit={t('pages.rag.unitChars')}
                disabled={chunkAuto}
                onChange={(v) => {
                  setChunkSize(v);
                  // Pull overlap back only if it now exceeds the (smaller) chunk size.
                  setChunkOverlap((prev) => (prev > v - 1 ? Math.max(0, v - 1) : prev));
                }}
              />
              <p className="mt-1 text-[12px]" style={{ color: 'var(--hub-ink-3)' }}>
                {t('pages.rag.chunkSizeHint', { max: maxContext })}
              </p>
            </div>
            <div>
              <NumericSlider
                label={t('pages.rag.chunkOverlap')}
                value={chunkAuto ? resolvedOverlap : chunkOverlap}
                min={0}
                max={Math.max(0, (chunkAuto ? resolvedSize : chunkSize) - 1)}
                unit={t('pages.rag.unitChars')}
                disabled={chunkAuto}
                onChange={setChunkOverlap}
              />
              <p className="mt-1 text-[12px]" style={{ color: 'var(--hub-ink-3)' }}>
                {t('pages.rag.chunkOverlapHint')}
              </p>
            </div>
          </div>
        </div>
        <div className="flex items-center justify-end gap-2 p-5 border-t border-[var(--hub-line-2)]">
          <button onClick={onClose} className="hub-btn">
            {t('pages.rag.cancel')}
          </button>
          <button
            onClick={() =>
              onSave({
                vectorWeight,
                keywordWeight,
                maxResults,
                scoreThreshold,
                // Auto: send 0 so the backend resolves per loaded model.
                chunkSize: chunkAuto ? 0 : chunkSize,
                chunkOverlap: chunkAuto ? 0 : chunkOverlap,
              })
            }
            className="hub-btn primary"
          >
            {t('pages.rag.save')}
          </button>
        </div>
      </div>
    </div>
  );
};

const WeightSlider: React.FC<{ label: string; value: number; onChange: (v: number) => void }> = ({
  label,
  value,
  onChange,
}) => {
  return (
    <div>
      <div className="flex items-center justify-between mb-1 gap-2">
        <label className="text-[13px] font-medium" style={{ color: 'var(--hub-ink)' }}>
          {label}
        </label>
        {/* Editable value - a slider alone is hard to set precisely. The field
         * clamps to [0,1] and keeps two decimals, matching the slider step. */}
        <input
          type="number"
          min={0}
          max={1}
          step={0.01}
          value={value.toFixed(2)}
          onChange={(e) => {
            const v = parseFloat(e.target.value);
            if (Number.isNaN(v)) return;
            onChange(Math.max(0, Math.min(1, v)));
          }}
          className="hub-mono text-[12px] text-right"
          style={{ width: 56, background: 'var(--hub-bg-2)', color: 'var(--hub-ink)' }}
        />
      </div>
      <input
        type="range"
        min={0}
        max={1}
        step={0.01}
        value={value}
        onChange={(e) => onChange(parseFloat(e.target.value))}
        className="w-full"
        style={{ accentColor: 'var(--hub-accent)' }}
      />
    </div>
  );
};

/** Generic integer range slider (label + editable value + native range). Used
 *  for chunk_size / chunk_overlap / max_results. `unit` is shown next to the
 *  range bounds (e.g. "字" for chunk params). */
const NumericSlider: React.FC<{
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  unit?: string;
  disabled?: boolean;
  onChange: (v: number) => void;
}> = ({ label, value, min, max, step = 1, unit, disabled = false, onChange }) => {
  const lo = Math.min(min, max);
  const hi = Math.max(min, max);
  const unitSuffix = unit ? ` ${unit}` : '';
  const disabledStyle = disabled ? { opacity: 0.5, cursor: 'not-allowed' } : {};
  return (
    <div style={disabledStyle}>
      <div className="flex items-center justify-between mb-1 gap-2">
        <label className="text-[13px] font-medium" style={{ color: 'var(--hub-ink)' }}>
          {label}
        </label>
        <span className="hub-mono text-[12px] flex items-center gap-1.5" style={{ color: 'var(--hub-ink-3)' }}>
          {/* Editable value - clamp to [lo,hi] on blur/enter so the slider and
           * value stay consistent; NaN (mid-typing) is ignored. */}
          <input
            type="number"
            min={lo}
            max={hi}
            step={step}
            value={value}
            disabled={disabled}
            onChange={(e) => {
              const v = parseInt(e.target.value, 10);
              if (Number.isNaN(v)) return;
              onChange(Math.max(lo, Math.min(hi, v)));
            }}
            className="hub-mono text-[12px] text-right"
            style={{ width: 56, background: 'var(--hub-bg-2)', color: 'var(--hub-ink)' }}
          />
          <span style={{ fontSize: 10 }}>
            ({lo}–{hi}{unitSuffix})
          </span>
        </span>
      </div>
      <input
        type="range"
        min={lo}
        max={hi}
        step={step}
        value={Math.max(lo, Math.min(value, hi))}
        disabled={disabled}
        onChange={(e) => {
          const v = parseInt(e.target.value, 10);
          if (Number.isNaN(v)) return;
          onChange(v);
        }}
        className="w-full"
        style={{ accentColor: 'var(--hub-accent)' }}
      />
    </div>
  );
};

/** Dialog showing the MCP tools RAG exposes (rag_search / rag_get /
 *  rag_tag_search) - name, description, and input schema. Read-only. */
const ToolsDialog: React.FC<{ onClose: () => void }> = ({ onClose }) => {
  const { t } = useTranslation();
  const [tools, setTools] = useState<Record<string, unknown>[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');

  useEffect(() => {
    let alive = true;
    getRagTools()
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
          <h2 className="text-xl font-bold text-gray-900 dark:text-gray-100">{t('pages.rag.viewTools')}</h2>
          <button onClick={onClose} className="hub-icon-btn sm">
            <X size={16} />
          </button>
        </div>
        <div className="flex-1 overflow-y-auto p-5 space-y-3">
          {loading ? (
            <div className="flex items-center gap-2 text-[13px]" style={{ color: 'var(--hub-ink-3)' }}>
              <Loader2 size={14} className="animate-spin" /> {t('pages.rag.opening')}
            </div>
          ) : error ? (
            <div className="text-[13px]" style={{ color: 'var(--hub-err)' }}>
              {error}
            </div>
          ) : tools.length === 0 ? (
            <div className="text-[13px]" style={{ color: 'var(--hub-ink-3)' }}>
              {t('pages.rag.toolsEmpty')}
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
            {t('pages.rag.close')}
          </button>
        </div>
      </div>
    </div>
  );
};

/** Reusable tag editor: chips with remove + an add input (Enter or comma). */
const TagEditor: React.FC<{ tags: string[]; onChange: (tags: string[]) => void }> = ({
  tags,
  onChange,
}) => {
  const { t } = useTranslation();
  const [draft, setDraft] = useState('');

  const commit = () => {
    const parts = draft
      .split(',')
      .map((s) => s.trim())
      .filter((s) => s.length > 0 && !tags.includes(s));
    if (parts.length > 0) onChange([...tags, ...parts]);
    setDraft('');
  };

  return (
    <div className="flex items-center gap-1.5 flex-wrap" style={{ minHeight: 30 }}>
      {tags.map((tag) => (
        <span
          key={tag}
          className="hub-tag flex items-center gap-1"
          style={{ fontSize: 11, padding: '2px 6px' }}
        >
          {tag}
          <button
            type="button"
            onClick={() => onChange(tags.filter((x) => x !== tag))}
            className="inline-flex"
            style={{ lineHeight: 1 }}
            title={t('pages.rag.removeFile')}
          >
            <X size={11} />
          </button>
        </span>
      ))}
      <input
        type="text"
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ',') {
            e.preventDefault();
            commit();
          }
        }}
        onBlur={commit}
        placeholder={t('pages.rag.tagPlaceholder')}
        className="flex-1 min-w-[120px] bg-transparent outline-none text-[12.5px]"
        style={{ color: 'var(--hub-ink)', minWidth: 120 }}
      />
    </div>
  );
};

/** View dialog: show doc content + editable tags. Tags are persisted in real
 *  time on each add/remove (no Save button) — `onSaveTags(next)` is called
 *  with the full new list on every change. */
const ViewDialog: React.FC<{
  doc: RagDoc;
  onClose: () => void;
  onSaveTags: (tags: string[]) => Promise<void>;
}> = ({ doc, onClose, onSaveTags }) => {
  const { t } = useTranslation();
  const [tags, setTags] = useState<string[]>(doc.tags || []);
  const [busy, setBusy] = useState(false);
  // Render vs source view + zoom level. Zoom is gesture-driven (Ctrl/Cmd +
  // wheel, or trackpad pinch which the WebView synthesizes as ctrl+wheel) -
  // no buttons.
  const [mode, setMode] = useState<'render' | 'source'>('render');
  const [zoom, setZoom] = useState(1);
  const contentRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  // Mirror zoom into a ref so the gesture handlers (bound once) read the
  // latest value without re-binding on every zoom change.
  const zoomRef = useRef(1);
  zoomRef.current = zoom;
  // Apply zoom via CSS `zoom` (NOT transform: scale). `zoom` affects layout -
  // the content reflows and the parent's overflow-y-auto scrolls naturally,
  // with no visual overflow / draggable artefact (transform: scale left the
  // scaled content overflowing the wrapper, which the WebView let the user
  // drag around). Set via setProperty because CSSProperties doesn't type the
  // (non-standard but Safari+Chromium-supported) `zoom` property.
  useEffect(() => {
    contentRef.current?.style.setProperty('zoom', String(zoom));
  }, [zoom]);
  // Gesture zoom: Ctrl/Cmd + wheel (Chromium WebView2 + Safari ctrl+wheel),
  // AND Mac trackpad pinch (WKWebView fires Safari's gesturestart/gesturechange
  // with e.scale = cumulative scale since gesturestart). preventDefault stops
  // browser page zoom; needs non-passive listeners.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const clamp = (n: number) => Math.min(3, Math.max(0.5, +n.toFixed(3)));
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey && !e.metaKey) return;
      e.preventDefault();
      setZoom(clamp(zoomRef.current - e.deltaY * 0.005));
    };
    let startZoom = 1;
    const onGestureStart = (e: Event) => {
      e.preventDefault();
      startZoom = zoomRef.current;
    };
    const onGestureChange = (e: Event) => {
      e.preventDefault();
      const scale = (e as unknown as { scale?: number }).scale ?? 1;
      setZoom(clamp(startZoom * scale));
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    el.addEventListener('gesturestart', onGestureStart as EventListener);
    el.addEventListener('gesturechange', onGestureChange as EventListener);
    return () => {
      el.removeEventListener('wheel', onWheel);
      el.removeEventListener('gesturestart', onGestureStart as EventListener);
      el.removeEventListener('gesturechange', onGestureChange as EventListener);
    };
  }, []);

  // Sync from the (re-fetched) doc prop after each save completes.
  useEffect(() => {
    setTags(doc.tags || []);
  }, [doc]);

  const handleChange = async (next: string[]) => {
    setTags(next); // optimistic
    setBusy(true);
    try {
      await onSaveTags(next);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="fixed inset-0 bg-black/50 z-50 flex items-center justify-center p-4">
      <div className="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-5xl w-full mx-4 border border-gray-100 dark:border-gray-700 max-h-[90vh] flex flex-col">
        <div className="flex items-center justify-between p-5 border-b border-[var(--hub-line-2)]">
          <h2 className="text-xl font-bold text-gray-900 dark:text-gray-100 truncate" title={doc.name}>
            {doc.name}
          </h2>
          <button onClick={onClose} className="hub-icon-btn sm">
            <X size={16} />
          </button>
        </div>
        <div ref={scrollRef} className="flex-1 overflow-y-auto p-5 space-y-4">
          {/* Tags editor (real-time) */}
          <div>
            <div className="flex items-center gap-1.5 mb-1.5">
              <Tag size={13} style={{ color: 'var(--hub-ink-3)' }} />
              <label className="text-[13px] font-medium" style={{ color: 'var(--hub-ink)' }}>
                {t('pages.rag.tags')}
              </label>
              {busy && <Loader2 size={12} className="animate-spin" style={{ color: 'var(--hub-ink-3)' }} />}
            </div>
            <div className="hub-card" style={{ padding: '6px 10px', background: 'var(--hub-surface)' }}>
              <TagEditor tags={tags} onChange={handleChange} />
            </div>
          </div>
          {/* Content toolbar: render/source toggle. Zoom is gesture-only
              (Ctrl/Cmd + wheel or pinch) - show the level as a hint. */}
          <div className="flex items-center justify-between">
            <button
              type="button"
              onClick={() => setMode((m) => (m === 'render' ? 'source' : 'render'))}
              className="hub-btn sm inline-flex items-center gap-1"
              title={mode === 'render' ? t('pages.rag.viewSource') : t('pages.rag.viewRender')}
            >
              {mode === 'render' ? <Code size={13} /> : <Eye size={13} />}
              <span>{mode === 'render' ? t('pages.rag.viewSource') : t('pages.rag.viewRender')}</span>
            </button>
            <div className="flex items-center gap-1">
              <button
                type="button"
                onClick={() => setZoom((z) => Math.max(0.5, +(z - 0.1).toFixed(2)))}
                className="hub-icon-btn sm"
                title={t('pages.rag.zoomOut')}
              >
                <Minus size={13} />
              </button>
              <span
                className="text-[11px] hub-mono"
                style={{ minWidth: 40, textAlign: 'center', color: 'var(--hub-ink-3)' }}
                title={t('pages.rag.zoomHint')}
              >
                {Math.round(zoom * 100)}%
              </span>
              <button
                type="button"
                onClick={() => setZoom((z) => Math.min(3, +(z + 0.1).toFixed(2)))}
                className="hub-icon-btn sm"
                title={t('pages.rag.zoomIn')}
              >
                <Plus size={13} />
              </button>
              {zoom !== 1 && (
                <button
                  type="button"
                  onClick={() => setZoom(1)}
                  className="hub-btn sm"
                  title={t('pages.rag.zoomReset')}
                >
                  {t('pages.rag.zoomReset')}
                </button>
              )}
            </div>
          </div>
          {/* Content - rendered by file type. Zoom is applied via CSS `zoom`
              on this div (layout-affecting, so no overflow/drag artefact).
              Background + padding + radius give the content a clear card edge
              so zoom looks intentional (not floating text on the dialog). */}
          <div
            ref={contentRef}
            style={{ background: 'var(--hub-bg)', padding: 12, borderRadius: 6 }}
          >
            <FileTypeRenderer
              content={doc.content}
              fileName={doc.name}
              fileType={doc.fileType}
              raw={mode === 'source'}
            />
          </div>
        </div>
        <div className="flex items-center justify-end gap-2 p-5 border-t border-[var(--hub-line-2)]">
          <button onClick={onClose} className="hub-btn">
            {t('pages.rag.close')}
          </button>
        </div>
      </div>
    </div>
  );
};

/** Batch tags dialog: add or remove a comma-separated tag list for the
 *  currently selected documents. */
const BatchTagsDialog: React.FC<{
  mode: 'add' | 'remove';
  count: number;
  onClose: () => void;
  onConfirm: (tags: string[]) => Promise<void>;
}> = ({ mode, count, onClose, onConfirm }) => {
  const { t } = useTranslation();
  const [tags, setTags] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);

  const handleConfirm = async () => {
    setBusy(true);
    try {
      await onConfirm(tags);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="fixed inset-0 bg-black/50 z-50 flex items-center justify-center p-4">
      <div className="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-md w-full mx-4 border border-gray-100 dark:border-gray-700">
        <div className="flex items-center justify-between p-5 border-b border-[var(--hub-line-2)]">
          <h2 className="text-xl font-bold text-gray-900 dark:text-gray-100">
            {mode === 'add' ? t('pages.rag.batchAddTags') : t('pages.rag.batchRemoveTags')}
          </h2>
          <button onClick={onClose} className="hub-icon-btn sm">
            <X size={16} />
          </button>
        </div>
        <div className="p-5 space-y-3">
          <p className="text-[12px]" style={{ color: 'var(--hub-ink-3)' }}>
            {t('pages.rag.batchTagsHint')}
          </p>
          <div className="hub-mono text-[12px]" style={{ color: 'var(--hub-ink-3)' }}>
            {count} selected
          </div>
          <div className="hub-card" style={{ padding: '6px 10px', background: 'var(--hub-surface)' }}>
            <TagEditor tags={tags} onChange={setTags} />
          </div>
        </div>
        <div className="flex items-center justify-end gap-2 p-5 border-t border-[var(--hub-line-2)]">
          <button onClick={onClose} className="hub-btn">
            {t('pages.rag.cancel')}
          </button>
          <button onClick={handleConfirm} className="hub-btn primary" disabled={busy || tags.length === 0}>
            {busy ? <Loader2 size={13} className="animate-spin" /> : <Tag size={13} />} {t('pages.rag.saveTags')}
          </button>
        </div>
      </div>
    </div>
  );
};

export default RagPage;
