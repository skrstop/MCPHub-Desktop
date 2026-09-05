import React, { useState, useMemo, useEffect, useRef, useCallback } from 'react';
import { createPortal } from 'react-dom';
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
  CheckCircle2,
  ChevronDown,
  Code,
  Plus,
  Minus,
  RefreshCw,
  Link2,
  Copy as CopyIcon,
  HelpCircle,
  AlertTriangle,
} from 'lucide-react';
import { Switch } from '@/components/ui/ToggleGroup';
import Pagination from '@/components/ui/Pagination';
import FileTypeRenderer from '@/components/ui/FileTypeRenderer';
import { isExtractedSource } from '@/utils/fileType';
import { useToast } from '@/contexts/ToastContext';
import { useRagData } from '@/hooks/useRagData';
import { getRagTools, ragDocSearchPaged, ragTagSearchPaged, getOcrStatus } from '@/services/ragService';
import { RagDoc, RagDocInfo, RagModelInfo, RagFolderScan, RagScanFile, RagPickedFile, RagSettings, RagTagStat, RagUpdateCheck, RagOcrStatus, BatchPreview } from '@/types';

const formatSize = (bytes: number): string => {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
};

// UTF-8 byte length of a string (matches the backend's byte-offset paging, so
// the "已加载 X / 全部 Y" hint stays consistent with the server's totals).
const utf8Bytes = (s: string): number => new TextEncoder().encode(s).length;

// ─── 导入方式（软链接 / 文件拷贝）—— 已接真实后端 ───
// 阶段 0 UI 预览已完成并定稿，mock 开关置 false 后所有交互走真实后端。
// 详见 doc/rag_import_method_20260821.md。保留 `MockDocInfo`/`MockUpdateCheck`/
// `BatchProgress` 类型别名以复用阶段 0 的 dialog 组件签名（这些类型与真实后端
// 类型 RagDocInfo/RagUpdateCheck 一致或为其超集）。
const MOCK_IMPORT_METHOD: boolean = false;

type MockMethod = 'symlink' | 'copy';

// 真实 RagDocInfo 已含 method/originalPath/md5/lostOriginal（types/index.ts）。
// 这里仅保留一个供 dialog 组件签名使用的类型别名（阶段 0 遗留，无运行时影响）。
type MockDocInfo = RagDocInfo;

// 真实 RagUpdateCheck 已含 lostOriginal；保留别名供 UpdateDialog 复用。
type MockUpdateCheck = RagUpdateCheck;

type BatchProgress = {
  current: number;
  total: number;
  name: string;
  phase: 'checking' | 'reindexing' | 'done' | 'error';
};

// 假数据仅在 MOCK_IMPORT_METHOD=true（UI 预览）时使用；接后端后置 false，
// 真实数据走 useRagData().ragDocs。保留常量以便后续再开预览，不再渲染。
const MOCK_DOCS: MockDocInfo[] = [];

// 导入方式帮助按钮（悬浮/点击 popover 解释软链接 vs 文件拷贝），仿 SkillsPage MethodHelpIcon。
const RagMethodHelpIcon: React.FC = () => {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const closeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const cancelClose = () => { if (closeTimer.current) { clearTimeout(closeTimer.current); closeTimer.current = null; } };
  const scheduleClose = () => { cancelClose(); closeTimer.current = setTimeout(() => setOpen(false), 150); };
  const handleEnter = () => { cancelClose(); setOpen(true); };
  const handleLeave = () => { scheduleClose(); };
  const handleClick = (e: React.MouseEvent) => { e.preventDefault(); e.stopPropagation(); cancelClose(); setOpen(true); };
  useEffect(() => () => cancelClose(), []);
  return (
    <span className="relative inline-flex">
      <button type="button" onClick={handleClick} onMouseEnter={handleEnter} onMouseLeave={handleLeave}
        className="hub-icon-btn sm" title={t('pages.rag.importMethodHelp', '查看导入方式说明')} style={{ color: 'var(--hub-ink-3)' }}>
        <HelpCircle size={13} />
      </button>
      {open && (
        <div className="absolute z-50 left-0 top-full mt-1 w-[300px] text-[12px] space-y-1.5 shadow-lg"
          style={{ padding: '10px 12px', background: 'var(--hub-surface)', border: '1px solid var(--hub-line)', borderRadius: 8, color: 'var(--hub-ink-2)' }}
          onMouseEnter={handleEnter} onMouseLeave={handleLeave}>
          <div className="flex items-start gap-1.5">
            <Link2 size={12} className="mt-0.5 flex-shrink-0" style={{ color: 'var(--hub-accent)' }} />
            <div><span className="font-medium" style={{ color: 'var(--hub-ink)' }}>{t('pages.rag.symlink', '软链接')}:</span> {t('pages.rag.symlinkHelp', '只记录文件原始地址，不拷贝实体文件；原始文件修改后可通过更新同步向量，删除不会动原始文件。')}</div>
          </div>
          <div className="flex items-start gap-1.5">
            <CopyIcon size={12} className="mt-0.5 flex-shrink-0" style={{ color: 'var(--hub-accent)' }} />
            <div><span className="font-medium" style={{ color: 'var(--hub-ink)' }}>{t('pages.rag.fileCopy', '文件拷贝')}:</span> {t('pages.rag.fileCopyHelp', '将文件复制一份到 RAG 目录，修改原始文件不影响已导入内容；删除会同时删除拷贝。')}</div>
          </div>
        </div>
      )}
    </span>
  );
};

// OCR 引擎缺失弹框：Linux 无 tesseract（或语言包不全）时，导入图片/PDF/
// Office 的 OCR 环节会带 `OCR_MISSING:` 哨兵失败——此处按平台/发行版展示
// 具体安装命令（等宽块 + 一键复制）。macOS/Windows 理论不触发（系统内置），
// 兜底文案直接说明引擎不可用。
const OcrMissingDialog: React.FC<{
  status: RagOcrStatus;
  onClose: () => void;
}> = ({ status, onClose }) => {
  const { t } = useTranslation();
  const { showToast } = useToast();
  const distro = (status.distro || '').toLowerCase();
  const installCmd = distro.includes('ubuntu') || distro.includes('debian') || distro.includes('apt')
    ? t('pages.rag.ocrInstallApt')
    : distro.includes('fedora') || distro.includes('rhel') || distro.includes('centos') || distro.includes('dnf')
      ? t('pages.rag.ocrInstallDnf')
      : distro.includes('arch') || distro.includes('manjaro') || distro.includes('pacman')
        ? t('pages.rag.ocrInstallPacman')
        : t('pages.rag.ocrInstallOther');
  const [copied, setCopied] = useState(false);
  const copyCmd = async () => {
    try {
      await navigator.clipboard.writeText(installCmd);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      showToast(t('pages.rag.ocrInstallCopyFailed', '复制失败'), 'error');
    }
  };
  const missing = status.missingLangs.length > 0
    ? t('pages.rag.ocrMissingLangs', { langs: status.missingLangs.join(', ') })
    : null;
  return (
    <div className="fixed inset-0 bg-black/50 z-[70] flex items-center justify-center p-4">
      <div className="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-md w-full mx-4 border border-gray-100 dark:border-gray-700 max-h-[90vh] flex flex-col">
        <div className="flex items-center justify-between p-5 border-b border-[var(--hub-line-2)]">
          <h2 className="text-xl font-bold text-gray-900 dark:text-gray-100">
            {t('pages.rag.ocrMissingTitle', 'OCR 引擎不可用')}
          </h2>
          <button onClick={onClose} className="hub-icon-btn sm">
            <X size={16} />
          </button>
        </div>
        <div className="flex-1 overflow-y-auto p-5 space-y-3">
          <p className="text-[13px]" style={{ color: 'var(--hub-ink-2)' }}>
            {t('pages.rag.ocrMissingBody', { platform: status.platform || 'linux' })}
          </p>
          {missing && (
            <p className="text-[13px]" style={{ color: 'var(--hub-ink-2)' }}>{missing}</p>
          )}
          {/* 等宽安装命令块 + 一键复制（Linux 按发行版择一展示） */}
          <div className="relative group">
            <pre
              className="hub-mono text-[12px] overflow-x-auto"
              style={{ padding: '10px 12px', background: 'var(--hub-bg-2)', border: '1px solid var(--hub-line)', borderRadius: 8, color: 'var(--hub-ink)', whiteSpace: 'pre-wrap', wordBreak: 'break-all' }}
            >
              {installCmd}
            </pre>
            <button
              onClick={copyCmd}
              className="hub-btn sm absolute top-1.5 right-1.5"
              title={t('pages.rag.ocrInstallCopy', '复制命令')}
            >
              {copied ? <Check size={12} /> : <CopyIcon size={12} />}
            </button>
          </div>
          <p className="text-[12px]" style={{ color: 'var(--hub-ink-3)' }}>
            {t('pages.rag.ocrInstallHint', '安装完成后重新导入即可。')}
          </p>
        </div>
        <div className="flex items-center justify-end p-5 border-t border-[var(--hub-line-2)] shrink-0">
          <button onClick={onClose} className="hub-btn primary">{t('common.ok', '知道了')}</button>
        </div>
      </div>
    </div>
  );
};

const RagPage: React.FC = () => {  const { t } = useTranslation();
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
    openSourceFile,
    search,
    setTags,
    pickFiles,
    pickFolder,
    updateSettings,
    checkUpdate,
    getBatchPreview,
    batchUpdate,
    batchUpdateRunning,
    batchProgress,
    ocrMissing,
    dismissOcrMissing,
    reportOcrMissingIf,
    docsVersion,
  } = useRagData();

  const [showUpload, setShowUpload] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [showVectorSearch, setShowVectorSearch] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<RagDocInfo | null>(null);
  // ── 文件选择状态:分组结构(文件夹扫描或多选文件统一折叠为一组) ──
  // scan.groups 按子文件夹分组;scan.selectedPaths 是勾选集(默认全选)。
  // 上传时只取勾选的文件,flat 映射回 RagPickedFile 走原有 upload 管线。
  const [scan, setScan] = useState<RagFolderScan | null>(null);
  // 勾选状态用 Set<string>(文件绝对路径)表示,null = 未初始化(扫描后默认全选)。
  const [selectedPaths, setSelectedPaths] = useState<Set<string> | null>(null);  // 「包含子文件夹」递归开关(默认关 = 现状行为),扫描「选择文件夹」时读取。
  const [folderRecursive, setFolderRecursive] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [uploadTags, setUploadTags] = useState<string[]>([]);
  const [fileNameSearch, setFileNameSearch] = useState('');
  const [selectedTagFilter, setSelectedTagFilter] = useState<Set<string>>(new Set());
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [showBatchTags, setShowBatchTags] = useState(false);
  const [batchMode, setBatchMode] = useState<'add' | 'remove'>('add');
  const [showTools, setShowTools] = useState(false);
  const [showBatchDelete, setShowBatchDelete] = useState(false);
  const [batchDeleting, setBatchDeleting] = useState(false);

  // ── 导入方式 state（接真实后端） ──
  const [uploadMethod, setUploadMethod] = useState<MockMethod>('symlink'); // 默认软链接（需求1）
  const [updateTarget, setUpdateTarget] = useState<MockDocInfo | null>(null); // 单条更新弹框目标
  const [updateCheck, setUpdateCheck] = useState<MockUpdateCheck | null>(null); // 检查原始文件结果
  const [updateChecking, setUpdateChecking] = useState(false); // 检查中
  const [updateActionPhase, setUpdateActionPhase] = useState<'idle' | 'reindexing' | 'done' | 'error'>('idle'); // 执行更新阶段
  const [showBatchUpdateDialog, setShowBatchUpdateDialog] = useState(false); // 进度弹框（可关闭）
  const [batchConfirm, setBatchConfirm] = useState<BatchPreview | null>(null); // 批量更新前置确认弹框
  const [batchConfirmChecking, setBatchConfirmChecking] = useState(false); // 确认弹框「预扫描」中

  // mock 列表：开启 mock 时用 MOCK_DOCS，否则用真实 ragDocs。
  const docs: MockDocInfo[] = MOCK_IMPORT_METHOD ? MOCK_DOCS : (ragDocs as MockDocInfo[]);

  // 批量更新运行状态来自 useRagData（监听 rag://batch-update-progress 事件）。
  const batchProgressTyped = batchProgress as unknown as BatchProgress | null;

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

  // ── 选中操作(分组树,无 checkbox:点行选中/取消) ──
  // null 哨兵(= 全选)物化为具体集合;所有增删都在物化后的集合上进行。
  // updater 内必须用传入的 prev 物化,不能用闭包里的 selectedPaths
  // (那是渲染时旧值,连续点击会互相覆盖)。
  const resolveSelected = (prev: Set<string> | null): Set<string> => {
    if (prev) return prev;
    if (!scan) return new Set();
    return new Set(scan.groups.flatMap((g) => g.files.map((f) => f.path)));
  };
  // 当前选中集(selectedPaths 为 null 时 = 全选,即扫描结果中所有文件)。
  const currentSelected = (): Set<string> => resolveSelected(selectedPaths);
  // 单个文件选中/取消。
  const toggleFileSelected = (path: string) => {
    setSelectedPaths((prev) => {
      const next = resolveSelected(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };
  // 整组切换:组内全选中 -> 清空;否则全选。
  const toggleGroupSelected = (files: RagScanFile[]) => {
    setSelectedPaths((prev) => {
      const next = resolveSelected(prev);
      const all = files.every((f) => next.has(f.path));
      if (all) files.forEach((f) => next.delete(f.path));
      else files.forEach((f) => next.add(f.path));
      return next;
    });
  };
  // 清除整组已选(部分选中状态下的组级「清除 M」)。
  const clearGroupSelected = (files: RagScanFile[]) => {
    setSelectedPaths((prev) => {
      const next = resolveSelected(prev);
      files.forEach((f) => next.delete(f.path));
      return next;
    });
  };
  // 全部选中 / 全部清除(底栏全局按钮)。「清除」必须物化为空集,
  // 不能保留 null 哨兵,否则 UI 仍显示全选。
  const setAllSelected = (on: boolean) => {
    setSelectedPaths(
      on && scan ? new Set(scan.groups.flatMap((g) => g.files.map((f) => f.path))) : new Set<string>()
    );
  };

  // 多选文件:折叠成单组(根)分组结构,与文件夹扫描共用一套渲染/上传。
  const handlePick = async () => {
    const picked = await pickFiles();
    if (picked.length === 0) return;
    mergeIntoScan({
      root: '',
      skippedDirs: 0,
      skippedFiles: 0,
      truncated: false,
      groups: [{ relPath: '', files: picked.map((p) => ({ ...p, size: 0 })) }],
    });
  };

  // Pick a folder: backend scans per the recursive switch (flat = immediate
  // children as one root group — the pre-existing behavior; recursive = all
  // descendant dirs with folder_ignore.json + hidden + symlink skipping, cap
  // 500). Result replaces the tree (a folder pick is a fresh scan, not a
  // merge — mixing two roots' relative paths would be misleading).
  const handlePickFolder = async () => {
    setScanning(true);
    try {
      const result = await pickFolder(folderRecursive);
      if (result.groups.length === 0) return;
      setScan(result);
      setSelectedPaths(null); // 默认全选
    } finally {
      setScanning(false);
    }
  };

  // 把一次多选文件合并进现有树:按 path 去重。文件夹扫描是 replace
  // (handlePickFolder 里直接 setScan),只有多选文件走 merge(用户可以
  // 「选择文件」多次累加);合并保留原有 root/统计,skipped* 只累加新值。
  const mergeIntoScan = (next: RagFolderScan) => {
    setScan((prev) => {
      if (!prev || prev.groups.length === 0) return next;
      const existing = new Set(prev.groups.flatMap((g) => g.files.map((f) => f.path)));
      const merged = [...prev.groups];
      for (const g of next.groups) {
        const idx = merged.findIndex((m) => m.relPath === g.relPath);
        const fresh = g.files.filter((f) => !existing.has(f.path));
        if (fresh.length === 0) continue;
        if (idx >= 0) merged[idx] = { ...merged[idx], files: [...merged[idx].files, ...fresh] };
        else merged.push({ ...g, files: fresh });
      }
      return {
        ...prev,
        skippedFiles: prev.skippedFiles + next.skippedFiles,
        groups: merged,
      };
    });
    // 新合并进来的文件默认勾选(不改变已有勾选)。
    setSelectedPaths((prev) => {
      const base = prev ?? new Set<string>();
      const nextSet = new Set(base);
      for (const g of next.groups) for (const f of g.files) nextSet.add(f.path);
      return nextSet;
    });
  };

  // 选中文件的扁平列表(上传管线输入,按组序 + 组内排序保持展示顺序)。
  const selectedPickedFiles: RagPickedFile[] = useMemo(() => {
    const sel = currentSelected();
    if (!scan) return [];
    return scan.groups
      .flatMap((g) => g.files)
      .filter((f) => sel.has(f.path))
      .map((f) => ({ path: f.path, name: f.name }));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scan, selectedPaths]);

  const removeFile = (path: string) => {
    setScan((prev) => {
      if (!prev) return prev;
      const groups = prev.groups
        .map((g) => ({ ...g, files: g.files.filter((f) => f.path !== path) }))
        .filter((g) => g.files.length > 0);
      return { ...prev, groups };
    });
    setSelectedPaths((prev) => {
      if (!prev) return prev;
      const next = new Set(prev);
      next.delete(path);
      return next;
    });
  };

  // 重置选择状态(关闭对话框 / 完成导入时)。
  const resetPickState = () => {
    setScan(null);
    setSelectedPaths(null);
  };

  const handleUploadConfirm = async () => {
    if (selectedPickedFiles.length === 0) {
      showToast(t('pages.rag.noFileSelected'), 'error');
      return;
    }
    const { success, failed } = await upload(selectedPickedFiles, uploadTags, uploadMethod);
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
    resetPickState();
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
    // Use the full hook list (`ragDocs`) so batch ops work across pages —
    // selectedIds can contain docs from pages other than the current one.
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

  const handleOpenFolder = async (doc: RagDocInfo, target?: 'source' | 'content') => {
    // 软链接/拷贝打开地址，原始丢失时后端会报错（open_file_location 对 symlink
    // 找不到原始文件时返回 Err）。这里只兜底提示；真实打开由后端 reveal 处理。
    // target="source" 显式打开原始文件位置（二次解析类文档的两个选项之一）。
    try {
      await openLocation(doc.id, target);
    } catch (err) {
      showToast(err instanceof Error ? err.message : t('pages.rag.openFolderNotReady'), 'error');
    }
  };

  // 二次解析类文档（PDF/Office/图片）的「打开文件夹」两选项菜单：
  // 源文件（original_path）/ 解析后的文件（rag/files 提取 Markdown）。
  const [folderMenuId, setFolderMenuId] = useState<string | null>(null);
  const handleFolderClick = (doc: RagDocInfo) => {
    const hasSource = !!doc.originalPath && doc.lostOriginal !== true;
    if (isExtractedSource(doc.name) && hasSource) {
      setFolderMenuId((cur) => (cur === doc.id ? null : doc.id));
      return;
    }
    // 非二次解析 / 无可用源文件：保持原行为（打开内容文件位置）。
    void handleOpenFolder(doc);
  };
  const handleViewSourceFile = async (doc: { id: string }) => {
    setFolderMenuId(null);
    try {
      await openSourceFile(doc.id);
    } catch (err) {
      showToast(err instanceof Error ? err.message : t('pages.rag.openFolderNotReady'), 'error');
    }
  };

  // 查看按钮：走真实 getRagDoc（useRagData.view）。contentAvailable=false 的文档
  // 按钮已置灰（symlink 原始丢失 / copy 拷贝被外部删除），此处兜底保护。
  const handleView = (doc: RagDocInfo) => {
    if (doc.contentAvailable === false) {
      showToast(t('pages.rag.contentUnavailableView', '无法读取文档内容'), 'error');
      return;
    }
    view(doc.id);
  };

  // 单条「更新」：弹出 UpdateDialog（先检查原始文件，再按分支执行），不再直接弹 OS picker。
  const [updatingId, setUpdatingId] = useState<string | null>(null);
  const handleUpdate = (doc: RagDocInfo) => {
    setUpdateTarget(doc as MockDocInfo);
    setUpdateCheck(null);
    setUpdateChecking(false);
    setUpdateActionPhase('idle');
    // 自动触发「检查原始文件」
    void runUpdateCheck(doc.id);
  };

  // 「检查原始文件」：调后端 check_rag_update（读 meta + md5 比对源文件）。
  const runUpdateCheck = async (id: string) => {
    setUpdateChecking(true);
    setUpdateCheck(null);
    try {
      const result = await checkUpdate(id);
      setUpdateCheck(result);
    } catch (err) {
      showToast(err instanceof Error ? err.message : t('pages.rag.updateFailed', '更新文档失败'), 'error');
    } finally {
      setUpdateChecking(false);
    }
  };

  // 执行更新（从原始 / 手动上传）：调后端 update_rag_doc，复用上传进度浮层。
  const runUpdateAction = async (doc: MockDocInfo, mode: 'original' | 'file') => {
    setUpdateActionPhase('reindexing');
    setUpdatingId(doc.id);
    try {
      if (mode === 'original') {
        await updateDoc(doc.id, doc.name, { mode: 'original' });
      } else {
        const picked = await pickFiles();
        if (picked.length === 0) {
          setUpdateActionPhase('idle');
          setUpdatingId(null);
          return;
        }
        await updateDoc(doc.id, picked[0].name, { mode: 'file', filePath: picked[0].path });
      }
      setUpdateActionPhase('done');
      showToast(t('pages.rag.updateDone', '文档已更新'), 'success');
    } catch (err) {
      setUpdateActionPhase('error');
      // OCR 缺失（更新图片类文档时引擎不可用）→ 安装命令弹框，而非 toast。
      if (await reportOcrMissingIf(err)) {
        setUpdatingId(null);
        return;
      }
      showToast(err instanceof Error ? err.message : t('pages.rag.updateFailed', '更新文档失败'), 'error');
    } finally {
      setUpdatingId(null);
    }
  };

  // 列表右上「批量文件更新」：先调后端 preview_batch_update 弹确认框（展示统计），
  // 用户确认后再调 batch_update_rag_docs 启动后台任务。
  const handleBatchUpdate = () => {
    // 已 running 时再点 → 只重开进度弹框，不重启任务（需求6）
    if (batchUpdateRunning) {
      setShowBatchUpdateDialog(true);
      return;
    }
    setBatchConfirmChecking(true);
    setBatchConfirm(null);
    void (async () => {
      try {
        const preview = await getBatchPreview();
        setBatchConfirm(preview);
      } catch (err) {
        showToast(err instanceof Error ? err.message : t('pages.rag.batchUpdate', '批量更新'), 'error');
      } finally {
        setBatchConfirmChecking(false);
      }
    })();
  };

  // 用户在确认框点「开始更新」后真正启动后台任务。
  const startBatchUpdate = async () => {
    setBatchConfirm(null);
    setShowBatchUpdateDialog(true);
    try {
      await batchUpdate();
    } catch (err) {
      setShowBatchUpdateDialog(false);
      showToast(err instanceof Error ? err.message : t('pages.rag.batchUpdate', '批量更新'), 'error');
    }
  };

  // ── 文件列表后端分页 ──
  // 列表始终走后端 SQL 分页（`ragDocSearchPaged`：name LIKE + 标签 ANY 过滤，
  // uploaded_at DESC，LIMIT/OFFSET，total 同步返回）。防抖 250ms + reqId 竞态守卫
  // （同标签下拉模式）。搜索词/标签/页码/每页数任一变化都重取当前页。后端 page 0-based，
  // UI page 1-based，调用时传 `page - 1`。
  const [pageDocs, setPageDocs] = useState<RagDocInfo[]>([]);
  const [pageTotal, setPageTotal] = useState(0);
  const [pageLoading, setPageLoading] = useState(false);
  const [docPage, setDocPage] = useState(1);
  const [docPageSize, setDocPageSize] = useState(10);
  const docReqId = useRef(0);

  const searchKey = fileNameSearch.trim();
  // 标签集合的稳定序列化（排序后拼接），用作 effect 依赖以监听标签变化。
  const searchTagsKey = useMemo(
    () => [...selectedTagFilter].sort().join(''),
    [selectedTagFilter],
  );

  useEffect(() => {
    const id = ++docReqId.current;
    setPageLoading(true);
    const timer = setTimeout(async () => {
      try {
        const res = await ragDocSearchPaged(
          searchKey,
          selectedTagFilter.size > 0 ? [...selectedTagFilter] : [],
          docPage - 1,
          docPageSize,
        );
        if (id !== docReqId.current) return;
        setPageDocs(res.items);
        setPageTotal(res.total);
      } catch {
        if (id !== docReqId.current) return;
        setPageDocs([]);
        setPageTotal(0);
      } finally {
        if (id === docReqId.current) setPageLoading(false);
      }
    }, 250);
    return () => clearTimeout(timer);
    // docsVersion:导入/删除/更新/批量更新完成后 fetchDocs 会自增它,这里
    // 联动重取当前页(否则变更后的列表停留旧数据,需手动刷新)。requestId
    // 竞态守卫保证乱序响应不会覆盖较新的结果。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchKey, searchTagsKey, docPage, docPageSize, docsVersion]);

  // 过滤条件变化 → 回到第一页（避免停在越界的页码）。每页数变化单独处理。
  useEffect(() => {
    setDocPage(1);
  }, [searchKey, searchTagsKey]);
  const handleDocPageSizeChange = (next: number) => {
    setDocPageSize(next);
    setDocPage(1);
  };

  // mock 模式用客户端过滤，真实模式用后端分页结果。
  const filteredDocs = useMemo(() => {
    const q = fileNameSearch.trim().toLowerCase();
    const tags = selectedTagFilter;
    return docs.filter((d) => {
      const nameOk = !q || d.name.toLowerCase().includes(q);
      const tagOk = tags.size === 0 || (d.tags || []).some((t) => tags.has(t));
      return nameOk && tagOk;
    });
  }, [docs, fileNameSearch, selectedTagFilter]);
  const visibleDocs: RagDocInfo[] = MOCK_IMPORT_METHOD ? (filteredDocs as RagDocInfo[]) : pageDocs;

  const totalPages = Math.max(1, Math.ceil(pageTotal / docPageSize));
  const safePage = Math.min(docPage, totalPages);
  useEffect(() => {
    if (safePage !== docPage) setDocPage(safePage);
  }, [safePage, docPage]);
  const docPagination = {
    page: safePage,
    limit: docPageSize,
    total: pageTotal,
    totalPages,
  };

  // Paths already imported — uploads record `originalPath`, so a file already
  // in the library (by path, not name) is flagged in the upload dialog. Name
  // collisions are NOT flagged: uploads never overwrite (fresh uuid per file),
  // same-named docs coexist as separate documents. Uses the full hook list
  // (`ragDocs`) so the flag works across pages, not just the current page.
  const existingPaths = useMemo(
    () => new Set(ragDocs.map((d) => (d as MockDocInfo).originalPath || '').filter(Boolean)),
    [ragDocs],
  );

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
          {/* 批量文件更新按钮（需求6）——运行时文字变「查看进度」+ loading，点它开/重开进度弹框 */}
          <button
            onClick={() => (batchUpdateRunning ? setShowBatchUpdateDialog(true) : handleBatchUpdate())}
            className="hub-btn primary"
            disabled={disabled}
            title={
              batchUpdateRunning
                ? t('pages.rag.batchViewProgressHint', '查看批量更新进度')
                : t('pages.rag.batchUpdateHint', '扫描全部文档，对有更新的自动从原始文件重新索引')
            }
          >
            {batchUpdateRunning ? <Loader2 size={13} className="animate-spin" /> : <RefreshCw size={13} />}
            {batchUpdateRunning ? t('pages.rag.batchViewProgress', '查看进度') : t('pages.rag.batchUpdate', '批量更新')}
          </button>
          <button onClick={() => setShowSettings(true)} className="hub-btn" disabled={disabled}>
            <SlidersHorizontal size={13} /> {t('pages.rag.searchSettings')}
          </button>
        </div>
      </div>

      {/* Toolbar: filename fuzzy search + tag multi-select search */}
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
        {/* 标签搜索：可搜索 + 分页查询后端的多选下拉框 */}
        <TagSearchSelect
          selected={selectedTagFilter}
          onChange={setSelectedTagFilter}
          placeholder={t('pages.rag.tagSearchPlaceholder', '按标签筛选…')}
        />
        <div className="ml-auto flex items-center gap-2">
          <span className="hub-mono text-[12px]" style={{ color: 'var(--hub-ink-3)' }}>
            {docPagination.total}
          </span>
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
      {visibleDocs.length === 0 ? (
        <div className="hub-card p-10 text-center" style={{ color: 'var(--hub-ink-3)' }}>
          <FileText size={20} className="mx-auto mb-2" />
          {pageLoading ? (
            <Loader2 size={16} className="mx-auto mb-2 animate-spin" />
          ) : null}
          <div>{docPagination.total === 0 ? t('pages.rag.empty') : t('pages.rag.noResults')}</div>
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
          {visibleDocs.map((doc, idx) => {
            const checked = selectedIds.has(doc.id);
            const mDoc = doc as MockDocInfo;
            const lost = !!mDoc.lostOriginal;
            // contentAvailable gates the view/open-location buttons (copy docs
            // whose original vanished still have their copy -> view works).
            // lost gates only the ⚠️ badge + (via the update dialog) auto-update.
            const noContent = mDoc.contentAvailable === false;
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
                    className="hub-checkbox"
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
                  {/* 导入方式徽章（软链接 Link / 文件拷贝 Copy），hover 显示原始地址（需求3徽章） */}
                  {mDoc.method && (
                    <span
                      className="hub-tag flex-shrink-0 inline-flex items-center gap-0.5"
                      title={
                        mDoc.method === 'symlink'
                          ? t('pages.rag.methodSymlinkTip', '软链接 · 原始地址：{{path}}', { path: mDoc.originalPath || '-' })
                          : t('pages.rag.methodFileCopyTip', '文件拷贝 · 原始地址：{{path}}', { path: mDoc.originalPath || '-' })
                      }
                      style={{ fontSize: 10, color: 'var(--hub-ink-3)' }}
                    >
                      {mDoc.method === 'symlink' ? <Link2 size={10} /> : <CopyIcon size={10} />}
                      {mDoc.method === 'symlink' ? t('pages.rag.methodSymlink', '软链接') : t('pages.rag.methodFileCopy', '拷贝')}
                    </span>
                  )}
                  {/* 原始文件丢失标记 —— 不影响查看/分片/向量，仅无法自动更新 */}
                  {lost && (
                    <span
                      className="flex-shrink-0 inline-flex items-center gap-0.5"
                      title={t('pages.rag.originalLost', '原始文件不存在，无法自动更新（可手动上传覆盖）')}
                      style={{ color: 'var(--hub-err)', fontSize: 10 }}
                    >
                      <AlertTriangle size={11} />
                      {t('pages.rag.originalLostTag', '原始丢失')}
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
                  onClick={() => handleView(doc)}
                  title={noContent ? t('pages.rag.contentUnavailableView', '无法读取文档内容') : t('pages.rag.view')}
                  disabled={disabled || noContent}
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
                <div className="relative">
                  <button
                    className="hub-icon-btn sm"
                    onClick={() => handleFolderClick(doc)}
                    title={noContent ? t('pages.rag.contentUnavailableOpen', '无法打开文件位置') : t('pages.rag.openFolder')}
                    disabled={disabled || noContent}
                  >
                    <FolderOpen size={13} />
                  </button>
                  {folderMenuId === doc.id && (
                    <>
                      {/* 透明遮罩：点击任意处关闭菜单 */}
                      <div className="fixed inset-0 z-20" onClick={() => setFolderMenuId(null)} />
                      <div
                        className="absolute right-0 top-full z-30 mt-1 rounded-lg border py-1 min-w-[140px]"
                        style={{
                          borderColor: 'var(--hub-line-2)',
                          background: 'var(--hub-surface, #fff)',
                          boxShadow: '0 4px 12px rgba(0,0,0,0.15)',
                        }}
                      >
                        <button
                          className="w-full text-left px-3 py-1.5 text-[12px] hover:bg-black/5 dark:hover:bg-white/10"
                          style={{ color: 'var(--hub-ink)' }}
                          onClick={() => { setFolderMenuId(null); void handleOpenFolder(doc, 'source'); }}
                        >
                          {t('pages.rag.openFolderSource')}
                        </button>
                        <button
                          className="w-full text-left px-3 py-1.5 text-[12px] hover:bg-black/5 dark:hover:bg-white/10"
                          style={{ color: 'var(--hub-ink)' }}
                          onClick={() => { setFolderMenuId(null); void handleOpenFolder(doc); }}
                        >
                          {t('pages.rag.openFolderExtracted')}
                        </button>
                      </div>
                    </>
                  )}
                </div>
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
          {/* 分页页脚：显示区间 + 翻页 + 每页数（与其他列表页一致） */}
          <div className="flex items-center mt-2 text-[12px]" style={{ color: 'var(--hub-ink-3)', borderTop: '1px solid var(--hub-line-2)', padding: '8px 16px' }}>
            <div className="flex-[2]">
              {t('common.showing', {
                start: docPagination.total === 0 ? 0 : (docPagination.page - 1) * docPagination.limit + 1,
                end: Math.min(docPagination.page * docPagination.limit, docPagination.total),
                total: docPagination.total,
              })}
            </div>
            <div className="flex-[4] flex justify-center">
              {docPagination.totalPages > 1 && (
                <Pagination
                  currentPage={docPagination.page}
                  totalPages={docPagination.totalPages}
                  onPageChange={setDocPage}
                  disabled={pageLoading}
                />
              )}
            </div>
            <div className="flex-[2] flex items-center justify-end gap-2">
              <label htmlFor="ragPerPage">{t('common.itemsPerPage')}:</label>
              <select
                id="ragPerPage"
                value={docPageSize}
                onChange={(e) => handleDocPageSizeChange(Number(e.target.value))}
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
        </div>
      )}

      {/* Upload dialog */}
      {showUpload && (
        <UploadDialog
          onClose={() => {
            setShowUpload(false);
            resetPickState();
            setUploadTags([]);
          }}
          scan={scan}
          selectedPaths={currentSelected()}
          scanning={scanning}
          recursive={folderRecursive}
          onRecursiveChange={setFolderRecursive}
          onPick={handlePick}
          onPickFolder={handlePickFolder}
          onToggleFile={toggleFileSelected}
          onToggleGroup={toggleGroupSelected}
          onClearGroup={clearGroupSelected}
          onSetAll={setAllSelected}
          onRemoveFile={removeFile}
          onConfirm={handleUploadConfirm}
          tags={uploadTags}
          onTagsChange={setUploadTags}
          method={uploadMethod}
          onMethodChange={setUploadMethod}
          existingPaths={existingPaths}
        />
      )}

      {/* 单条更新弹框（需求5：软链接/拷贝/老版本/丢失分支） */}
      {updateTarget && (
        <UpdateDialog
          doc={updateTarget}
          check={updateCheck}
          checking={updateChecking}
          actionPhase={updateActionPhase}
          onRetryCheck={() => runUpdateCheck(updateTarget.id)}
          onFromOriginal={() => runUpdateAction(updateTarget, 'original')}
          onManualUpload={async () => {
            await runUpdateAction(updateTarget, 'file');
          }}
          onClose={() => {
            setUpdateTarget(null);
            setUpdateCheck(null);
            setUpdateChecking(false);
            setUpdateActionPhase('idle');
          }}
        />
      )}

      {/* 批量更新前置确认弹框（展示文件更新数量等信息，用户确认后再执行） */}
      {(batchConfirmChecking || batchConfirm) && (
        <BatchUpdateConfirmDialog
          preview={batchConfirm}
          checking={batchConfirmChecking}
          onCancel={() => { setBatchConfirm(null); setBatchConfirmChecking(false); }}
          onConfirm={startBatchUpdate}
        />
      )}

      {/* 批量更新进度弹框（需求6：可关闭，再点按钮重开；上传同款双进度条） */}
      {showBatchUpdateDialog && batchProgressTyped && (
        <BatchUpdateDialog
          progress={batchProgressTyped}
          charProgress={charProgress}
          onClose={() => setShowBatchUpdateDialog(false)}
        />
      )}

      {/* OCR 引擎缺失弹框（导入图片/PDF/Office 时 Linux 无 tesseract） */}
      {ocrMissing && <OcrMissingDialog status={ocrMissing} onClose={dismissOcrMissing} />}

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
          moreLoading={viewMoreLoading}
          onLoadMore={loadMoreView}
          onClose={closeView}
          onSaveTags={async (tags) => {
            await setTags(viewedDoc.id, tags);
            await view(viewedDoc.id);
          }}
          onOpenSourceFile={() => handleViewSourceFile(viewedDoc)}
        />
      )}

      {/* View chunks dialog - lists chunks (index + text) of a doc, paginated:
          5 chunks per page, the next page auto-loads on scroll-to-bottom. */}
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
            <ChunksScrollList
              chunks={chunksList}
              total={chunksTotal}
              loading={chunksLoading}
              moreLoading={chunksMoreLoading}
              onLoadMore={loadMoreChunks}
            />
            <div className="flex items-center justify-between p-5 border-t border-[var(--hub-line-2)]">
              <span className="text-[12px] hub-mono" style={{ color: 'var(--hub-ink-3)' }}>
                {t('pages.rag.chunksShownCount', { shown: chunksList.length, total: chunksTotal })}
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
              {/* 按导入方式区分后果:软链接只删索引/元数据(原始文件不动);
                  拷贝会连 rag/files 里的拷贝文件一起实际删除。 */}
              {(deleteTarget.method === 'symlink' || deleteTarget.method === 'copy') && (
                <div
                  className="mt-2 flex items-start gap-1.5"
                  style={{ fontSize: 12, color: deleteTarget.method === 'copy' ? 'var(--hub-warn)' : 'var(--hub-ink-3)' }}
                >
                  {deleteTarget.method === 'symlink' ? (
                    <>
                      <Link2 size={13} style={{ marginTop: 1, flexShrink: 0 }} />
                      <span>{t('pages.rag.deleteConfirmSymlink')}</span>
                    </>
                  ) : (
                    <>
                      <CopyIcon size={13} style={{ marginTop: 1, flexShrink: 0 }} />
                      <span>{t('pages.rag.deleteConfirmCopy')}</span>
                    </>
                  )}
                </div>
              )}
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
              {/* 批量删除按选中集里两种导入方式的数量分别说明后果(legacy 无
                  method 字段的文档按拷贝处理,后端删除分支同样如此)。 */}
              {(() => {
                const picked = docs.filter((d) => selectedIds.has(d.id));
                const nSymlink = picked.filter((d) => d.method === 'symlink').length;
                const nCopy = picked.length - nSymlink;
                return (
                  <div className="mt-2 space-y-1">
                    {nSymlink > 0 && (
                      <div className="flex items-start gap-1.5" style={{ fontSize: 12, color: 'var(--hub-ink-3)' }}>
                        <Link2 size={13} style={{ marginTop: 1, flexShrink: 0 }} />
                        <span>{t('pages.rag.deleteConfirmSymlinkCount', { count: nSymlink })}</span>
                      </div>
                    )}
                    {nCopy > 0 && (
                      <div className="flex items-start gap-1.5" style={{ fontSize: 12, color: 'var(--hub-warn)' }}>
                        <CopyIcon size={13} style={{ marginTop: 1, flexShrink: 0 }} />
                        <span>{t('pages.rag.deleteConfirmCopyCount', { count: nCopy })}</span>
                      </div>
                    )}
                  </div>
                );
              })()}
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

/** Upload dialog: two entry points (multi-file pick + folder pick with an
 *  optional recursive switch) feeding one grouped candidate tree. Each group
 *  is a sub-folder (root files form the '' group). Selection is checkbox-free:
 *  clicking a row selects it (left accent bar + highlight + CheckCircle icon),
 *  group headers carry 全选 N / 清除 text buttons; the chevron collapses/
 *  expands independently. Only selected files upload; the footer shows the
 *  selected/total count and the confirm button carries a live count badge.
 *  Already-imported paths are flagged in red (existing logic). */
const UploadDialog: React.FC<{
  onClose: () => void;
  scan: RagFolderScan | null;
  /** Currently-checked file paths (null-semantics already resolved by the
   *  parent — this is always a concrete set). */
  selectedPaths: Set<string>;
  scanning: boolean;
  recursive: boolean;
  onRecursiveChange: (v: boolean) => void;
  onPick: () => void;
  onPickFolder: () => void;
  onToggleFile: (path: string) => void;
  onToggleGroup: (files: RagScanFile[]) => void;
  onClearGroup: (files: RagScanFile[]) => void;
  onSetAll: (on: boolean) => void;
  onRemoveFile: (path: string) => void;
  onConfirm: () => void;
  tags: string[];
  onTagsChange: (tags: string[]) => void;
  // 导入方式(软链接/文件拷贝)。
  method?: MockMethod;
  onMethodChange?: (m: MockMethod) => void;
  existingPaths: Set<string>;
}> = ({
  onClose, scan, selectedPaths, scanning, recursive, onRecursiveChange,
  onPick, onPickFolder, onToggleFile, onToggleGroup, onClearGroup, onSetAll,
  onRemoveFile, onConfirm,
  tags, onTagsChange, method, onMethodChange, existingPaths,
}) => {
  const { t } = useTranslation();
  const showMethod = method !== undefined && onMethodChange !== undefined;
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  // OCR pre-flight: when the dialog opens, probe the platform's OCR engine —
  // image/PDF/Office imports route through OCR for (parts of) their content,
  // so warn early on Linux boxes without tesseract instead of failing per
  // file during the upload.
  const [ocrStatus, setOcrStatus] = useState<RagOcrStatus | null>(null);
  useEffect(() => {
    let cancelled = false;
    getOcrStatus()
      .then((s) => { if (!cancelled) setOcrStatus(s); })
      .catch(() => { /* probe failure is non-fatal — skip the hint */ });
    return () => { cancelled = true; };
  }, []);
  const totalFiles = scan ? scan.groups.reduce((n, g) => n + g.files.length, 0) : 0;
  const totalSize = scan
    ? scan.groups.reduce((n, g) => n + g.files.reduce((s, f) => s + (f.size || 0), 0), 0)
    : 0;
  // 递归扫描分组多时对话框加宽,扁平/单组保持原宽度。
  const groupCount = scan?.groups.length ?? 0;
  const wide = groupCount > 1;

  const toggleCollapse = (rel: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(rel)) next.delete(rel);
      else next.add(rel);
      return next;
    });
  };

  return (
    <div className="fixed inset-0 bg-black/50 z-50 flex items-center justify-center p-4">
      <div
        className="bg-white dark:bg-gray-800 rounded-xl shadow-2xl w-full mx-4 border border-gray-100 dark:border-gray-700 max-h-[90vh] flex flex-col"
        style={{ maxWidth: wide ? 640 : 512 }}
      >
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
          {/* OCR 预检提示：Linux 无 tesseract 时提前告知（图片/PDF/Office 依赖） */}
          {ocrStatus && !ocrStatus.available && (
            <p className="text-[12px] flex items-start gap-1.5" style={{ color: 'var(--hub-ink-3)' }}>
              <AlertTriangle size={12} className="mt-0.5 flex-shrink-0" style={{ color: '#d97706' }} />
              {t('pages.rag.ocrPreflightHint')}
            </p>
          )}
          {/* 导入方式选择(软链接 / 文件拷贝)+ 帮助按钮 —— 仿 skill 安装的分段切换样式 */}
          {showMethod && (
            <div className="flex items-center gap-2 flex-wrap">
              <label className="text-[13px] font-medium" style={{ color: 'var(--hub-ink)' }}>
                {t('pages.rag.importMethod', '导入方式')}
              </label>
              <RagMethodHelpIcon />
              <div
                className="inline-flex items-center rounded-md"
                style={{ border: '1px solid var(--hub-line)', background: 'var(--hub-bg-2)' }}
              >
                <button
                  type="button"
                  onClick={() => onMethodChange!('symlink')}
                  title={t('pages.rag.symlink', '软链接')}
                  className="flex items-center gap-1 px-2.5 py-1.5 text-[12px] transition-colors"
                  style={{
                    borderRadius: 5,
                    background: method === 'symlink' ? 'var(--hub-surface)' : 'transparent',
                    color: method === 'symlink' ? 'var(--hub-ink)' : 'var(--hub-ink-3)',
                    border: 'none',
                    cursor: 'pointer',
                    fontWeight: method === 'symlink' ? 600 : 400,
                  }}
                >
                  <Link2 size={12} />
                  {t('pages.rag.symlink', '软链接')}
                </button>
                <button
                  type="button"
                  onClick={() => onMethodChange!('copy')}
                  title={t('pages.rag.fileCopy', '文件拷贝')}
                  className="flex items-center gap-1 px-2.5 py-1.5 text-[12px] transition-colors"
                  style={{
                    borderRadius: 5,
                    background: method === 'copy' ? 'var(--hub-surface)' : 'transparent',
                    color: method === 'copy' ? 'var(--hub-ink)' : 'var(--hub-ink-3)',
                    border: 'none',
                    cursor: 'pointer',
                    fontWeight: method === 'copy' ? 600 : 400,
                  }}
                >
                  <CopyIcon size={12} />
                  {t('pages.rag.fileCopy', '文件拷贝')}
                </button>
              </div>
            </div>
          )}
          {/* OS pickers (Tauri dialog) — backend reads from disk by path,
              no bytes/base64 over IPC. Two entry points: multi-file pick +
              folder pick (flat or recursive per the switch beside it). */}
          <div className="flex items-center gap-2 flex-wrap">
            <button type="button" onClick={onPick} className="hub-btn">
              <Upload size={13} /> {t('pages.rag.uploadSelect')}
            </button>
            <button type="button" onClick={onPickFolder} className="hub-btn" disabled={scanning}>
              {scanning ? <Loader2 size={13} className="animate-spin" /> : <FolderOpen size={13} />}{' '}
              {t('pages.rag.uploadSelectFolder')}
            </button>
            {/* 递归开关:仅影响「选择文件夹」的扫描深度(hub-switch,同全局开关样式) */}
            <span
              className="flex items-center gap-1.5 text-[12px] cursor-pointer select-none"
              style={{ color: 'var(--hub-ink-2)' }}
              title={t('pages.rag.recursiveHint', '开启后递归扫描所选文件夹的全部子目录;关闭只导入一级文件')}
              onClick={() => onRecursiveChange(!recursive)}
            >
              <Switch checked={recursive} onCheckedChange={onRecursiveChange} size="compact" />
              {t('pages.rag.recursive', '包含子文件夹')}
            </span>
          </div>
          {/* 扫描中占位(大目录递归扫描可能 >1s) */}
          {scanning && (
            <div className="hub-card flex flex-col items-center gap-2" style={{ padding: 24 }}>
              <Loader2 size={18} className="animate-spin" style={{ color: 'var(--hub-ink-2)' }} />
              <span className="text-[12.5px]" style={{ color: 'var(--hub-ink-2)' }}>
                {t('pages.rag.scanningFolder', '正在扫描文件夹…')}
              </span>
            </div>
          )}
          {/* 分组树:统计条 + 每个子文件夹一组 */}
          {!scanning && scan && totalFiles > 0 && (
            <div className="space-y-2">
              {/* 统计条:根路径 + 文件夹/文件/大小 */}
              <div
                className="hub-card flex items-center gap-2 flex-wrap"
                style={{ padding: '8px 12px', background: 'var(--hub-bg-2)' }}
              >
                <FolderOpen size={13} style={{ color: 'var(--hub-ink-3)' }} />
                {scan.root && (
                  <span className="hub-mono text-[12px] truncate" style={{ color: 'var(--hub-ink)', maxWidth: 220 }} title={scan.root}>
                    {scan.root}
                  </span>
                )}
                <span className="hub-tag" style={{ fontSize: 10 }}>
                  {t('pages.rag.scanGroupsCount', { count: groupCount })}
                </span>
                <span className="hub-tag" style={{ fontSize: 10 }}>
                  {t('pages.rag.filesSelected', { count: totalFiles })}
                </span>
                {totalSize > 0 && (
                  <span className="hub-tag" style={{ fontSize: 10 }}>
                    {formatSize(totalSize)}
                  </span>
                )}
              </div>
              {scan.truncated && (
                <div
                  className="flex items-start gap-2 hub-card"
                  style={{ padding: '8px 12px', borderColor: 'var(--hub-warn)' }}
                >
                  <AlertTriangle size={14} style={{ color: 'var(--hub-warn)', marginTop: 2, flexShrink: 0 }} />
                  <span className="text-[12px]" style={{ color: 'var(--hub-ink-2)' }}>
                    {t('pages.rag.scanTruncated', {
                      kept: totalFiles,
                      cap: 500,
                    })}
                  </span>
                </div>
              )}
              {(scan.skippedDirs > 0 || scan.skippedFiles > 0) && !scan.truncated && (
                <div className="flex items-center gap-1.5 flex-wrap" style={{ fontSize: 11, color: 'var(--hub-ink-3)' }}>
                  <Info size={11} />
                  {scan.skippedDirs > 0 && (
                    <span>
                      {t('pages.rag.scanSkippedDirs', { count: scan.skippedDirs })}
                    </span>
                  )}
                  {scan.skippedDirs > 0 && scan.skippedFiles > 0 && '·'}
                  {scan.skippedFiles > 0 && (
                    <span>{t('pages.rag.scanSkippedFiles', { count: scan.skippedFiles })}</span>
                  )}
                </div>
              )}
              {/* 组列表 — 无 checkbox 的选中交互:整行点击 = 选中/取消,选中态
                  由左侧 accent 竖条 + 行高亮 + CheckCircle 图标表达;组头右侧
                  「全选 N / 清除」文本按钮整组操作;chevron 只负责折叠。 */}
              <div className="hub-card overflow-hidden">
                {scan.groups.map((g) => {
                  const isCollapsed = collapsed.has(g.relPath);
                  const allChecked = g.files.every((f) => selectedPaths.has(f.path));
                  const someChecked = g.files.some((f) => selectedPaths.has(f.path));
                  const groupActive = allChecked || someChecked;
                  return (
                    <div key={g.relPath || '__root__'} className="border-b last:border-b-0" style={{ borderColor: 'var(--hub-line-2)' }}>
                      {/* 组头:右侧全选/清除按钮整组切换;chevron 折叠 */}
                      <div
                        className="flex items-center gap-2 select-none"
                        style={{ padding: '6px 10px', background: 'var(--hub-bg-2)' }}
                      >
                        <span
                          onClick={(e) => { e.stopPropagation(); toggleCollapse(g.relPath); }}
                          title={isCollapsed ? t('pages.rag.expandGroup', '展开') : t('pages.rag.collapseGroup', '收起')}
                          className="cursor-pointer hover:opacity-70 transition-opacity"
                          style={{ display: 'inline-flex', alignItems: 'center', padding: 2 }}
                        >
                          <ChevronDown
                            size={13}
                            style={{
                              color: 'var(--hub-ink-3)',
                              transform: isCollapsed ? 'rotate(-90deg)' : 'rotate(0deg)',
                              transition: 'transform 0.15s ease',
                            }}
                          />
                        </span>
                        <FolderOpen
                          size={13}
                          style={{ color: groupActive ? 'var(--hub-ink)' : 'var(--hub-ink-3)', flexShrink: 0 }}
                        />
                        <span
                          className="hub-mono text-[12px] font-medium truncate"
                          style={{ color: 'var(--hub-ink)' }}
                          title={g.relPath || scan.root}
                        >
                          {g.relPath || t('pages.rag.scanRootGroup', '(根目录)')}
                        </span>
                        {/* 全选/清除:文本按钮,选中数 >0 时显示已选比例 */}
                        <button
                          type="button"
                          onClick={() => onToggleGroup(g.files)}
                          className="ml-auto flex-shrink-0 text-[11.5px] font-medium cursor-pointer bg-transparent border-0 transition-colors"
                          style={{ color: allChecked ? 'var(--hub-ink-3)' : 'var(--hub-ink-2)', padding: '2px 6px' }}
                        >
                          {allChecked
                            ? t('pages.rag.clearGroup', '清除')
                            : someChecked
                              ? t('pages.rag.selectAllGroupRemaining', '补选 {{count}}', { count: g.files.length - g.files.filter((f) => selectedPaths.has(f.path)).length })
                              : t('pages.rag.selectAllGroup', '全选 {{count}}', { count: g.files.length })}
                        </button>
                        {/* 部分选中时提供独立「清除 M」(否则点行逐个取消才能清空) */}
                        {someChecked && !allChecked && (
                          <>
                            <button
                              type="button"
                              onClick={() => onClearGroup(g.files)}
                              className="flex-shrink-0 text-[11.5px] cursor-pointer bg-transparent border-0 transition-colors"
                              style={{ color: 'var(--hub-ink-3)', padding: '2px 6px' }}
                            >
                              {t('pages.rag.clearGroupCount', '清除 {{count}}', { count: g.files.filter((f) => selectedPaths.has(f.path)).length })}
                            </button>
                            <span className="hub-mono text-[11px] flex-shrink-0" style={{ color: 'var(--hub-ink-3)' }}>
                              {g.files.filter((f) => selectedPaths.has(f.path)).length}/{g.files.length}
                            </span>
                          </>
                        )}
                      </div>
                      {/* 组内文件(收起时隐藏):整行点击 = 切换选中;✕ 仅移除 */}
                      {!isCollapsed &&
                        g.files.map((file) => {
                          const exists = existingPaths.has(file.path);
                          const checked = selectedPaths.has(file.path);
                          return (
                            <div
                              key={file.path}
                              className="group/file relative flex items-center gap-2 select-none cursor-pointer transition-colors"
                              style={{
                                padding: '5px 10px 5px 14px',
                                background: checked ? 'var(--hub-surface-hover)' : undefined,
                              }}
                              onClick={() => onToggleFile(file.path)}
                              role="option"
                              aria-selected={checked}
                            >
                              {/* 选中标记:左侧 accent 竖条 */}
                              {checked && (
                                <span
                                  className="absolute left-0 top-0 bottom-0"
                                  style={{ width: 2.5, background: 'var(--hub-ink)', borderRadius: '0 2px 2px 0' }}
                                />
                              )}
                              {checked ? (
                                <CheckCircle2 size={13} style={{ color: 'var(--hub-ink)', flexShrink: 0 }} />
                              ) : (
                                <FileText size={13} style={{ color: 'var(--hub-ink-3)', flexShrink: 0 }} className="opacity-60 transition-opacity group-hover/file:opacity-100" />
                              )}
                              <span
                                className="truncate text-[12.5px]"
                                style={{ color: checked ? 'var(--hub-ink)' : 'var(--hub-ink-2)' }}
                                title={file.path}
                              >
                                {file.name}
                              </span>
                              {exists && (
                                <span className="text-[11px] flex-shrink-0" style={{ color: 'var(--hub-err)' }}>
                                  {t('pages.rag.pathExists')}
                                </span>
                              )}
                              <span className="hub-mono text-[11px] ml-auto flex-shrink-0" style={{ color: 'var(--hub-ink-3)' }}>
                                {file.size > 0 ? formatSize(file.size) : ''}
                              </span>
                              <button
                                className="hub-icon-btn sm opacity-0 group-hover/file:opacity-100"
                                onClick={(e) => { e.stopPropagation(); onRemoveFile(file.path); }}
                                title={t('pages.rag.removeFile')}
                                style={{ width: 22, height: 22 }}
                              >
                                <X size={12} />
                              </button>
                            </div>
                          );
                        })}
                    </div>
                  );
                })}
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
          {totalFiles > 0 && (
            <>
              <span className="hub-mono text-[12px] mr-auto" style={{ color: 'var(--hub-ink-3)' }}>
                {t('pages.rag.scanSelectedCount', { selected: selectedPaths.size, total: totalFiles })}
              </span>
              {/* 全局 全选/清除:与组头按钮同语义,一键覆盖所有分组 */}
              <button
                type="button"
                onClick={() => onSetAll(selectedPaths.size < totalFiles)}
                className="text-[12px] font-medium cursor-pointer bg-transparent border-0 transition-colors"
                style={{ color: 'var(--hub-ink-2)', padding: '2px 6px' }}
              >
                {selectedPaths.size < totalFiles
                  ? t('pages.rag.selectAllAll', '全选 {{count}}', { count: totalFiles })
                  : t('pages.rag.clearAllAll', '清除 {{count}}', { count: totalFiles })}
              </button>
            </>
          )}
          <button onClick={onClose} className="hub-btn">
            {t('pages.rag.cancel')}
          </button>
          <button onClick={onConfirm} className="hub-btn primary" disabled={selectedPaths.size === 0}>
            {t('pages.rag.uploadConfirm')}
            {selectedPaths.size > 0 && (
              <span
                className="hub-mono"
                style={{
                  fontSize: 11,
                  background: 'rgba(255,255,255,0.22)',
                  borderRadius: 9,
                  padding: '0 7px',
                  height: 17,
                  display: 'inline-flex',
                  alignItems: 'center',
                }}
              >
                {selectedPaths.size}
              </span>
            )}
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
  // Auto doc update (timed batch re-index): on/off + interval (minutes).
  // Backend clamps the interval to [60s, 86400s]; the slider works in minutes
  // ([1, 1440]) for readability and converts on save. `?? fallbacks` cover a
  // settings fetch that hasn't landed yet (backend defaults: on / 5 min).
  const [autoUpdateEnabled, setAutoUpdateEnabled] = useState(initial.autoUpdateEnabled ?? true);
  const [autoUpdateIntervalMin, setAutoUpdateIntervalMin] = useState(
    Math.max(1, Math.round((initial.autoUpdateIntervalSecs ?? 300) / 60)),
  );
  // Doc-detail page size (KiB): how much content the View dialog loads per
  // page. Backend clamps to [10, 65536].
  const [docLoadChunkKb, setDocLoadChunkKb] = useState(initial.docLoadChunkKb || 200);
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
          {/* ── Document update / loading settings ── */}
          <div className="border-t border-[var(--hub-line-2)] pt-4 space-y-4">
            <p className="text-[12px] font-medium" style={{ color: 'var(--hub-ink-2)' }}>
              {t('pages.rag.docUpdateSection')}
            </p>
            {/* Auto doc update toggle */}
            <div>
              <label className="flex items-center gap-2 text-[13px] cursor-pointer" style={{ color: 'var(--hub-ink)' }}>
                <input
                  type="checkbox"
                  checked={autoUpdateEnabled}
                  onChange={(e) => setAutoUpdateEnabled(e.target.checked)}
                  style={{ accentColor: 'var(--hub-accent)' }}
                />
                {t('pages.rag.autoUpdateEnabled')}
              </label>
              <p className="mt-1 text-[12px]" style={{ color: 'var(--hub-ink-3)' }}>
                {t('pages.rag.autoUpdateHint')}
              </p>
            </div>
            {/* Interval (minutes) — only meaningful while enabled */}
            <div style={autoUpdateEnabled ? {} : { opacity: 0.5, pointerEvents: 'none' }}>
              <NumericSlider
                label={t('pages.rag.autoUpdateInterval')}
                value={autoUpdateIntervalMin}
                min={1}
                max={1440}
                step={1}
                unit={t('pages.rag.unitMinutes')}
                onChange={setAutoUpdateIntervalMin}
              />
              <p className="mt-1 text-[12px]" style={{ color: 'var(--hub-ink-3)' }}>
                {t('pages.rag.autoUpdateIntervalHint')}
              </p>
            </div>
            {/* Doc-detail page size */}
            <div>
              <NumericSlider
                label={t('pages.rag.docLoadChunkKb')}
                value={docLoadChunkKb}
                min={10}
                max={65_536}
                step={10}
                unit="KB"
                onChange={(v) => setDocLoadChunkKb(Math.max(10, Math.min(65_536, v)))}
              />
              <p className="mt-1 text-[12px]" style={{ color: 'var(--hub-ink-3)' }}>
                {t('pages.rag.docLoadChunkKbHint')}
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
                autoUpdateEnabled,
                autoUpdateIntervalSecs: autoUpdateIntervalMin * 60,
                docLoadChunkKb,
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

/** Position for a portal-rendered (fixed) dropdown anchored to an element. */
interface AnchoredPos {
  /** viewport top for the dropdown's top edge (bottom placement). */
  top?: number;
  /** CSS `bottom` value for the dropdown's bottom edge (top placement). */
  bottom?: number;
  left: number;
  width: number;
  placement: 'bottom' | 'top';
}

/**
 * Fixed positioning for dropdowns rendered via createPortal to document.body.
 * Dialog bodies use overflow-y-auto, which clips absolutely-positioned
 * children (the original "tag dropdown hidden in the import dialog" bug);
 * portalling to body + fixed coords escapes any scroll container. Recomputes
 * on open, scroll (any ancestor, including the dialog body), and resize.
 * Flips above the anchor (anchored by its bottom edge, so short lists hug
 * the input) when there isn't room below.
 */
const useAnchoredPos = (
  anchorRef: React.RefObject<HTMLElement | null>,
  open: boolean,
  maxHeight: number,
  gap = 4
): AnchoredPos | null => {
  const [pos, setPos] = useState<AnchoredPos | null>(null);

  useEffect(() => {
    if (!open) {
      setPos(null);
      return;
    }
    const measure = () => {
      const el = anchorRef.current;
      if (!el) return;
      const r = el.getBoundingClientRect();
      const vh = window.innerHeight;
      const below = vh - r.bottom;
      // Flip above only when the space below is clearly too short AND there
      // is more room above.
      const placement: 'bottom' | 'top' = below >= Math.min(maxHeight, 160) || below >= r.top ? 'bottom' : 'top';
      setPos({
        left: r.left,
        width: r.width,
        placement,
        top: placement === 'bottom' ? r.bottom + gap : undefined,
        bottom: placement === 'top' ? vh - r.top + gap : undefined,
      });
    };
    measure();
    // capture: catch scrolls of any ancestor (dialog body) on the way down.
    document.addEventListener('scroll', measure, true);
    window.addEventListener('resize', measure);
    return () => {
      document.removeEventListener('scroll', measure, true);
      window.removeEventListener('resize', measure);
    };
  }, [open, maxHeight, gap, anchorRef]);

  return pos;
};

/** Reusable tag editor: chips with remove + a searchable, paginated dropdown
 *  that queries the backend (so picking from a large tag library never shows
 *  the wrong tag). Typing searches the backend (debounced); scrolling to the
 *  bottom loads the next page. A brand-new tag can still be typed + Enter'd
 *  even if it isn't in the library yet. */
const TagEditor: React.FC<{ tags: string[]; onChange: (tags: string[]) => void }> = ({
  tags,
  onChange,
}) => {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [items, setItems] = useState<RagTagStat[]>([]);
  const [total, setTotal] = useState(0);
  const [page, setPage] = useState(0);
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  // Portal-rendered dropdown lives at document.body, so the outside-click
  // check must include it (contains() only sees the DOM subtree).
  const menuRef = useRef<HTMLDivElement>(null);
  const reqId = useRef(0);
  const PAGE_SIZE = 50;
  // max dropdown height: 220px list + ~34px "add new" row ≈ 254. Anchor to the
  // WRAPPER (chips row + input, i.e. the full tag editor row) so the dropdown
  // spans the tag bar's full width instead of just the trailing input box.
  const pos = useAnchoredPos(wrapRef, open, 254);

  // Debounced backend search. Resets the list whenever the query changes.
  useEffect(() => {
    if (!open) return;
    const id = ++reqId.current;
    setLoading(true);
    const timer = setTimeout(async () => {
      try {
        const res = await ragTagSearchPaged(query.trim(), 0, PAGE_SIZE);
        if (id !== reqId.current) return; // a newer request superseded us
        setItems(res.items);
        setTotal(res.total);
        setPage(0);
      } catch {
        if (id === reqId.current) {
          setItems([]);
          setTotal(0);
          setPage(0);
        }
      } finally {
        if (id === reqId.current) setLoading(false);
      }
    }, 250);
    return () => clearTimeout(timer);
  }, [open, query]);

  // Close on outside click.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      const inside = (wrapRef.current && wrapRef.current.contains(t)) || (menuRef.current && menuRef.current.contains(t));
      if (!inside) setOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open]);

  const hasMore = items.length < total;

  const loadMore = useCallback(async () => {
    if (loadingMore || !hasMore) return;
    const id = ++reqId.current;
    setLoadingMore(true);
    const next = page + 1;
    try {
      const res = await ragTagSearchPaged(query.trim(), next, PAGE_SIZE);
      if (id !== reqId.current) return;
      setItems((prev) => {
        const seen = new Set(prev.map((p) => p.tag));
        const merged = [...prev];
        for (const it of res.items) if (!seen.has(it.tag)) merged.push(it);
        return merged;
      });
      setTotal(res.total);
      setPage(next);
    } catch {
      // ignore — user can scroll again to retry
    } finally {
      if (id === reqId.current) setLoadingMore(false);
    }
  }, [loadingMore, hasMore, page, query]);

  const onListScroll = (e: React.UIEvent<HTMLDivElement>) => {
    const el = e.currentTarget;
    if (el.scrollHeight - el.scrollTop - el.clientHeight < 24) loadMore();
  };

  // Case-insensitive check: a tag already attached (any spelling) is filtered
  // out of the dropdown and can't be re-added.
  const hasTagCI = (v: string) => tags.some((x) => x.toLowerCase() === v.toLowerCase());

  const add = (tag: string) => {
    const v = tag.trim();
    if (!v || hasTagCI(v)) return;
    onChange([...tags, v]);
    setQuery('');
  };

  const remove = (tag: string) => onChange(tags.filter((x) => x !== tag));

  const commitDraft = () => {
    const parts = query
      .split(',')
      .map((s) => s.trim())
      .filter((s) => s.length > 0 && !hasTagCI(s));
    if (parts.length > 0) onChange([...tags, ...parts]);
    setQuery('');
  };

  // Exact-match (case-insensitive) against the LIBRARY items for the "add new
  // tag" row; the list itself additionally hides tags already attached.
  const exactInList = items.some((it) => it.tag.toLowerCase() === query.trim().toLowerCase());
  // Dropdown list: hide tags already on this doc (they're shown as chips above).
  const selectableItems = items.filter((it) => !hasTagCI(it.tag));

  return (
    <div className="flex items-center gap-1.5 flex-wrap" ref={wrapRef} style={{ minHeight: 30 }}>
      {tags.map((tag) => (
        <span
          key={tag}
          className="hub-tag flex items-center gap-1"
          style={{ fontSize: 11, padding: '2px 6px' }}
        >
          {tag}
          <button
            type="button"
            onClick={() => remove(tag)}
            className="inline-flex"
            style={{ lineHeight: 1 }}
            title={t('pages.rag.removeFile')}
          >
            <X size={11} />
          </button>
        </span>
      ))}
      <div className="relative flex-1" style={{ minWidth: 120 }}>
        <input
          type="text"
          value={query}
          onFocus={() => setOpen(true)}
          onChange={(e) => {
            setQuery(e.target.value);
            setOpen(true);
          }}
          onKeyDown={(e) => {
            if (e.key === 'Enter' || e.key === ',') {
              e.preventDefault();
              commitDraft();
            } else if (e.key === 'Backspace' && query === '' && tags.length > 0) {
              remove(tags[tags.length - 1]);
            }
          }}
          onBlur={() => {
            // Defer so a click on a dropdown item registers first.
            setTimeout(() => setOpen(false), 150);
            commitDraft();
          }}
          placeholder={t('pages.rag.tagPlaceholder')}
          className="w-full bg-transparent outline-none text-[12.5px]"
          style={{ color: 'var(--hub-ink)' }}
        />
        {open && pos && createPortal(
          <div
            ref={menuRef}
            className="fixed z-[60] rounded-lg shadow-2xl border overflow-hidden"
            style={{
              top: pos.top,
              bottom: pos.bottom,
              left: pos.left,
              width: pos.width,
              background: 'var(--hub-surface)',
              borderColor: 'var(--hub-line-2)',
              maxHeight: pos.placement === 'top' ? 254 : undefined,
              display: 'flex',
              flexDirection: 'column',
            }}
          >
            <div
              ref={listRef}
              onScroll={onListScroll}
              className="overflow-y-auto"
              style={{ maxHeight: 220, overscrollBehavior: 'contain' }}
            >
              {loading ? (
                <div className="flex items-center justify-center gap-1.5 text-[12px]" style={{ color: 'var(--hub-ink-3)', padding: '12px 0' }}>
                  <Loader2 size={12} className="animate-spin" />
                  {t('pages.rag.tagSearchLoading', '搜索中…')}
                </div>
              ) : selectableItems.length === 0 ? (
                <div className="text-[12px] text-center" style={{ color: 'var(--hub-ink-3)', padding: '12px 0' }}>
                  {query.trim()
                    ? t('pages.rag.tagSearchEmpty', '无匹配标签')
                    : t('pages.rag.tagSearchEmptyAll', '暂无标签')}
                </div>
              ) : (
                selectableItems.map((it) => {
                  const checked = tags.includes(it.tag);
                  return (
                    <button
                      key={it.tag}
                      type="button"
                      onMouseDown={(e) => e.preventDefault()}
                      onClick={() => (checked ? remove(it.tag) : add(it.tag))}
                      className="flex items-center gap-2 w-full text-left"
                      style={{ padding: '6px 10px', background: checked ? 'var(--hub-surface)' : 'transparent', border: 'none', cursor: 'pointer' }}
                    >
                      <span
                        style={{
                          width: 14, height: 14, borderRadius: 3, flexShrink: 0,
                          border: '1px solid var(--hub-line)',
                          background: checked ? 'var(--hub-accent)' : 'transparent',
                          display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
                        }}
                      >
                        {checked && <Check size={10} style={{ color: '#fff' }} />}
                      </span>
                      <span className="truncate text-[12.5px] flex-1" style={{ color: 'var(--hub-ink)' }}>{it.tag}</span>
                      <span className="text-[10.5px] hub-mono" style={{ color: 'var(--hub-ink-3)' }}>{it.fileCount}</span>
                    </button>
                  );
                })
              )}
              {!loading && hasMore && (
                <button
                  type="button"
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={loadMore}
                  className="flex items-center justify-center gap-1.5 w-full text-[11.5px]"
                  style={{ padding: '8px 0', color: 'var(--hub-ink-3)', border: 'none', cursor: 'pointer', borderTop: '1px solid var(--hub-line)' }}
                >
                  {loadingMore ? <Loader2 size={11} className="animate-spin" /> : <ChevronDown size={11} />}
                  {loadingMore ? t('pages.rag.tagSearchLoadingMore', '加载中…') : t('pages.rag.tagSearchLoadMore', '加载更多')}
                </button>
              )}
            </div>
            {/* Offer the typed string as a brand-new tag (not in library yet). */}
            {query.trim() && !exactInList && !hasTagCI(query.trim()) && (
              <button
                type="button"
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => add(query.trim())}
                className="flex items-center gap-2 w-full text-left"
                style={{ padding: '7px 10px', borderTop: '1px solid var(--hub-line)', color: 'var(--hub-accent)', border: 'none', cursor: 'pointer', background: 'transparent' }}
              >
                <Plus size={12} />
                <span className="text-[12.5px]">{t('pages.rag.tagAddNew', '添加 "{{tag}}"', { tag: query.trim() })}</span>
              </button>
            )}
          </div>,
          document.body
        )}
      </div>
    </div>
  );
};

/** View dialog: show doc content + editable tags. Tags are persisted in real
 *  time on each add/remove (no Save button) — `onSaveTags(next)` is called
 *  with the full new list on every change. */
const ViewDialog: React.FC<{
  doc: RagDoc;
  moreLoading: boolean;
  onLoadMore: () => void;
  onClose: () => void;
  onSaveTags: (tags: string[]) => Promise<void>;
  /** Open the doc's ORIGINAL source file with the OS default app. Only
   *  provided for extractable-source docs (PDF/Office/image); the button is
   *  hidden when absent or when the source is lost/unknown. */
  onOpenSourceFile?: () => void;
}> = ({ doc, moreLoading, onLoadMore, onClose, onSaveTags, onOpenSourceFile }) => {
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
          {/* Content toolbar: render/source toggle + view-original-file
              button (extractable sources only). Zoom is gesture-only
              (Ctrl/Cmd + wheel or pinch) - show the level as a hint. */}
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2">
              <button
                type="button"
                onClick={() => setMode((m) => (m === 'render' ? 'source' : 'render'))}
                className="hub-btn sm inline-flex items-center gap-1"
                title={mode === 'render' ? t('pages.rag.viewSource') : t('pages.rag.viewRender')}
              >
                {mode === 'render' ? <Code size={13} /> : <Eye size={13} />}
                <span>{mode === 'render' ? t('pages.rag.viewSource') : t('pages.rag.viewRender')}</span>
              </button>
              {/* 查看源文件：二次解析类文档（PDF/Office/图片）默认展示的是
                  提取后的 Markdown/OCR 文本；此按钮用系统默认程序打开原始
                  文件。原始丢失/未知时隐藏。 */}
              {onOpenSourceFile && isExtractedSource(doc.name) && doc.originalPath && doc.lostOriginal !== true && (
                <button
                  type="button"
                  onClick={onOpenSourceFile}
                  className="hub-btn sm inline-flex items-center gap-1"
                  title={t('pages.rag.openOriginalFile')}
                >
                  <FileText size={13} />
                  <span>{t('pages.rag.openOriginalFile')}</span>
                </button>
              )}
            </div>
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
          {/* Paged-content footer: only when the doc was truncated (only part
              of it loaded). Shows how much is loaded out of the total and the
              "load more" button that fetches the next docLoadChunkKb slice.
              Each click APPENDS (see loadMoreView) until truncated=false. */}
          {doc.truncated && (
            <div
              className="flex items-center justify-between gap-2 rounded-lg border px-3"
              style={{ borderColor: 'var(--hub-line-2)', background: 'var(--hub-surface)', padding: '8px 12px' }}
            >
              <span className="text-[12px] hub-mono" style={{ color: 'var(--hub-ink-3)' }}>
                {t('pages.rag.viewLoadedHint', {
                  loaded: formatSize(utf8Bytes(doc.content)),
                  total: formatSize(doc.contentTotalBytes ?? 0),
                })}
              </span>
              <button
                type="button"
                onClick={onLoadMore}
                disabled={moreLoading}
                className="hub-btn sm inline-flex items-center gap-1"
                title={t('pages.rag.viewLoadMoreHint')}
              >
                {moreLoading ? <Loader2 size={13} className="animate-spin" /> : <ChevronDown size={13} />}
                {t('pages.rag.viewLoadMore')}
              </button>
            </div>
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

/** Scrollable chunks list with scroll-to-bottom auto-load. Shows the loaded
 *  pages (5 chunks each); when the user scrolls to (near) the bottom and more
 *  pages remain, `onLoadMore` fires to fetch the next page. A manual "load
 *  more" button sits at the tail as a fallback for scroll-jitter edge cases. */
const ChunksScrollList: React.FC<{
  chunks: { chunkIndex: number; chunkText: string }[];
  total: number;
  loading: boolean;
  moreLoading: boolean;
  onLoadMore: () => void;
}> = ({ chunks, total, loading, moreLoading, onLoadMore }) => {
  const { t } = useTranslation();
  const scrollRef = useRef<HTMLDivElement>(null);
  const hasMore = chunks.length < total;

  // Scroll-to-bottom detection: fire when within 40px of the bottom (a small
  // threshold so it triggers reliably even with fractional scroll positions /
  // zoom). Guarded by `moreLoading` upstream (loadMoreChunks no-ops while a
  // fetch is in flight), so repeated scroll events don't stack requests.
  const handleScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el || loading || moreLoading || !hasMore) return;
    const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
    if (nearBottom) onLoadMore();
  }, [loading, moreLoading, hasMore, onLoadMore]);

  return (
    <div
      ref={scrollRef}
      onScroll={handleScroll}
      className="flex-1 overflow-y-auto p-5 space-y-3"
    >
      {loading ? (
        <div className="flex items-center justify-center py-12">
          <Loader2 size={22} className="animate-spin" style={{ color: 'var(--hub-ink-3)' }} />
        </div>
      ) : chunks.length === 0 ? (
        <p className="text-[13px] text-center py-12" style={{ color: 'var(--hub-ink-3)' }}>
          {t('pages.rag.chunksEmpty')}
        </p>
      ) : (
        <>
          {chunks.map((c) => (
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
          ))}
          {/* Tail: next-page spinner / manual load-more / all-loaded hint. */}
          {hasMore ? (
            <div className="flex items-center justify-center py-3">
              {moreLoading ? (
                <Loader2 size={16} className="animate-spin" style={{ color: 'var(--hub-ink-3)' }} />
              ) : (
                <button type="button" className="hub-btn sm" onClick={onLoadMore}>
                  <ChevronDown size={13} />
                  {t('pages.rag.chunksLoadMore', { count: Math.min(5, total - chunks.length) })}
                </button>
              )}
            </div>
          ) : (
            <div
              className="flex items-center justify-center gap-1.5 text-[11px] py-3"
              style={{ color: 'var(--hub-ink-3)' }}
            >
              <Check size={12} />
              {t('pages.rag.chunksAllLoaded')}
            </div>
          )}
        </>
      )}
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
            {busy ? <Loader2 size={13} className="animate-spin" /> : <Tag size={13} />}{' '}
            {mode === 'add' ? t('pages.rag.saveTags') : t('pages.rag.removeTags')}
          </button>
        </div>
      </div>
    </div>
  );
};

// ── 单条更新弹框（需求5：软链接/拷贝/老版本/丢失分支） ──
const UpdateDialog: React.FC<{
  doc: MockDocInfo;
  check: MockUpdateCheck | null;
  checking: boolean;
  actionPhase: 'idle' | 'reindexing' | 'done' | 'error';
  onRetryCheck: () => void;
  onFromOriginal: () => void;
  onManualUpload: () => Promise<void>;
  onClose: () => void;
}> = ({ doc, check, checking, actionPhase, onRetryCheck, onFromOriginal, onManualUpload, onClose }) => {
  const { t } = useTranslation();
  const method = check?.method ?? (doc.method || 'copy');
  const reindexing = actionPhase === 'reindexing';
  const done = actionPhase === 'done';

  // 检查完成后的分支判定：
  //  - 原始丢失（hasOriginalPath && !originalExists，或 lostOriginal）→ 提示 + 强制手动上传
  //  - 原始存在有更新 → 「从原始文件更新」+「手动上传」次按钮
  //  - 原始存在无更新 → 「无更新」+「手动上传」次按钮
  //  - 无 original_path（老版本）→ 仅「手动上传」+ 兼容提示
  const lostOriginal = check ? check.lostOriginal : !!doc.lostOriginal;
  const originalLost = lostOriginal || (check?.hasOriginalPath && !check.originalExists);
  const noOriginalPath = check ? !check.hasOriginalPath : !doc.originalPath;

  return (
    <div className="fixed inset-0 bg-black/50 z-[60] flex items-center justify-center p-4">
      <div className="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-md w-full mx-4 border border-gray-100 dark:border-gray-700 max-h-[90vh] flex flex-col">
        <div className="flex items-center justify-between p-5 border-b border-[var(--hub-line-2)]">
          <h2 className="text-xl font-bold text-gray-900 dark:text-gray-100">
            {t('pages.rag.updateDialogTitle', '更新文档')}
          </h2>
          <button onClick={onClose} className="hub-icon-btn sm" disabled={reindexing}>
            <X size={16} />
          </button>
        </div>
        <div className="flex-1 overflow-y-auto p-5 space-y-3">
          {/* 文档信息 */}
          <div className="hub-card flex items-center gap-2" style={{ padding: '8px 12px', background: 'var(--hub-surface)' }}>
            <FileText size={14} style={{ color: 'var(--hub-ink-3)' }} />
            <span className="truncate text-[13px]" style={{ color: 'var(--hub-ink)' }} title={doc.name}>{doc.name}</span>
            <span className="hub-tag flex-shrink-0" style={{ fontSize: 10 }}>
              {method === 'symlink' ? (
                <span className="inline-flex items-center gap-0.5"><Link2 size={9} />{t('pages.rag.methodSymlink', '软链接')}</span>
              ) : (
                <span className="inline-flex items-center gap-0.5"><CopyIcon size={9} />{t('pages.rag.methodFileCopy', '拷贝')}</span>
              )}
            </span>
          </div>
          {doc.originalPath && (
            <div className="hub-mono text-[11px] truncate" style={{ color: 'var(--hub-ink-3)' }} title={doc.originalPath}>
              {t('pages.rag.originalPath', '原始地址')}: {doc.originalPath}
            </div>
          )}

          {/* 检查中 */}
          {checking && (
            <div className="flex items-center gap-2 text-[13px]" style={{ color: 'var(--hub-ink-2)' }}>
              <Loader2 size={14} className="animate-spin" style={{ color: 'var(--hub-ink-3)' }} />
              {t('pages.rag.updateChecking', '正在检查原始文件…')}
            </div>
          )}

          {/* 原始文件不存在（软链接/拷贝丢失）→ 不禁用，提示 + 强制手动上传（需求更正：丢失仅无法自动更新） */}
          {!checking && originalLost && (
            <div className="flex items-start gap-2 text-[13px]" style={{ color: 'var(--hub-err)' }}>
              <AlertTriangle size={15} className="mt-0.5 flex-shrink-0" />
              <div>{t('pages.rag.updateOriginalLost', '原始文件不存在，无法从原始文件更新。可手动上传文件覆盖。')}</div>
            </div>
          )}

          {/* 原始存在有更新 */}
          {!checking && check?.originalExists && check.originalChanged && (
            <div className="space-y-2">
              <div className="flex items-start gap-2 text-[13px]" style={{ color: 'var(--hub-ink-2)' }}>
                <RefreshCw size={14} className="mt-0.5 flex-shrink-0" style={{ color: 'var(--hub-accent)' }} />
                <div>{t('pages.rag.updateOriginalChanged', '检测到原始文件有更新，可从原始文件一键更新。')}</div>
              </div>
            </div>
          )}

          {/* 原始存在无更新 */}
          {!checking && check?.originalExists && !check.originalChanged && (
            <div className="flex items-start gap-2 text-[13px]" style={{ color: 'var(--hub-ink-3)' }}>
              <Check size={14} className="mt-0.5 flex-shrink-0" />
              <div>{t('pages.rag.updateNoChange', '原始文件无更新。仍可手动上传文件覆盖。')}</div>
            </div>
          )}

          {/* 老版本无 original_path（拷贝）→ 手动上传兼容分支 */}
          {!checking && !check?.originalExists && noOriginalPath && (
            <div className="flex items-start gap-2 text-[13px]" style={{ color: 'var(--hub-ink-2)' }}>
              <Info size={14} className="mt-0.5 flex-shrink-0" />
              <div>
                {t('pages.rag.updateManualUploadLegacyHint', '该文档为老版本（无原始地址记录），请手动选择文件更新。新文件将记录原始地址以便后续更新检测。')}
              </div>
            </div>
          )}

          {/* 执行更新进度（reindexing / done） */}
          {reindexing && (
            <div className="space-y-2" style={{ marginTop: 4 }}>
              <div className="flex items-center gap-2 text-[13px]" style={{ color: 'var(--hub-ink-2)' }}>
                <Loader2 size={14} className="animate-spin" /> {t('pages.rag.updatingFile', { name: doc.name })}
              </div>
              <div style={{ height: 6, borderRadius: 3, background: 'var(--hub-line)', overflow: 'hidden' }}>
                <div style={{ width: '60%', height: '100%', background: 'var(--hub-ink)', transition: 'width 0.2s' }} />
              </div>
            </div>
          )}
          {done && (
            <div className="flex items-center gap-2 text-[13px]" style={{ color: 'var(--hub-accent)' }}>
              <Check size={14} /> {t('pages.rag.updateDone', '文档已更新')}
            </div>
          )}
        </div>

        {/* 底部按钮区：按分支渲染 */}
        <div className="flex items-center justify-end gap-2 p-5 border-t border-[var(--hub-line-2)]">
          {reindexing || done ? (
            <button onClick={onClose} className="hub-btn">{t('pages.rag.close', '关闭')}</button>
          ) : (
            <>
              <button onClick={onClose} className="hub-btn" disabled={reindexing}>
                {t('pages.rag.cancel')}
              </button>
              {/* 原始存在 → 「从原始文件更新」（软链接/拷贝通用）；丢失/无 original_path 则不显示此按钮 */}
              {check?.originalExists && (
                <button onClick={onFromOriginal} className="hub-btn primary" disabled={reindexing || checking}>
                  <RefreshCw size={13} /> {t('pages.rag.updateFromOriginal', '从原始文件更新')}
                </button>
              )}
              {/* 手动上传：原始丢失或老版本无 original_path 时为主按钮，否则为次按钮 */}
              <button
                onClick={onManualUpload}
                className={originalLost || noOriginalPath ? 'hub-btn primary' : 'hub-btn'}
                disabled={reindexing || checking}
              >
                <Upload size={13} /> {t('pages.rag.updateManualUpload', '手动上传文件更新')}
              </button>
              {!checking && (
                <button onClick={onRetryCheck} className="hub-icon-btn sm" title={t('pages.rag.updateRecheck', '重新检查')}>
                  <RefreshCw size={12} />
                </button>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
};

// ── 标签搜索——可搜索 + 分页查询后端的多选下拉框（输入过滤 + 复选 + 已选 chip + 滚动加载更多） ──
const TagSearchSelect: React.FC<{
  selected: Set<string>;
  onChange: (next: Set<string>) => void;
  placeholder: string;
}> = ({ selected, onChange, placeholder }) => {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [items, setItems] = useState<RagTagStat[]>([]);
  const [total, setTotal] = useState(0);
  const [page, setPage] = useState(0);
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const reqId = useRef(0);
  const PAGE_SIZE = 50;
  // max dropdown height: search 38 + chips ~40 + list 200 + load-more 32 ≈ 310
  const pos = useAnchoredPos(wrapRef, open, 310);

  useEffect(() => {
    if (!open) return;
    const id = ++reqId.current;
    setLoading(true);
    const timer = setTimeout(async () => {
      try {
        const res = await ragTagSearchPaged(query.trim(), 0, PAGE_SIZE);
        if (id !== reqId.current) return;
        setItems(res.items);
        setTotal(res.total);
        setPage(0);
      } catch {
        if (id === reqId.current) {
          setItems([]);
          setTotal(0);
          setPage(0);
        }
      } finally {
        if (id === reqId.current) setLoading(false);
      }
    }, 250);
    return () => clearTimeout(timer);
  }, [open, query]);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      const inside = (wrapRef.current && wrapRef.current.contains(t)) || (menuRef.current && menuRef.current.contains(t));
      if (!inside) setOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open]);

  const toggle = (tag: string) => {
    const next = new Set(selected);
    if (next.has(tag)) next.delete(tag);
    else next.add(tag);
    onChange(next);
  };

  const hasMore = items.length < total;

  const loadMore = useCallback(async () => {
    if (loadingMore || !hasMore) return;
    const id = ++reqId.current;
    setLoadingMore(true);
    const next = page + 1;
    try {
      const res = await ragTagSearchPaged(query.trim(), next, PAGE_SIZE);
      if (id !== reqId.current) return;
      setItems((prev) => {
        const seen = new Set(prev.map((p) => p.tag));
        const merged = [...prev];
        for (const it of res.items) if (!seen.has(it.tag)) merged.push(it);
        return merged;
      });
      setTotal(res.total);
      setPage(next);
    } catch {
      // ignore
    } finally {
      if (id === reqId.current) setLoadingMore(false);
    }
  }, [loadingMore, hasMore, page, query]);

  const onListScroll = (e: React.UIEvent<HTMLDivElement>) => {
    const el = e.currentTarget;
    if (el.scrollHeight - el.scrollTop - el.clientHeight < 24) loadMore();
  };

  const selectedList = Array.from(selected);

  return (
    <div className="relative" ref={wrapRef}>
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="hub-card flex items-center gap-1.5"
        style={{
          height: 30,
          background: 'var(--hub-surface)',
          padding: '0 8px',
          minWidth: 240,
          cursor: 'pointer',
          borderColor: open ? 'var(--hub-accent)' : undefined,
        }}
      >
        <Tag size={13} style={{ color: 'var(--hub-ink-3)' }} />
        <span className="truncate text-[13px]" style={{ color: selectedList.length > 0 ? 'var(--hub-ink)' : 'var(--hub-ink-3)' }}>
          {selectedList.length > 0
            ? t('pages.rag.tagSearchCount', '{{count}} 个标签', { count: selectedList.length })
            : placeholder}
        </span>
        <ChevronDown size={12} style={{ marginLeft: 'auto', color: 'var(--hub-ink-3)', flexShrink: 0 }} />
      </button>

      {open && pos && createPortal(
        <div
          ref={menuRef}
          className="fixed z-[60] rounded-lg shadow-2xl border overflow-hidden"
          style={{
            top: pos.top,
            bottom: pos.bottom,
            left: pos.left,
            width: Math.max(pos.width, 220),
            background: 'var(--hub-surface)',
            borderColor: 'var(--hub-line-2)',
            maxHeight: pos.placement === 'top' ? 310 : undefined,
            display: 'flex',
            flexDirection: 'column',
          }}
        >
          {/* 搜索输入 */}
          <div className="flex items-center gap-1.5 px-2.5" style={{ padding: '8px 10px', borderBottom: '1px solid var(--hub-line)' }}>
            <Search size={12} style={{ color: 'var(--hub-ink-3)' }} />
            <input
              autoFocus
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder={t('pages.rag.tagSearchInputPlaceholder', '搜索标签…')}
              className="flex-1 bg-transparent outline-none text-[12.5px]"
              style={{ color: 'var(--hub-ink)' }}
            />
            {query && (
              <button onClick={() => setQuery('')} className="hub-icon-btn sm">
                <X size={10} />
              </button>
            )}
          </div>
          {/* 已选 chips */}
          {selectedList.length > 0 && (
            <div className="flex flex-wrap gap-1" style={{ padding: '8px 10px', borderBottom: '1px solid var(--hub-line)' }}>
              {selectedList.map((tag) => (
                <span
                  key={tag}
                  className="hub-tag inline-flex items-center gap-1"
                  style={{ fontSize: 11, padding: '2px 6px', cursor: 'pointer' }}
                  onClick={() => toggle(tag)}
                >
                  {tag}
                  <X size={10} />
                </span>
              ))}
              <button
                type="button"
                onClick={() => onChange(new Set())}
                className="text-[11px]"
                style={{ color: 'var(--hub-ink-3)' }}
              >
                {t('pages.rag.tagSearchClear', '清除')}
              </button>
            </div>
          )}
          {/* 选项列表 —— 后端分页查询，滚动到底自动加载下一页 */}
          <div className="overflow-y-auto" style={{ maxHeight: 200, overscrollBehavior: 'contain' }} onScroll={onListScroll}>
            {loading ? (
              <div className="flex items-center justify-center gap-1.5 text-[12px]" style={{ color: 'var(--hub-ink-3)', padding: '12px 0' }}>
                <Loader2 size={12} className="animate-spin" />
                {t('pages.rag.tagSearchLoading', '搜索中…')}
              </div>
            ) : items.length === 0 ? (
              <div className="text-[12px] text-center" style={{ color: 'var(--hub-ink-3)', padding: '12px 0' }}>
                {query.trim()
                  ? t('pages.rag.tagSearchEmpty', '无匹配标签')
                  : t('pages.rag.tagSearchEmptyAll', '暂无标签')}
              </div>
            ) : (
              items.map((it) => {
                const checked = selected.has(it.tag);
                return (
                  <button
                    key={it.tag}
                    type="button"
                    onClick={() => toggle(it.tag)}
                    className="flex items-center gap-2 w-full text-left"
                    style={{ padding: '6px 10px', background: checked ? 'var(--hub-surface)' : 'transparent', border: 'none', cursor: 'pointer' }}
                  >
                    <span
                      style={{
                        width: 14, height: 14, borderRadius: 3, flexShrink: 0,
                        border: '1px solid var(--hub-line)',
                        background: checked ? 'var(--hub-accent)' : 'transparent',
                        display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
                      }}
                    >
                      {checked && <Check size={10} style={{ color: '#fff' }} />}
                    </span>
                    <span className="truncate text-[12.5px] flex-1" style={{ color: 'var(--hub-ink)' }}>{it.tag}</span>
                    <span className="text-[10.5px] hub-mono" style={{ color: 'var(--hub-ink-3)' }}>{it.fileCount}</span>
                  </button>
                );
              })
            )}
            {!loading && hasMore && (
              <button
                type="button"
                onClick={loadMore}
                className="flex items-center justify-center gap-1.5 w-full text-[11.5px]"
                style={{ padding: '8px 0', color: 'var(--hub-ink-3)', border: 'none', cursor: 'pointer', borderTop: '1px solid var(--hub-line)' }}
              >
                {loadingMore ? <Loader2 size={11} className="animate-spin" /> : <ChevronDown size={11} />}
                {loadingMore ? t('pages.rag.tagSearchLoadingMore', '加载中…') : t('pages.rag.tagSearchLoadMore', '加载更多')}
              </button>
            )}
          </div>
        </div>,
        document.body
      )}
    </div>
  );
};

// ── 批量更新前置确认弹框（预扫描后展示文件更新数量等信息，用户确认后再执行） ──
const BatchUpdateConfirmDialog: React.FC<{
  preview: BatchPreview | null;
  checking: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}> = ({ preview, checking, onCancel, onConfirm }) => {
  const { t } = useTranslation();
  const none = preview !== null && preview.toUpdate === 0;
  return (
    <div className="fixed inset-0 bg-black/50 z-[60] flex items-center justify-center p-4">
      <div className="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-md w-full mx-4 border border-gray-100 dark:border-gray-700">
        <div className="flex items-center justify-between p-5 border-b border-[var(--hub-line-2)]">
          <h2 className="text-xl font-bold text-gray-900 dark:text-gray-100">
            {t('pages.rag.batchUpdate', '批量更新')}
          </h2>
          <button onClick={onCancel} className="hub-icon-btn sm">
            <X size={16} />
          </button>
        </div>
        <div className="p-5 space-y-3">
          {checking ? (
            <div className="flex items-center gap-2 text-[13px]" style={{ color: 'var(--hub-ink-2)' }}>
              <Loader2 size={14} className="animate-spin" style={{ color: 'var(--hub-ink-3)' }} />
              {t('pages.rag.batchScanChecking', '正在扫描文档更新状态…')}
            </div>
          ) : preview && (
            <>
              <p className="text-[13px]" style={{ color: 'var(--hub-ink-2)' }}>
                {t('pages.rag.batchConfirmSummary', '将扫描全部文档并自动重新索引有更新的文件。以下是预扫描结果：')}
              </p>
              <div className="grid grid-cols-2 gap-2 text-[12px]">
                <div className="hub-card flex items-center justify-between" style={{ padding: '8px 12px', background: 'var(--hub-surface)' }}>
                  <span style={{ color: 'var(--hub-ink-3)' }}>{t('pages.rag.batchScanTotal', '文档总数')}</span>
                  <span className="hub-mono" style={{ color: 'var(--hub-ink)' }}>{preview.total}</span>
                </div>
                <div className="hub-card flex items-center justify-between" style={{ padding: '8px 12px', background: 'var(--hub-surface)' }}>
                  <span style={{ color: 'var(--hub-ink-3)' }}>{t('pages.rag.batchScanToUpdate', '待更新')}</span>
                  <span className="hub-mono" style={{ color: 'var(--hub-accent)' }}>{preview.toUpdate}</span>
                </div>
                <div className="hub-card flex items-center justify-between" style={{ padding: '8px 12px', background: 'var(--hub-surface)' }}>
                  <span style={{ color: 'var(--hub-ink-3)' }}>{t('pages.rag.batchScanSkipped', '无需更新')}</span>
                  <span className="hub-mono" style={{ color: 'var(--hub-ink-3)' }}>{preview.skipped}</span>
                </div>
                <div className="hub-card flex items-center justify-between" style={{ padding: '8px 12px', background: 'var(--hub-surface)' }}>
                  <span style={{ color: 'var(--hub-ink-3)' }}>{t('pages.rag.batchScanLost', '原始丢失')}</span>
                  <span className="hub-mono" style={{ color: preview.lost > 0 ? 'var(--hub-err)' : 'var(--hub-ink-3)' }}>{preview.lost}</span>
                </div>
              </div>
              {none && (
                <div className="flex items-center gap-2 text-[12px]" style={{ color: 'var(--hub-ink-3)' }}>
                  <Check size={13} /> {t('pages.rag.batchScanNone', '没有需要更新的文档。')}
                </div>
              )}
              {preview.lost > 0 && (
                <div className="flex items-start gap-2 text-[12px]" style={{ color: 'var(--hub-ink-3)' }}>
                  <AlertTriangle size={13} className="mt-0.5 flex-shrink-0" style={{ color: 'var(--hub-err)' }} />
                  <div>{t('pages.rag.batchScanLostHint', '{{lost}} 个文档原始文件丢失，将跳过自动更新（可单独手动上传覆盖）。', { lost: preview.lost })}</div>
                </div>
              )}
            </>
          )}
        </div>
        <div className="flex items-center justify-end gap-2 p-5 border-t border-[var(--hub-line-2)]">
          <button onClick={onCancel} className="hub-btn">
            {t('pages.rag.cancel')}
          </button>
          <button onClick={onConfirm} className="hub-btn primary" disabled={checking || none}>
            {!checking && none ? <Check size={13} /> : <RefreshCw size={13} />}
            {t('pages.rag.batchConfirmStart', '开始更新')}
          </button>
        </div>
      </div>
    </div>
  );
};

// ── 批量更新进度弹框（需求6：可关闭，再点按钮重开；使用上传浮层同款样式，只显示文件进度 + 向量进度，不逐条列出） ──
// 向量进度条复用 charProgress（rag://upload-progress 事件，reindex_doc 内发出），
// 与上传浮层同范式：按 name 匹配当前文档，用 charsDone/charsTotal 算百分比。
const BatchUpdateDialog: React.FC<{
  progress: BatchProgress;
  charProgress: { name: string; charsDone: number; charsTotal: number } | null;
  onClose: () => void;
}> = ({ progress, charProgress, onClose }) => {
  const { t } = useTranslation();
  const pct = progress.total > 0 ? Math.round((progress.current / progress.total) * 100) : 0;
  const done = progress.phase === 'done' && progress.current >= progress.total;
  const errored = progress.phase === 'error';
  const reindexing = progress.phase === 'reindexing';
  // 向量进度：charProgress 与当前文档 name 匹配时取 charsDone/charsTotal。
  const matches = charProgress && charProgress.name === progress.name;
  const charsDone = matches ? charProgress!.charsDone : 0;
  const charsTotal = matches ? charProgress!.charsTotal : 0;
  const docPct = charsTotal > 0 ? Math.min(100, Math.round((charsDone / charsTotal) * 100)) : 0;
  return (
    <div className="fixed inset-0 bg-black/40 z-[60] flex items-center justify-center p-4">
      <div
        className="hub-card w-full max-w-sm p-6 flex flex-col items-center gap-4 shadow-2xl relative"
        style={{ background: 'var(--hub-surface)' }}
      >
        {/* 关闭按钮：只关弹框，不中断后台任务 */}
        <button
          onClick={onClose}
          className="hub-icon-btn sm"
          style={{ position: 'absolute', top: 8, right: 8 }}
          title={t('pages.rag.batchCloseDialog', '关闭（后台任务继续运行）')}
        >
          <X size={14} />
        </button>
        {errored ? (
          <AlertTriangle size={22} style={{ color: 'var(--hub-err)' }} />
        ) : done ? (
          <Check size={22} style={{ color: 'var(--hub-accent)' }} />
        ) : (
          <Loader2 size={22} className="animate-spin" style={{ color: 'var(--hub-ink-2)' }} />
        )}
        <div className="text-[13px] text-center" style={{ color: errored ? 'var(--hub-err)' : 'var(--hub-ink)' }}>
          {errored
            ? t('pages.rag.batchUpdateFailed', '批量更新失败，请查看日志')
            : done
            ? t('pages.rag.batchUpdateDone', '批量更新完成')
            : reindexing
            ? t('pages.rag.reindexingFile', { current: progress.current + 1, total: progress.total, name: progress.name })
            : progress.name
            ? t('pages.rag.batchCheckingFile', { name: progress.name })
            : t('pages.rag.batchProgressTitle', '批量更新进度')}
        </div>
        {/* 文件进度条（current/total）——同上传浮层文件级进度样式 */}
        <div className="w-full" style={{ height: 6, borderRadius: 3, background: 'var(--hub-line)', overflow: 'hidden' }}>
          <div
            style={{ width: `${pct}%`, height: '100%', background: 'var(--hub-ink)', transition: 'width 0.2s ease' }}
          />
        </div>
        <div className="hub-mono text-[11px]" style={{ color: 'var(--hub-ink-3)' }}>
          {progress.current}/{progress.total}
        </div>
        {/* 向量进度条（当前文档 char 级子进度，复用 charProgress）——同上传浮层单文档进度样式 */}
        {reindexing && (
          <div className="w-full flex flex-col gap-1" style={{ marginTop: 2 }}>
            <div className="flex items-center justify-between">
              <span className="text-[11px]" style={{ color: 'var(--hub-ink-3)' }}>
                {t('pages.rag.docProgress', '单文档进度')}
              </span>
              <span className="hub-mono text-[11px]" style={{ color: 'var(--hub-ink-3)' }}>
                {docPct}%
              </span>
            </div>
            <div style={{ height: 6, borderRadius: 3, background: 'var(--hub-line)', overflow: 'hidden' }}>
              <div
                style={{ width: `${docPct}%`, height: '100%', background: 'var(--hub-accent, var(--hub-ink))', transition: 'width 0.15s ease' }}
              />
            </div>
            <div className="hub-mono text-[10px]" style={{ color: 'var(--hub-ink-3)' }}>
              {charsTotal > 0
                ? `${charsDone.toLocaleString()} / ${charsTotal.toLocaleString()} ${t('pages.rag.chars', '字符')}`
                : t('pages.rag.docProgressPreparing', '准备中…')}
            </div>
          </div>
        )}
        <div className="text-[11px] text-center" style={{ color: 'var(--hub-ink-3)' }}>
          {t('pages.rag.batchCloseHint', '关闭弹框不会中断后台任务，可点击「查看进度」重新打开。')}
        </div>
      </div>
    </div>
  );
};

export default RagPage;
