import React, { createContext, useContext, useState, useCallback, useRef, useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useAuth } from '@/contexts/AuthContext';
import { useToast } from '@/contexts/ToastContext';
import { isTauri } from '@/utils/tauriClient';
import { RagChunk, RagDoc, RagDocInfo, RagModelInfo, RagModelLimits, RagPickedFile, RagSettings, RagSearchResult } from '@/types';
import {
  listRagDocs,
  getRagSettings,
  getRagModelLimits,
  saveRagSettings,
  deleteRagDoc,
  updateRagDoc,
  getRagDoc,
  getRagChunks,
  uploadRagDoc,
  pickRagFiles,
  openRagFileLocation,
  ragToggle,
  ragStatus,
  searchRagDocs,
  setRagTags,
  reindexAllRag,
  listRagModels,
  currentRagModel,
  selectRagModel,
  downloadRagModel,
} from '@/services/ragService';

/**
 * RAG data store, shared app-wide via context so the sidebar badge and the
 * RAG page stay in sync (e.g. deleting a doc updates both). Mounted once at
 * the app root (see RagDataProvider in App.tsx).
 *
 * State machine for the RAG switch:
 *   off → (toggle on) → initializing → ready ; ready → (toggle off) → off
 * The backend `rag_toggle(true)` blocks until the embedding model + vector
 * DB are ready; while it runs, `rag_status().initializing` is true and the
 * page grays out.
 */
type RagDataValue = ReturnType<typeof useRagDataState>;
const RagDataContext = createContext<RagDataValue | undefined>(undefined);

const useRagDataState = () => {
  const { t } = useTranslation();
  const { auth } = useAuth();
  const { showToast } = useToast();

  const [ragDocs, setRagDocs] = useState<RagDocInfo[]>([]);
  const [enabled, setEnabled] = useState(false);
  const [initializing, setInitializing] = useState(false);
  // Target state of an in-flight toggle ('on' | 'off' | null) so the loading
  // overlay can say "开启中" vs "关闭中" — `initializing` alone can't tell the
  // two apart (both set it true). Cleared when the toggle resolves.
  const [togglingTo, setTogglingTo] = useState<'on' | 'off' | null>(null);
  // True while switching the embedding model (selectModel). Distinct from
  // `initializing` (toggle on/off) so the switch can show "切换中" instead of
  // "开启中". Either flag grays out the page.
  const [switchingModel, setSwitchingModel] = useState(false);
  const [settings, setSettings] = useState<RagSettings>({ vectorWeight: 0.9, keywordWeight: 0.1, maxResults: 20, scoreThreshold: 0.65, chunkSize: 0, chunkOverlap: 0 });
  const [modelLimits, setModelLimits] = useState<RagModelLimits>({ maxContext: 2048 });
  const [viewedDoc, setViewedDoc] = useState<RagDoc | null>(null);
  const [viewLoading, setViewLoading] = useState(false);
  // "View chunks" dialog: the doc whose chunks are shown + the loaded chunks +
  // a loading flag while fetching (RAG must be enabled to read lancedb).
  const [chunksDoc, setChunksDoc] = useState<RagDocInfo | null>(null);
  const [chunksList, setChunksList] = useState<RagChunk[]>([]);
  const [chunksLoading, setChunksLoading] = useState(false);
  const [searchResults, setSearchResults] = useState<RagSearchResult[]>([]);
  const [searching, setSearching] = useState(false);
  const [uploading, setUploading] = useState(false);
  const [uploadProgress, setUploadProgress] = useState<{ current: number; total: number; name: string } | null>(null);
  // Per-document (character-based) progress, updated from the backend's
  // `rag://upload-progress` event during indexing. `null` outside an upload;
  // the UI shows it as a SECOND bar under the per-file bar. `charsDone` is the
  // number of document characters embedded so far, `charsTotal` the whole doc.
  const [charProgress, setCharProgress] = useState<{ name: string; charsDone: number; charsTotal: number } | null>(null);
  // True while a model-swap-triggered reindex of all docs is running. The
  // upload overlay is reused (same bars); `reindexing` just switches the
  // title text from "uploading" to "reindexing".
  const [reindexing, setReindexing] = useState(false);
  // True while a single-doc update (updateDoc) is re-embedding — reuses the
  // upload overlay (same bars) but switches the title text to "updating".
  const [updatingDoc, setUpdatingDoc] = useState(false);
  // When a model swap changes the embedding dim, the backend recreates the
  // vector table (old embeddings gone). We DON'T auto-reindex - we set this
  // flag so the page can prompt the user to confirm before re-embedding all
  // docs (which is expensive). Confirm -> reindexAll; cancel -> leave docs
  // un-reindexed (they show 0 chunks until manually re-uploaded/reindexed).
  const [reindexConfirm, setReindexConfirm] = useState(false);
  // The model that was selected BEFORE a dim-changing switch (saved so a
  // cancelled reindex confirm can revert to it). Set only by `selectModel`;
  // cleared by confirm/cancel.
  const [prevModel, setPrevModel] = useState<string | null>(null);
  // Model selection: available sizes (dropdown), the current size, and the
  // download progress for a size being fetched via its download.url.
  const [models, setModels] = useState<RagModelInfo[]>([]);
  const [currentModel, setCurrentModel] = useState<string | null>(null);
  const [modelDownload, setModelDownload] = useState<{
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
  } | null>(null);
  const mounted = useRef(true);

  const fetchDocs = useCallback(async () => {
    try {
      const data = await listRagDocs();
      if (mounted.current) setRagDocs(data);
    } catch {
      if (mounted.current) setRagDocs([]);
    }
  }, []);

  // Refresh the model-size dropdown + the current selection.
  const fetchModels = useCallback(async () => {
    try {
      const [list, cur] = await Promise.all([listRagModels(), currentRagModel()]);
      if (!mounted.current) return;
      setModels(list);
      if (cur) {
        setCurrentModel(cur);
      } else if (list.length > 0) {
        // Never selected before - default to the size whose deploy.json has
        // "default": true (and is ready), else the first ready size. UI-only:
        // the backend persists the default on next `start`. Prevents the
        // dropdown from showing a blank placeholder on first run.
        const fallback = list.find((m) => m.default && m.ready) ?? list.find((m) => m.ready);
        setCurrentModel(fallback ? fallback.size : null);
      } else {
        setCurrentModel(null);
      }
    } catch {
      if (mounted.current) setModels([]);
    }
  }, []);

  const fetchSettings = useCallback(async () => {
    try {
      const s = await getRagSettings();
      if (mounted.current) setSettings(s);
    } catch {
      // keep defaults
    }
  }, []);

  // Model context window (tokens) - caps the chunk_size input in the UI.
  const fetchModelLimits = useCallback(async () => {
    try {
      const m = await getRagModelLimits();
      if (mounted.current) setModelLimits(m);
    } catch {
      // keep default 2048
    }
  }, []);

  // Sync status from the backend on mount (in case RAG was already on).
  const syncStatus = useCallback(async () => {
    try {
      const st = await ragStatus();
      if (mounted.current) {
        setEnabled(st.enabled);
        setInitializing(st.initializing);
      }
    } catch {
      // ignore
    }
  }, []);

  // Clear state while unauthenticated.
  useEffect(() => {
    if (auth.loading || auth.isAuthenticated) return;
    setRagDocs([]);
    setEnabled(false);
    setInitializing(false);
  }, [auth.loading, auth.isAuthenticated]);

  useEffect(() => {
    mounted.current = true;
    if (auth.loading || !auth.isAuthenticated) return;
    fetchDocs();
    fetchSettings();
    fetchModelLimits();
    fetchModels();
    syncStatus();
    return () => {
      mounted.current = false;
    };
  }, [auth.loading, auth.isAuthenticated, fetchDocs, fetchSettings, fetchModelLimits, fetchModels, syncStatus]);

  // Listen for per-document (character-based) progress events from the
  // backend during indexing. The backend emits one event per embedding batch
  // (`rag://upload-progress`); we update the second progress bar in the upload
  // overlay. Cleared when an upload starts/ends (see `upload`).
  useEffect(() => {
    if (!isTauri()) return;
    let unlistenUpload: UnlistenFn | undefined;
    let unlistenReindex: UnlistenFn | undefined;
    let unlistenDownload: UnlistenFn | undefined;
    let cancelled = false;
    listen<{ name: string; charsDone: number; charsTotal: number }>('rag://upload-progress', (event) => {
      const p = event.payload;
      if (!p || !mounted.current) return;
      setCharProgress({ name: p.name, charsDone: p.charsDone, charsTotal: p.charsTotal });
    }).then((un) => {
      if (cancelled) un();
      else unlistenUpload = un;
    });

    // File-level progress during `reindex_all` (model-swap reindex). Drives
    // the SAME `uploadProgress` state as imports so the upload overlay reuses
    // unchanged; only the title text differs (see `reindexing`).
    listen<{ current: number; total: number; name: string }>('rag://reindex-progress', (event) => {
      const p = event.payload;
      if (!p || !mounted.current) return;
      setUploadProgress({ current: p.current, total: p.total, name: p.name });
    }).then((un) => {
      if (cancelled) un();
      else unlistenReindex = un;
    });

    // Model download progress (download.url -> .zip extract). Drives the
    // download sub-button's progress bar + the "done" state that flips a size
    // from downloadable to ready.
    listen<{
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
    }>('rag://model-download', (event) => {
      const p = event.payload;
      if (!p || !mounted.current) return;
      setModelDownload({ ...p });
      if (p.phase === 'done' || p.phase === 'error') {
        // Refresh the model list so the downloaded size becomes "ready" /
        // selectable. Clear the progress shortly after.
        fetchModels();
        if (p.phase === 'done') {
          setTimeout(() => {
            if (mounted.current) setModelDownload(null);
          }, 1500);
        }
      }
    }).then((un) => {
      if (cancelled) un();
      else unlistenDownload = un;
    });

    return () => {
      cancelled = true;
      unlistenUpload?.();
      unlistenReindex?.();
      unlistenDownload?.();
    };
  }, [fetchModels]);

  // While the backend is initializing (e.g. auto-start on boot after restart),
  // poll rag_status until it settles, so the switch transitions out of
  // "opening" without a manual refresh.
  useEffect(() => {
    if (!initializing) return;
    const id = window.setInterval(async () => {
      try {
        const st = await ragStatus();
        if (mounted.current) {
          setEnabled(st.enabled);
          setInitializing(st.initializing);
        }
      } catch {
        // ignore
      }
    }, 1000);
    return () => window.clearInterval(id);
  }, [initializing]);

  // Re-embed every doc with the currently-loaded model, after a model swap
  // recreated the vector table (different embedding dim). Reuses the upload
  // overlay (same bars) — the backend emits `rag://reindex-progress`
  // (file-level) and `rag://upload-progress` (char-level) per doc. Triggered
  // automatically from `toggleEnabled` when `status.needsReindex` is set.
  const reindexAll = useCallback(async (): Promise<void> => {
    console.log('[RAG] reindexAll: start (uploading=true, overlay should show)');
    setReindexing(true);
    setUploading(true);
    setUploadProgress({ current: 0, total: 0, name: '' });
    setCharProgress(null);
    try {
      const done = await reindexAllRag();
      console.log('[RAG] reindexAll: backend returned', done);
      await fetchDocs();
      // Success toast gives explicit feedback even if the progress overlay
      // flashed too briefly to notice (few/small docs, fast GPU, or per-doc
      // embed failures swallowed by the backend's match).
      showToast(t('pages.rag.reindexDone', { count: done }), 'success');
    } catch (err) {
      showToast(err instanceof Error ? err.message : t('pages.rag.reindexFailed'), 'error');
      console.error('[RAG] reindex failed', err);
    } finally {
      if (mounted.current) {
        setReindexing(false);
        setUploading(false);
        setUploadProgress(null);
        setCharProgress(null);
      }
    }
  }, [fetchDocs, t, showToast]);

  // User confirmed the reindex prompt (model dim changed) -> run it now.
  const confirmReindex = useCallback(async () => {
    setReindexConfirm(false);
    setPrevModel(null);
    await reindexAll();
  }, [reindexAll]);

  // User cancelled the reindex prompt (from a dim-changing model switch) ->
  // REVERT to the model that was selected before the switch. Silent: no toast,
  // and ignore the revert's own `needsReindex` (the user already declined
  // reindexing, so we don't re-prompt - the docs stay at 0 chunks until
  // manually re-uploaded/reindexed). Only reverts when this confirm came from
  // `selectModel` (prevModel set); a confirm from `toggleEnabled` has no
  // prevModel -> just dismiss.
  const cancelReindex = useCallback(async () => {
    setReindexConfirm(false);
    const revertTo = prevModel;
    setPrevModel(null);
    if (!revertTo) return;
    setInitializing(true);
    try {
      const st = await selectRagModel(revertTo);
      if (!mounted.current) return;
      setCurrentModel(revertTo);
      setEnabled(st.enabled);
      setInitializing(st.initializing);
      if (st.enabled) fetchModelLimits();
      // Deliberately ignore st.needsReindex for the revert (no re-prompt loop).
    } catch (err) {
      if (mounted.current) setInitializing(false);
      console.error('[RAG] revert to previous model failed', err);
    }
  }, [prevModel, fetchModelLimits]);

  // Select a different model size: persist + auto-restart RAG (backend reloads
  // the model). If the new model's embed dim differs from the old table,
  // `status.needsReindex` comes back true and we trigger the reindex overlay
  // (same as a model swap). No-op if the size isn't ready.
  const selectModel = useCallback(
    async (size: string) => {
      const m = models.find((x) => x.size === size);
      if (!m || !m.ready) {
        showToast(t('pages.rag.modelNotReady'), 'error');
        return;
      }
      if (size === currentModel) return;
      setSwitchingModel(true);
      // Remember the model before the switch so a cancelled reindex confirm
      // can revert to it (no toast on the revert).
      setPrevModel(currentModel);
      try {
        const st = await selectRagModel(size);
        if (!mounted.current) return;
        setCurrentModel(size);
        setEnabled(st.enabled);
        setSwitchingModel(false);
        // The backend's stop+start may still be settling (e.g. async cleanup);
        // mirror its initializing flag so the "开启中" state shows if needed.
        setInitializing(st.initializing);
        if (st.enabled) fetchModelLimits();
        if (st.enabled && st.needsReindex) {
          // Dim changed - show the confirm dialog (no success toast: the
          // switch is pending the user's reindex decision; cancel reverts).
          // Skip the prompt when there are no documents (nothing to re-embed).
          if (ragDocs.length > 0) setReindexConfirm(true);
          else showToast(t('pages.rag.modelSelected', { name: `model_${size}` }), 'success');
          showToast(t('pages.rag.modelSelected', { name: `model_${size}` }), 'success');
        }
      } catch (err) {
        if (mounted.current) {
          setSwitchingModel(false);
          setPrevModel(null);
        }
        showToast(err instanceof Error ? err.message : t('pages.rag.modelSelectFailed'), 'error');
      }
    },
    [models, currentModel, ragDocs, fetchModelLimits, t, showToast],
  );

  // Download a not-yet-ready model size via its download.url. The backend
  // streams the .zip + extracts, emitting `rag://model-download` progress
  // (handled by the listener above). On success the listener refreshes the
  // model list (size becomes "ready").
  const downloadModel = useCallback(
    async (size: string) => {
      setModelDownload({ size, phase: 'downloading', downloaded: 0, total: 0, percent: 0, speed: 0, eta: 0, fileCurrent: 0, fileTotal: 0 });
      try {
        await downloadRagModel(size);
      } catch (err) {
        if (mounted.current) setModelDownload(null);
        showToast(err instanceof Error ? err.message : t('pages.rag.modelDownloadFailed'), 'error');
      }
    },
    [t, showToast],
  );

  // Toggle RAG on/off via the backend (blocks until ready on enable). Sets
  // `initializing` optimistically so the page grays out immediately.
  const toggleEnabled = useCallback(
    async (next: boolean) => {
      if (next === enabled) return;
      setInitializing(true);
      setTogglingTo(next ? 'on' : 'off');
      try {
        const st = await ragToggle(next);
        if (mounted.current) {
          setEnabled(st.enabled);
          setInitializing(st.initializing);
          setTogglingTo(null);
          // After a successful enable the model is freshly loaded from disk -
          // re-read its context window so the chunk_size max reflects the
          // CURRENT model (the bound can change if the model files were
          // swapped, e.g. for a different context length).
          if (st.enabled) fetchModelLimits();
          // If the model swap recreated the table (embedding dim changed),
          // prompt the user before re-embedding all docs (expensive) instead
          // of auto-running it. Skip when there are no docs (nothing to re-embed).
          if (st.enabled && st.needsReindex && ragDocs.length > 0) setReindexConfirm(true);
        }
      } catch (err) {
        // revert on error
        if (mounted.current) {
          setEnabled(false);
          setInitializing(false);
          setTogglingTo(null);
        }
        throw err;
      }
    },
    [enabled, ragDocs, fetchModelLimits],
  );

  const upload = useCallback(
    async (files: RagPickedFile[], tags: string[] = []) => {
      if (files.length === 0) return { success: 0, failed: 0 };
      setUploading(true);
      setUploadProgress({ current: 0, total: files.length, name: files[0].name });
      // Reset the per-document bar at the start of each batch; the backend
      // emits fresh events as each file is indexed.
      setCharProgress(null);
      let success = 0;
      let failed = 0;
      try {
        for (let i = 0; i < files.length; i++) {
          if (!mounted.current) break;
          setUploadProgress({ current: i, total: files.length, name: files[i].name });
          // Drop the previous file's char progress so the second bar restarts
          // from 0 for the next file (the backend pushes the new file's events
          // once its embedding begins).
          setCharProgress({ name: files[i].name, charsDone: 0, charsTotal: 0 });
          try {
            await uploadRagDoc(files[i].path, tags);
            success++;
          } catch (err) {
            failed++;
            if (!mounted.current) break;
            const msg = err instanceof Error ? err.message : String(err);
            if (msg.startsWith('UNSUPPORTED_FORMAT')) {
              showToast(t('pages.rag.unsupportedFile', { name: files[i].name }), 'error');
            } else {
              showToast(`${t('pages.rag.uploadFailedFile', { name: files[i].name })}: ${msg}`, 'error');
            }
            console.error('[RAG] upload failed for', files[i].name, err);
          }
        }
        setUploadProgress({ current: files.length, total: files.length, name: '' });
        await fetchDocs();
      } finally {
        if (mounted.current) {
          setUploading(false);
          setUploadProgress(null);
          setCharProgress(null);
        }
      }
      return { success, failed };
    },
    [fetchDocs],
  );

  const pickFiles = useCallback(async (): Promise<RagPickedFile[]> => {
    try {
      return await pickRagFiles();
    } catch {
      return [];
    }
  }, []);

  // Update an existing document in place: overwrite its content + meta (id
  // preserved, tags preserved) + re-embed its vectors. Drives the SAME
  // upload-progress overlay as `upload` (file-level 1/1 + the backend's
  // `rag://upload-progress` char-level events emitted from reindex_doc), so
  // the user sees both progress bars while the new content is re-embedded.
  // Caller picks the file (via pickFiles) and passes the path + display name.
  // Requires RAG enabled (re-embeds).
  const updateDoc = useCallback(
    async (id: string, filePath: string, name: string) => {
      setUploading(true);
      setUpdatingDoc(true);
      setUploadProgress({ current: 0, total: 1, name });
      setCharProgress({ name, charsDone: 0, charsTotal: 0 });
      try {
        await updateRagDoc(id, filePath);
        setUploadProgress({ current: 1, total: 1, name });
        await fetchDocs();
      } finally {
        setUploading(false);
        setUpdatingDoc(false);
        setUploadProgress(null);
        setCharProgress(null);
      }
    },
    [fetchDocs],
  );

  const remove = useCallback(
    async (id: string) => {
      await deleteRagDoc(id);
      await fetchDocs();
    },
    [fetchDocs],
  );

  // Batch delete: delete each doc by id, then refresh the list once. Each
  // delete also reclaims lancedb vectors on the backend (see delete_doc).
  const removeMany = useCallback(
    async (ids: string[]) => {
      for (const id of ids) {
        await deleteRagDoc(id);
      }
      await fetchDocs();
    },
    [fetchDocs],
  );

  const view = useCallback(async (id: string) => {
    setViewLoading(true);
    try {
      const doc = await getRagDoc(id);
      setViewedDoc(doc);
    } finally {
      setViewLoading(false);
    }
  }, []);

  const closeView = useCallback(() => setViewedDoc(null), []);

  // Fetch a document's chunks (for the "view chunks" dialog). RAG must be
  // enabled (chunks live in lancedb); the backend errors otherwise - we surface
  // it as a toast and don't open the dialog.
  const viewChunks = useCallback(async (doc: RagDocInfo) => {
    setChunksLoading(true);
    setChunksList([]);
    setChunksDoc(doc);
    try {
      const chunks = await getRagChunks(doc.id);
      if (mounted.current) setChunksList(chunks);
    } catch (e) {
      showToast(e instanceof Error ? e.message : 'Failed to load chunks', 'error');
      if (mounted.current) setChunksDoc(null);
    } finally {
      if (mounted.current) setChunksLoading(false);
    }
  }, [showToast]);

  const closeChunks = useCallback(() => {
    setChunksDoc(null);
    setChunksList([]);
  }, []);

  const openLocation = useCallback(async (id: string) => {
    await openRagFileLocation(id);
  }, []);

  const search = useCallback(async (query: string, tags: string[] = []) => {
    setSearching(true);
    try {
      const results = await searchRagDocs(query, tags);
      if (mounted.current) setSearchResults(results);
    } finally {
      if (mounted.current) setSearching(false);
    }
  }, []);

  const setTags = useCallback(
    async (id: string, tags: string[]) => {
      await setRagTags(id, tags);
      await fetchDocs();
    },
    [fetchDocs],
  );

  const updateSettings = useCallback(async (s: RagSettings) => {
    setSettings(s);
    await saveRagSettings(s);
  }, []);

  return {
    t,
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
    pickFiles,
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
    updateSettings,
    refresh: fetchDocs,
  };
};

/** Provider that mounts the shared RAG store once at the app root. */
export const RagDataProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const value = useRagDataState();
  return <RagDataContext.Provider value={value}>{children}</RagDataContext.Provider>;
};

/** Consume the shared RAG store. Must be used inside <RagDataProvider>. */
export const useRagData = () => {
  const ctx = useContext(RagDataContext);
  if (!ctx) throw new Error('useRagData must be used within a RagDataProvider');
  return ctx;
};

