import React, { createContext, useContext, useState, useCallback, useRef, useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useAuth } from '@/contexts/AuthContext';
import { useToast } from '@/contexts/ToastContext';
import { isTauri } from '@/utils/tauriClient';
import { BatchPreview, RagChunk, RagDoc, RagDocInfo, RagFolderScan, RagModelInfo, RagModelLimits, RagPickedFile, RagSettings, RagSearchResult, RagUpdateCheck } from '@/types';
import {
  listRagDocs,
  getRagSettings,
  getRagModelLimits,
  saveRagSettings,
  deleteRagDoc,
  updateRagDoc,
  getRagDocPaged,
  getRagChunksPaged,
  uploadRagDoc,
  pickRagFiles,
  pickRagFolder,
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
  checkRagUpdate,
  previewBatchUpdate,
  batchUpdateRagDocs,
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
  // 每次 fetchDocs 完成后自增。导入/删除/更新/批量更新等所有变更路径最终都会
  // 调 fetchDocs(刷新 ragDocs),但页面的真实列表走独立的后端分页查询
  // (RagPage 的 ragDocSearchPaged effect)——它无法感知 ragDocs 变化。这个
  // 版本号作为「数据已变」信号暴露出去,分页 effect 把它加入依赖即可联动刷新。
  const [docsVersion, setDocsVersion] = useState(0);
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
  const [settings, setSettings] = useState<RagSettings>({ vectorWeight: 0.9, keywordWeight: 0.1, maxResults: 20, scoreThreshold: 0.65, chunkSize: 0, chunkOverlap: 0, autoUpdateEnabled: true, autoUpdateIntervalSecs: 300, docLoadChunkKb: 200 });
  const [modelLimits, setModelLimits] = useState<RagModelLimits>({ maxContext: 2048 });
  const [viewedDoc, setViewedDoc] = useState<RagDoc | null>(null);
  const [viewLoading, setViewLoading] = useState(false);
  // True while a "load more" page of the viewed doc's content is fetching
  // (drives the spinner on the View dialog's load-more button).
  const [viewMoreLoading, setViewMoreLoading] = useState(false);
  // "View chunks" dialog: the doc whose chunks are shown + the loaded chunks +
  // a loading flag while fetching (RAG must be enabled to read lancedb).
  // Chunks load in pages (5 at a time); `chunksTotal` is the backend-reported
  // full count so the UI knows when to stop auto-loading on scroll.
  const [chunksDoc, setChunksDoc] = useState<RagDocInfo | null>(null);
  const [chunksList, setChunksList] = useState<RagChunk[]>([]);
  const [chunksTotal, setChunksTotal] = useState(0);
  const [chunksLoading, setChunksLoading] = useState(false);
  // True while the next chunks page is fetching (scroll-to-bottom auto-load).
  const [chunksMoreLoading, setChunksMoreLoading] = useState(false);
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
  // Batch-update progress: emitted by the backend's `rag://batch-update-progress`
  // event during the async re-index pass. `batchUpdateRunning` is true while the
  // pass is in flight (drives the header button's "查看进度" + spinner). The
  // progress dialog reads `batchProgress` + `charProgress` for the two bars.
  // phase: "checking" | "reindexing" | "done" | "error" (error = the batch task
  // itself failed early, e.g. files_dir/read_dir error; the dialog shows the
  // failure state instead of a green "complete" checkmark).
  const [batchUpdateRunning, setBatchUpdateRunning] = useState(false);
  const [batchProgress, setBatchProgress] = useState<{
    current: number;
    total: number;
    name: string;
    phase: 'checking' | 'reindexing' | 'done' | 'error';
  } | null>(null);
  const mounted = useRef(true);

  const fetchDocs = useCallback(async () => {
    try {
      const data = await listRagDocs();
      if (mounted.current) {
        setRagDocs(data);
        setDocsVersion((v) => v + 1);
      }
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
    let unlistenBatch: UnlistenFn | undefined;
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

    // Batch-update progress: the backend emits one event per doc (checking /
    // reindexing phases) + a final "done" (or "error" on early failure). Drives
    // the batch-progress dialog's file-level bar; the char-level sub-bar reuses
    // `rag://upload-progress` above (reindex_doc emits it). On either terminal
    // phase we clear the running flag + refetch the doc list (chunk counts /
    // version / md5 changed; on error a partial pass may still have changed
    // some docs).
    //
    // Any non-terminal event also SETS the running flag: the backend's timed
    // auto-update pass emits the same events without the frontend having
    // initiated it, and the header button must flip to "查看进度" for that
    // pass too (manual + auto share one state).
    listen<{ current: number; total: number; name: string; phase: string }>(
      'rag://batch-update-progress',
      (event) => {
        const p = event.payload;
        if (!p || !mounted.current) return;
        const phase =
          p.phase === 'checking' || p.phase === 'reindexing' || p.phase === 'done' || p.phase === 'error'
            ? (p.phase as 'checking' | 'reindexing' | 'done' | 'error')
            : 'checking';
        setBatchProgress({ current: p.current, total: p.total, name: p.name, phase });
        if (phase === 'done' || phase === 'error') {
          // Both terminal phases clear the running flag. On either terminal
          // phase we also refetch the doc list: "done" because chunk counts /
          // versions / md5 changed, "error" because a partially-applied pass
          // (some docs re-indexed before the failure) still changed state and
          // the list must not stay stale. The original code skipped the refetch
          // on error, which left the list out of sync after a failed batch.
          setBatchUpdateRunning(false);
          // Slight delay on "done" so the "完成" state is visible before the
          // dialog clears. On "error" refetch immediately (no success state to
          // hold the dialog open for). `mounted.current` guards the unmount
          // race; the fetch itself is best-effort (fetchDocs swallows).
          const delay = phase === 'done' ? 600 : 0;
          setTimeout(() => {
            if (mounted.current) fetchDocs();
          }, delay);
        } else {
          // An in-flight pass (manual OR the timed auto-update): the button
          // shows "查看进度" so the user can open the shared progress dialog.
          setBatchUpdateRunning(true);
        }
      },
    ).then((un) => {
      if (cancelled) un();
      else unlistenBatch = un;
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
      unlistenBatch?.();
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
    async (files: RagPickedFile[], tags: string[] = [], method: 'symlink' | 'copy' = 'symlink') => {
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
            await uploadRagDoc(files[i].path, tags, method);
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

  // Pick a folder: the backend opens the OS folder picker and scans the
  // folder — immediate file children only when `recursive` is false, all
  // descendant dirs when true (dev dirs / hidden entries / symlinks skipped,
  // 500-file cap). Returns a grouped result the Upload dialog renders as a
  // tree; multi-file picks are folded into the same shape by the page.
  const pickFolder = useCallback(async (recursive: boolean): Promise<RagFolderScan> => {
    try {
      return await pickRagFolder(recursive);
    } catch {
      return { root: '', skippedDirs: 0, skippedFiles: 0, groups: [], truncated: false };
    }
  }, []);

  // Update an existing document in place. `mode`:
  //   'original' -> re-read the recorded original_path + re-index (the "from
  //                original" button). filePath is ignored.
  //   'file'     -> read a freshly-picked filePath + overwrite (the "manual
  //                upload" button); it becomes the recorded original_path.
  // Drives the SAME upload-progress overlay as `upload` (file-level 1/1 + the
  // backend's `rag://upload-progress` char-level events emitted from
  // reindex_doc), so the user sees both progress bars. Requires RAG enabled.
  const updateDoc = useCallback(
    async (id: string, name: string, opts: { mode: 'original' | 'file'; filePath?: string }) => {
      setUploading(true);
      setUpdatingDoc(true);
      setUploadProgress({ current: 0, total: 1, name });
      setCharProgress({ name, charsDone: 0, charsTotal: 0 });
      try {
        await updateRagDoc(id, opts);
        setUploadProgress({ current: 1, total: 1, name });
      } finally {
        setUploading(false);
        setUpdatingDoc(false);
        setUploadProgress(null);
        setCharProgress(null);
        // Refresh in `finally` regardless of success: the backend re-index
        // (reindex_doc) returns once chunks are re-embedded, and even on error
        // the on-disk meta/version may have partially changed. Without this, a
        // thrown update (error swallowed into {success:false} by apiRequest ->
        // updateRagDoc re-throws) left the row showing the old version/md5,
        // which is the "file list doesn't update after single update" symptom.
        await fetchDocs();
      }
    },
    [fetchDocs],
  );

  // Single-doc update check: classify the recorded original's state so the
  // UpdateDialog renders the right branch. Cheap (one md5, no embedding).
  const checkUpdate = useCallback(async (id: string): Promise<RagUpdateCheck> => {
    return await checkRagUpdate(id);
  }, []);

  // Batch-update preview: aggregate counts for the confirm dialog. No embedding.
  const getBatchPreview = useCallback(async (): Promise<BatchPreview> => {
    return await previewBatchUpdate();
  }, []);

  // Start the background batch update. Returns immediately; progress arrives
  // via `rag://batch-update-progress` (consumed by the listener below). The
  // header button flips to "查看进度" + spinner while `batchUpdateRunning` is
  // true; re-clicking just re-opens the progress dialog (no re-trigger — the
  // backend CAS-guards, and the frontend short-circuits).
  const batchUpdate = useCallback(async () => {
    if (batchUpdateRunning) return;
    setBatchUpdateRunning(true);
    setBatchProgress({ current: 0, total: 0, name: '', phase: 'checking' });
    try {
      await batchUpdateRagDocs();
    } catch (err) {
      if (mounted.current) setBatchUpdateRunning(false);
      throw err;
    }
  }, [batchUpdateRunning]);

  const remove = useCallback(
    async (id: string) => {
      // Refresh in `finally` regardless of success: the backend delete_doc
      // returns once the doc's vectors are removed (the slow lancedb Prune now
      // runs deferred in the background), so even on error the on-disk state
      // may have partially changed and the list should be re-read. Without this
      // a thrown delete (error swallowed into {success:false} by apiRequest ->
      // deleteRagDoc re-throws) would skip fetchDocs and the row would linger.
      try {
        await deleteRagDoc(id);
      } finally {
        await fetchDocs();
      }
    },
    [fetchDocs],
  );

  // Batch delete: delete each doc by id, then refresh the list once. Each
  // delete also reclaims lancedb vectors on the backend (the Prune is deduped
  // + deferred, so a 50-doc burst prunes once, not 50× — see delete_doc).
  //
  // Tolerates per-item failures: one bad id (e.g. meta already gone, or a
  // transient lock error) must NOT abort the remaining deletes AND skip the
  // final refresh (which left the list stale — the original symptom). Each
  // failure is collected + re-thrown AFTER the refresh so the caller can still
  // surface an error toast, but the list always reflects what actually got
  // deleted.
  const removeMany = useCallback(
    async (ids: string[]) => {
      const failures: string[] = [];
      try {
        for (const id of ids) {
          try {
            await deleteRagDoc(id);
          } catch (e) {
            failures.push(`${id}: ${e instanceof Error ? e.message : String(e)}`);
          }
        }
      } finally {
        // Always refresh: even if every delete threw, a prior partial delete
        // (the IPC returned {success:false} but the backend had already
        // removed the .meta) means the list is stale.
        await fetchDocs();
      }
      if (failures.length > 0) {
        throw new Error(`Failed to delete ${failures.length} of ${ids.length} doc(s): ${failures.join('; ')}`);
      }
    },
    [fetchDocs],
  );

  // Open the View dialog with the FIRST page of the doc's content (paged by
  // the user's docLoadChunkKb setting — limitBytes 0 lets the backend apply
  // it). Further pages come from `loadMoreView` until truncated=false.
  const view = useCallback(async (id: string) => {
    setViewLoading(true);
    try {
      const doc = await getRagDocPaged(id, 0, 0);
      setViewedDoc(doc);
    } finally {
      setViewLoading(false);
    }
  }, []);

  // Fetch the next page of the viewed doc's content (the "load more" button):
  // reads from the doc's nextOffset (end of what's loaded so far) and APPENDS
  // to the loaded content. A no-op when nothing is viewed / already complete.
  // `viewFetchingRef` is a synchronous lock (refs update immediately, unlike
  // state) so rapid double-clicks / stacked scroll events can't fire two
  // fetches for the same page before the re-render lands.
  const viewFetchingRef = useRef(false);
  const loadMoreView = useCallback(async () => {
    const doc = viewedDoc;
    if (!doc || !doc.truncated) return;
    if (viewFetchingRef.current) return;
    viewFetchingRef.current = true;
    setViewMoreLoading(true);
    try {
      const next = await getRagDocPaged(doc.id, doc.nextOffset ?? 0, 0);
      if (next && mounted.current) {
        setViewedDoc((prev) =>
          prev && prev.id === next.id
            ? {
                ...next,
                // Append: the page's content is the NEXT slice, not a
                // replacement. Everything else (truncated/nextOffset/totals)
                // comes fresh from this response.
                content: prev.content + next.content,
                tags: prev.tags,
              }
            : prev,
        );
      }
    } finally {
      viewFetchingRef.current = false;
      if (mounted.current) setViewMoreLoading(false);
    }
  }, [viewedDoc]);

  const closeView = useCallback(() => setViewedDoc(null), []);

  // Fetch a document's chunks (for the "view chunks" dialog) — first page
  // only (5 chunks). RAG must be enabled (chunks live in lancedb); the
  // backend errors otherwise - we surface it as a toast and don't open the
  // dialog. Further pages auto-load on scroll via `loadMoreChunks`.
  const CHUNKS_PAGE_SIZE = 5;
  const viewChunks = useCallback(
    async (doc: RagDocInfo) => {
      setChunksLoading(true);
      setChunksList([]);
      setChunksTotal(0);
      setChunksDoc(doc);
      try {
        const page = await getRagChunksPaged(doc.id, 0, CHUNKS_PAGE_SIZE);
        if (mounted.current) {
          setChunksList(page.items);
          setChunksTotal(page.total);
        }
      } catch (e) {
        showToast(e instanceof Error ? e.message : 'Failed to load chunks', 'error');
        if (mounted.current) setChunksDoc(null);
      } finally {
        if (mounted.current) setChunksLoading(false);
      }
    },
    [showToast],
  );

  // Fetch the next chunks page and append (scroll-to-bottom auto-load). No-op
  // when everything is loaded or a fetch is already in flight. `chunksFetchingRef`
  // is the synchronous lock (scroll events can stack within one render tick -
  // the state flag alone would let two handlers fetch the same page twice).
  const chunksFetchingRef = useRef(false);
  const loadMoreChunks = useCallback(async () => {
    const doc = chunksDoc;
    if (!doc || chunksLoading) return;
    if (chunksList.length >= chunksTotal) return;
    if (chunksFetchingRef.current) return;
    chunksFetchingRef.current = true;
    setChunksMoreLoading(true);
    try {
      const page = await getRagChunksPaged(doc.id, chunksList.length, CHUNKS_PAGE_SIZE);
      if (mounted.current) {
        setChunksList((prev) => [...prev, ...page.items]);
        setChunksTotal(page.total);
      }
    } catch (e) {
      showToast(e instanceof Error ? e.message : 'Failed to load chunks', 'error');
    } finally {
      chunksFetchingRef.current = false;
      if (mounted.current) setChunksMoreLoading(false);
    }
  }, [chunksDoc, chunksList, chunksTotal, chunksLoading, showToast]);

  const closeChunks = useCallback(() => {
    setChunksDoc(null);
    setChunksList([]);
    setChunksTotal(0);
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
    docsVersion,
    enabled,
    initializing,
    togglingTo,
    switchingModel,
    settings,
    modelLimits,
    viewedDoc,
    viewLoading,
    loadMoreView,
    viewMoreLoading,
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
    pickFolder,
    updateDoc,
    remove,
    removeMany,
    view,
    closeView,
    chunksDoc,
    chunksList,
    chunksTotal,
    chunksLoading,
    chunksMoreLoading,
    viewChunks,
    loadMoreChunks,
    closeChunks,
    openLocation,
    search,
    setTags,
    updateSettings,
    // Import-method feature: single-doc update check + batch update.
    checkUpdate,
    getBatchPreview,
    batchUpdate,
    batchUpdateRunning,
    batchProgress,
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

