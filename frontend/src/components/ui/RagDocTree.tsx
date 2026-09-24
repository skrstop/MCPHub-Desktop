import React, { useMemo, useState, useRef, useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { fileIcon } from '@/utils/fileIcon';
import { Ban, ChevronDown, ChevronsDownUp, ChevronsUpDown, Eye, FileText, FolderOpen, FolderTree, Globe, Loader2, Pencil, RefreshCw, Wrench } from 'lucide-react';
import GitIcon from '@/components/icons/GitIcon';
import { RagDocInfo } from '@/types';

/**
 * RAG 文档树形视图（需求3）——纯 GUI 展示层：
 * 把文档列表按「数据源 → 物理目录链 → 文档」在内存中聚合成树。
 * - 数据源节点：按 `sourceKind + sourceLabel` 分组（folder/git/file/tool）；
 *   存量无 source 字段的文档由后端 classify 为 kind "file"，统一归入
 *   「文件选择」节点。
 * - 目录节点：由 `relPath`（"/" 分隔的目录链）逐级构建，纯虚拟节点。
 * - 叶子行：由父组件注入的 `renderRow` 渲染（与平铺列表同一行 JSX，
 *   数据源徽章/操作按钮/批量勾选完全一致）。
 * 不持久化任何树结构、不改后端契约；搜索过滤在父组件完成后传入。
 */

/** 一棵文档树的节点（目录或文档叶子）。 */
interface TreeNode {
  name: string;
  /** 目录节点的子节点（按名称排序：目录在前，文档在后）。 */
  children: TreeNode[];
  /** 叶子：文档。 */
  doc?: RagDocInfo;
}

const KIND_ORDER: Record<string, number> = { folder: 0, git: 1, file: 2, tool: 3 };

/** 路径前缀判定：child == root 或 child 在 root 之下（与后端
    is_excluded_sync 的 path_under_root 语义一致，忽略尾随斜杠）。 */
const pathUnderRoot = (child: string, root: string): boolean => {
  if (!root || !child) return false;
  const c = child.replace(/\/+$/, '');
  const r = root.replace(/\/+$/, '');
  return c === r || c.startsWith(r + '/');
};

/** 文件夹勾选框：支持半选态。点击 stopPropagation，避免触发整行折叠/展开。 */
const DirCheckbox: React.FC<{
  checked: boolean;
  indeterminate: boolean;
  onChange: () => void;
}> = ({ checked, indeterminate, onChange }) => (
  <input
    type="checkbox"
    className="hub-checkbox flex-shrink-0"
    checked={checked}
    ref={(el) => { if (el) el.indeterminate = indeterminate; }}
    onChange={onChange}
    onClick={(e) => e.stopPropagation()}
  />
);

export interface RagDocTreeHandle {
  expandAll: () => void;
  collapseAll: () => void;
}

const RagDocTree = React.forwardRef<RagDocTreeHandle, {
  docs: RagDocInfo[];
  /** 叶子行渲染（复用平铺列表的单行 JSX）。 */
  renderRow: (doc: RagDocInfo) => React.ReactNode;
  /** 当前勾选的文档 id 集（与平铺列表共享同一状态）。 */
  selectedIds?: Set<string>;
  /** 文件夹勾选回调：勾选 = 级联选中子树全部文档；已全选则取消。 */
  onToggleDocs?: (docs: RagDocInfo[]) => void;
  /** 搜索激活时全部展开（让命中分支可见）。 */
  forceExpanded?: boolean;
  /** 内部「全部收起」状态上报（父组件同步工具栏按钮文案/图标）。 */
  onAllCollapsedChange?: (all: boolean) => void;
  /** 数据源级手动更新（数据源节点上的刷新按钮）：仅 git/folder 数据源渲染。
   *  回调收到该数据源的定位信息（kind + git url / folder root）。 */
  onRefreshSource?: (target: { kind: 'git' | 'folder' | 'file'; url?: string; root?: string }) => void;
  /** 数据源更新进行中（按钮转圈 + 禁用，父组件用数据源 key 标识）。 */
  refreshingSourceKey?: string | null;
  /** 排除注册表当前内容（目录行据此渲染排除按钮的开关状态）。 */
  excludedPaths?: Set<string>;
  /** 排除切换进行中（按钮禁用）。 */
  excludeToggling?: boolean;
  /** 目录行排除切换：target = 目录绝对路径（注册表目录条目前缀覆盖其下文件）。 */
  onToggleDirExcluded?: (dirPath: string) => void;
  /** 文档级批量排除切换（file 逻辑分组的排除按钮用：逐个切换组内文档）。 */
  onToggleDocsExcluded?: (docs: RagDocInfo[]) => void;
  /** 数据源节点排除切换（git/folder）：folder 加根目录条目，git 加持久
   *  clone 目录条目（路径前缀覆盖该数据源全部文档）。scopeDocs = 该数据源
   *  的文档（切换方向按「全部已排除？」判定，取消时清子树逐文件条目）。 */
  onToggleSourceExcluded?: (target: { kind: 'git' | 'folder' | 'file'; url?: string; root?: string }, scopeDocs?: RagDocInfo[]) => void;
  /** 数据源重命名（修改别名）：folder 传 root、git 传 url，
   *  currentLabel = 当前展示名（默认名或已有别名），供弹框预填。 */
  onRenameSource?: (target: { kind: 'git' | 'folder'; url?: string; root?: string; currentLabel: string }) => void;
}>(({ docs, renderRow, selectedIds, onToggleDocs, forceExpanded, onAllCollapsedChange, onRefreshSource, refreshingSourceKey, excludedPaths, excludeToggling, onToggleDirExcluded, onToggleDocsExcluded, onToggleSourceExcluded, onRenameSource }, ref) => {
  const { t } = useTranslation();
  // 折叠状态：key = `${kind}\n${label}\n${dirPath}`；默认全部展开。
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  // 一键展开/收起（需求反馈）：用 override 状态 + 递增版本号驱动全部节点。
  // expandAll 清空 collapsed；collapseAll 收集当前全部节点 key 一次性置入。
  // override版本号变化时无视 collapsed 集合（DirNode/源节点据此强制开/合）。
  const [collapseOverride, setCollapseOverride] = useState<{ all: boolean; version: number } | null>(null);
  const [version, setVersion] = useState(0);

  const toggle = (key: string) => {
    setCollapseOverride(null); // 手动点击后退出全局 override，恢复逐节点状态
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  /** 一键展开：清空 collapsed + 全局展开 override。 */
  const expandAll = () => {
    setCollapsed(new Set());
    setCollapseOverride({ all: false, version: version + 1 });
    setVersion((v) => v + 1);
  };

  /** 一键收起：收集当前全部节点 key（源 + 目录）一次性置入 + 收起 override
   *  （按钮再点 = 全部展开）。 */
  const collapseAll = () => {
    const keys = new Set<string>();
    for (const src of sourcesRef.current) {
      const srcKey = `${src.kind}\n${src.label}`;
      keys.add(srcKey);
      collectDirKeys(buildDirTree(src.docs), '', srcKey, keys);
    }
    setCollapsed(keys);
    setCollapseOverride({ all: true, version: version + 1 });
    setVersion((v) => v + 1);
  };

  // 一键展开/收起由父组件工具栏按钮驱动（ref imperative handle）。
  React.useImperativeHandle(ref, () => ({ expandAll, collapseAll }));

  // ── 按数据源分组 ──
  const sources = useMemo(() => {
    const map = new Map<string, { kind: string; label: string; docs: RagDocInfo[]; gitUrl: string; sourceRoot: string }>();
    for (const d of docs) {
      const kind = d.sourceKind || 'file';
      let label = d.sourceLabel || '';
      if (!label) {
        // 后端 classify 已保证 kind=file 时 label 非空；这里是防御兜底。
        label = t('pages.rag.legacyFileSource', '文件');
      }
      const key = `${kind}\n${label}`;
      let entry = map.get(key);
      if (!entry) {
        entry = { kind, label, docs: [], gitUrl: d.gitUrl || '', sourceRoot: d.sourceRoot || '' };
        map.set(key, entry);
      }
      entry.docs.push(d);
    }
    return [...map.values()].sort((a, b) => {
      const ka = KIND_ORDER[a.kind] ?? 9;
      const kb = KIND_ORDER[b.kind] ?? 9;
      return ka !== kb ? ka - kb : a.label.localeCompare(b.label);
    });
  }, [docs, t]);

  /** 把一组文档按 relPath 目录链构建子树。 */
  const buildDirTree = (groupDocs: RagDocInfo[]): TreeNode[] => {
    const root: TreeNode = { name: '', children: [] };
    for (const d of groupDocs) {
      const segs = (d.relPath || '').split('/').filter(Boolean);
      let cur = root;
      for (const seg of segs) {
        let child = cur.children.find((c) => !c.doc && c.name === seg);
        if (!child) {
          child = { name: seg, children: [] };
          cur.children.push(child);
        }
        cur = child;
      }
      cur.children.push({ name: d.name, children: [], doc: d });
    }
    // 排序：目录在前（按名称），文档在后（按上传时间倒序，与平铺一致）。
    const sortRec = (node: TreeNode) => {
      const dirs = node.children.filter((c) => !c.doc).sort((a, b) => a.name.localeCompare(b.name));
      const files = node.children.filter((c) => c.doc).sort((a, b) => (b.doc!.uploadedAt || '').localeCompare(a.doc!.uploadedAt || ''));
      node.children = [...dirs, ...files];
      for (const c of dirs) sortRec(c);
    };
    sortRec(root);
    return root.children;
  };

  // 当前是否处于「全部收起」态（驱动外部按钮文案/图标，经 onAllCollapsedChange 上报）。
  const allCollapsed = collapseOverride?.all === true;
  useEffect(() => {
    onAllCollapsedChange?.(allCollapsed);
  }, [allCollapsed, onAllCollapsedChange]);
  // collapseAll 需要读取最新 sources（state 闭包旧值问题）→ ref 同步。
  const sourcesRef = useRef(sources);
  useEffect(() => {
    sourcesRef.current = sources;
  }, [sources]);

  /** 收集目录子树全部折叠 key（DirNode 的 key = sourceKey + \n + dirPath）。 */
  const collectDirKeys = (nodes: TreeNode[], path: string, sourceKey: string, out: Set<string>) => {
    for (const n of nodes) {
      if (n.doc) continue;
      const dirPath = path ? `${path}/${n.name}` : n.name;
      out.add(`${sourceKey}\n${dirPath}`);
      collectDirKeys(n.children, dirPath, sourceKey, out);
    }
  };

  const kindIcon = (kind: string, size = 13) => {
    const style = { color: 'var(--hub-ink-3)', flexShrink: 0 };
    if (kind === 'git') return <GitIcon width={size} height={size} style={{ flexShrink: 0 }} />;
    if (kind === 'folder') return <FolderOpen size={size} style={style} />;
    if (kind === 'tool') return <Wrench size={size} style={style} />;
    if (kind === 'file') return fileIcon('', size);
    return <FileText size={size} style={style} />;
  };

  if (docs.length === 0) {
    return (
      <div className="hub-card p-10 text-center" style={{ color: 'var(--hub-ink-3)' }}>
        <FolderTree size={20} className="mx-auto mb-2" />
        {t('pages.rag.treeEmpty', '没有可展示的文档')}
      </div>
    );
  }

  return (
    <div className="hub-card overflow-hidden">
      {sources.map((src) => {
        const srcKey = `${src.kind}\n${src.label}`;
        // override 收起状态优先；override 展开则无视 collapsed 集合。
        const srcCollapsed = forceExpanded
          ? false
          : collapseOverride
            ? collapseOverride.all
            : collapsed.has(srcKey);
        const dirTree = buildDirTree(src.docs);
        // file 分组的排除状态：有 original_path 的文档全部已排除 = 已排除
        //（doc.excluded 由后端按注册表前缀匹配计算，含祖先目录条目覆盖）。
        // git/folder 数据源的排除按钮复用同一判定：路径条目（folder 根 /
        // git clone 目录）覆盖其下全部文档 → 全部 excluded。
        const srcGroupExcluded = src.docs.some((d) => d.originalPath)
          && src.docs.filter((d) => d.originalPath).every((d) => d.excluded);
        const rootDocs = dirTree.filter((n) => n.doc);
        const subDirs = dirTree.filter((n) => !n.doc);
        return (
          <div key={srcKey} className="border-b last:border-b-0" style={{ borderColor: 'var(--hub-line-2)' }}>
            {/* 数据源节点 */}
            <div
              className="flex items-center gap-2 select-none cursor-pointer"
              style={{ padding: '8px 16px', background: 'var(--hub-bg-2)' }}
              onClick={() => toggle(srcKey)}
            >
              {onToggleDocs && (
                <DirCheckbox
                  checked={selectedIds ? src.docs.every((d) => selectedIds.has(d.id)) && src.docs.length > 0 : false}
                  indeterminate={!!selectedIds && src.docs.some((d) => selectedIds.has(d.id)) && !src.docs.every((d) => selectedIds.has(d.id))}
                  onChange={() => onToggleDocs(src.docs)}
                />
              )}
              <ChevronDown
                size={13}
                style={{
                  color: 'var(--hub-ink-3)',
                  transform: srcCollapsed ? 'rotate(-90deg)' : 'rotate(0deg)',
                  transition: 'transform 0.15s ease',
                }}
              />
              {kindIcon(src.kind)}
              <span className="hub-mono text-[12.5px] font-semibold truncate" style={{ color: 'var(--hub-ink)' }} title={src.label}>
                {src.label}
              </span>
              {/* 顶层显示数据源类型（需求3 反馈）：仅 Git / 文件夹展示类型标签；
                  内置的「文件选择」与「MCP 工具」不展示。 */}
              {(src.kind === 'git' || src.kind === 'folder') && (
                <span
                  className="hub-tag flex-shrink-0 inline-flex items-center gap-0.5"
                  style={{ fontSize: 10, color: 'var(--hub-ink-3)' }}
                >
                  {src.kind === 'git' ? <GitIcon width={10} height={10} /> : <FolderOpen size={10} />}
                  {src.kind === 'git'
                    ? t('pages.rag.dataSourceGit', 'Git')
                    : t('pages.rag.dataSourceFolder', '文件夹')}
                </span>
              )}
              <span className="hub-tag ml-1 flex-shrink-0" style={{ fontSize: 10 }}>
                {t('pages.rag.treeDocCount', { count: src.docs.length })}
              </span>
              {/* 行尾操作区（与文件行的按钮列同一 170px 区域对齐）：按钮顺序
                  固定【刷新、排除】。git 强制拉取远端 + 数据源级同步/重索引；
                  folder 扫描源目录同步/重索引；file（逻辑「文件」分组）仅
                  重新索引有变更的文件（无源目录可扫）。排除：folder 加根目录
                  条目、git 加持久 clone 目录条目（路径前缀覆盖其下全部文档，
                  子文件/子文件夹行由后端前缀匹配 + dirExcluded 祖先检查同步
                  生效）；file 是逻辑分组（无单一路径），单独处理——逐个切换
                  组内文档的 original_path。排除状态统一按「有 original_path
                  的文档全部已排除」判定（doc.excluded 由后端按注册表计算）。
                  进行中转圈禁用。marginLeft:auto 把 170px 容器推到行尾（右
                  padding 16 与文件行一致），按钮在容器内左对齐，落点与文件
                  行按钮列的左边缘完全重合。 */}
              <span
                className="flex items-center gap-1 flex-shrink-0"
                style={{ width: 170, marginLeft: 'auto' }}
              >
                {onRefreshSource && (src.kind === 'git' || src.kind === 'folder' || src.kind === 'file') && (
                  <button
                    type="button"
                    className="hub-icon-btn sm flex-shrink-0"
                    style={{ height: 20, width: 20 }}
                    disabled={refreshingSourceKey === srcKey}
                    title={src.kind === 'file'
                      ? t('pages.rag.refreshFile', '更新这些文件（重新索引有变更的文件）')
                      : t('pages.rag.refreshSource', '更新该数据源（拉取远端 + 同步新增/删除 + 重新索引变更文件）')}
                    onClick={(e) => {
                      e.stopPropagation();
                      onRefreshSource(
                        src.kind === 'git'
                          ? { kind: 'git', url: src.gitUrl }
                          : src.kind === 'file'
                            ? { kind: 'file' }
                            : { kind: 'folder', root: src.sourceRoot },
                      );
                    }}
                  >
                    {refreshingSourceKey === srcKey ? (
                      <Loader2 size={11} className="animate-spin" />
                    ) : (
                      <RefreshCw size={11} />
                    )}
                  </button>
                )}
                {/* 排除按钮：file 走逐文档切换（onToggleDocsExcluded）；git/
                    folder 走路径条目（onToggleSourceExcluded）。状态判定同源：
                    有 original_path 的文档全部 excluded = 已排除。 */}
                {(src.kind === 'file'
                  ? !!onToggleDocsExcluded
                  : !!(onToggleSourceExcluded && (src.kind === 'git' || src.kind === 'folder'))) && (
                  <button
                    type="button"
                    className="hub-icon-btn sm flex-shrink-0"
                    style={{ height: 20, width: 20 }}
                    disabled={excludeToggling}
                    title={srcGroupExcluded
                      ? t('pages.rag.excludeFileGroupRemove', '取消排除这些文件（恢复参与更新检查）')
                      : t('pages.rag.excludeFileGroupAdd', '排除这些文件（更新检查忽略）')}
                    onClick={(e) => {
                      e.stopPropagation();
                      if (src.kind === 'file') onToggleDocsExcluded!(src.docs);
                      else onToggleSourceExcluded!(
                        src.kind === 'git'
                          ? { kind: 'git', url: src.gitUrl }
                          : { kind: 'folder', root: src.sourceRoot },
                        src.docs,
                      );
                    }}
                  >
                    {excludeToggling ? (
                      <Loader2 size={11} className="animate-spin" />
                    ) : srcGroupExcluded ? (
                      <Eye size={11} style={{ color: 'var(--hub-accent)' }} />
                    ) : (
                      <Ban size={11} />
                    )}
                  </button>
                )}
                {/* 修改别名按钮（folder/git）：打开重命名弹框，预填当前展示
                    名（别名或默认名）。 */}
                {onRenameSource && (src.kind === 'git' || src.kind === 'folder') && (
                  <button
                    type="button"
                    className="hub-icon-btn sm flex-shrink-0"
                    style={{ height: 20, width: 20 }}
                    title={t('pages.rag.renameSource', '修改别名')}
                    onClick={(e) => {
                      e.stopPropagation();
                      onRenameSource(
                        src.kind === 'git'
                          ? { kind: 'git', url: src.gitUrl, currentLabel: src.label }
                          : { kind: 'folder', root: src.sourceRoot, currentLabel: src.label },
                      );
                    }}
                  >
                    <Pencil size={11} />
                  </button>
                )}
              </span>
            </div>
            {/* 数据源下的目录树 + 根级文档 */}
            {!srcCollapsed && (
              <div style={{ paddingLeft: 18 }}>
                {subDirs.map((n) => (
                  <DirNode key={n.name} node={n} path="" sourceKey={srcKey} sourceKind={src.kind} sourceRoot={src.kind === 'folder' ? src.sourceRoot : ''} sourceGitUrl={src.kind === 'git' ? src.gitUrl : ''} collapsed={collapsed} onToggle={toggle} forceExpanded={forceExpanded} override={collapseOverride} kindIcon={kindIcon} renderRow={renderRow} depth={0} selectedIds={selectedIds} onToggleDocs={onToggleDocs} onRefreshSource={onRefreshSource} refreshingSourceKey={refreshingSourceKey} excludedPaths={excludedPaths} excludeToggling={excludeToggling} onToggleDirExcluded={onToggleDirExcluded} />
                ))}
                {rootDocs.map((n) => (
                  <React.Fragment key={n.doc!.id}>{renderRow(n.doc!)}</React.Fragment>
                ))}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
});

RagDocTree.displayName = 'RagDocTree';

/** 目录节点（虚拟目录）：递归渲染子目录 + 叶子文档。 */
const DirNode: React.FC<{
  node: TreeNode;
  /** 父目录链（不含本节点名）。 */
  path: string;
  sourceKey: string;
  /** 该目录所属数据源的 kind 与根路径（folder 源的目录行据此渲染
   *  「更新该文件夹」按钮，target root = sourceRoot + '/' + dirPath）；
   *  git 源另传 sourceGitUrl（目录行的更新 target = git url + 相对子目录）。 */
  sourceKind?: string;
  sourceRoot?: string;
  sourceGitUrl?: string;
  collapsed: Set<string>;
  onToggle: (key: string) => void;
  forceExpanded?: boolean;
  /** 全局展开/收起 override（一键按钮驱动）。 */
  override: { all: boolean; version: number } | null;
  kindIcon: (kind: string, size?: number) => React.ReactNode;
  renderRow: (doc: RagDocInfo) => React.ReactNode;
  depth: number;
  /** 勾选（级联子树文档）——见 RagDocTree props。 */
  selectedIds?: Set<string>;
  onToggleDocs?: (docs: RagDocInfo[]) => void;
  /** 数据源级手动更新回调（目录行复用：kind='folder' + 子目录绝对路径）。 */
  onRefreshSource?: (target: { kind: 'git' | 'folder' | 'file'; url?: string; root?: string }) => void;
  /** 进行中的刷新 key（`folder\n<root>`），用于按钮转圈/禁用。 */
  refreshingSourceKey?: string | null;
  /** 排除注册表（目录行排除按钮的开关状态）。 */
  excludedPaths?: Set<string>;
  /** 排除切换进行中。 */
  excludeToggling?: boolean;
  /** 目录排除切换回调。 */
  onToggleDirExcluded?: (dirPath: string) => void;
}> = ({ node, path, sourceKey, sourceKind, sourceRoot, sourceGitUrl, collapsed, onToggle, forceExpanded, override, kindIcon, renderRow, depth, selectedIds, onToggleDocs, onRefreshSource, refreshingSourceKey, excludedPaths, excludeToggling, onToggleDirExcluded }) => {
  const { t } = useTranslation();
  const dirPath = path ? `${path}/${node.name}` : node.name;
  const key = `${sourceKey}\n${dirPath}`;
  const isCollapsed = forceExpanded
    ? false
    : override
      ? override.all
      : collapsed.has(key);
  const docCount = countDocs(node);
  // 目录级更新按钮：folder 数据源的 target root = 源根 + 目录链（绝对路径）；
  // git 数据源的 target root = 目录链（clone 内相对子目录，后端自行解析
  // clone 路径）。后端按路径前缀圈定范围（只同步/重索引该子目录下的文件）。
  const dirAbsPath = sourceRoot ? `${sourceRoot.replace(/\/+$/, '')}/${dirPath}` : '';
  const isFolderSrc = sourceKind === 'folder' && !!dirAbsPath;
  const isGitSrc = sourceKind === 'git' && !!sourceGitUrl;
  const dirRefreshKey = isFolderSrc
    ? `folder\n${dirAbsPath}`
    : isGitSrc
      ? `git\n${sourceGitUrl}\n${dirPath}`
      : '';
  const showDirRefresh = !!(onRefreshSource && (isFolderSrc || isGitSrc));
  // 目录排除状态：与数据源行同源——子树内「有 original_path 的文档全部
  // excluded」即视为已排除。⚠️ 不能只查注册表里有无该目录/祖先条目：批量
  // 「排除更新」给注册表加的是逐文件路径（子孙条目），文件行徽章正常但
  // 目录按钮的祖先检查永远不命中，目录状态不更新（用户实测）。doc.excluded
  // 由后端按注册表前缀匹配计算（含祖先目录条目覆盖），本判定与文件行天然
  // 一致；点击按钮时 toggleDirExcluded 的「移除覆盖条目」逻辑会把子树文件
  // 条目一起清掉（pathUnderRoot(dirPath, e)），取消同样整体生效。
  const subDocs = collectDocs(node);
  const dirDocs = subDocs.filter((d) => d.originalPath);
  const dirExcluded = !!dirAbsPath && dirDocs.length > 0
    && dirDocs.every((d) => d.excluded);
  return (
    <div>
      <div
        className="flex items-center gap-1.5 select-none cursor-pointer"
        style={{ padding: '4px 16px 4px 6px' }}
        onClick={() => onToggle(key)}
      >
        {onToggleDocs && (() => {
          const sub = collectDocs(node);
          const allSel = selectedIds ? sub.every((d) => selectedIds.has(d.id)) && sub.length > 0 : false;
          const someSel = !!selectedIds && sub.some((d) => selectedIds.has(d.id));
          return (
            <DirCheckbox
              checked={allSel}
              indeterminate={someSel && !allSel}
              onChange={() => onToggleDocs(sub)}
            />
          );
        })()}
        <ChevronDown
          size={12}
          style={{
            color: 'var(--hub-ink-3)',
            transform: isCollapsed ? 'rotate(-90deg)' : 'rotate(0deg)',
            transition: 'transform 0.15s ease',
          }}
        />
        <FolderOpen size={12} style={{ color: 'var(--hub-ink-3)', flexShrink: 0 }} />
        <span className="text-[12px] truncate" style={{ color: 'var(--hub-ink-2)' }} title={dirPath}>
          {node.name}
        </span>
        <span className="hub-tag flex-shrink-0" style={{ fontSize: 10 }}>
          {t('pages.rag.treeDocCount', { count: docCount })}
        </span>
        {/* 行尾操作区：与文件行按钮列/数据源按钮同一 170px 区域（右 padding
            16 + marginLeft:auto + 容器内左对齐），落点完全重合。按钮顺序
            固定【刷新、排除】。排除 = 注册表加目录条目（前缀覆盖其下全部
            文件，更新检查/数据源同步忽略）；已排除显示 Eye（accent 色）可
            取消，取消时连同覆盖它的祖先条目一起移除。 */}
        {(showDirRefresh || onToggleDirExcluded) && (
          <span className="flex items-center gap-1 flex-shrink-0" style={{ width: 170, marginLeft: 'auto' }}>
            {showDirRefresh && (
              <button
                type="button"
                className="hub-icon-btn sm flex-shrink-0"
                style={{ height: 20, width: 20 }}
                disabled={refreshingSourceKey === dirRefreshKey}
                title={t('pages.rag.refreshFolder', '更新该文件夹（同步新增/删除 + 重新索引变更文件）')}
                onClick={(e) => {
                  e.stopPropagation();
                  onRefreshSource!(
                    isGitSrc
                      ? { kind: 'git', url: sourceGitUrl, root: dirPath }
                      : { kind: 'folder', root: dirAbsPath },
                  );
                }}
              >
                {refreshingSourceKey === dirRefreshKey ? (
                  <Loader2 size={11} className="animate-spin" />
                ) : (
                  <RefreshCw size={11} />
                )}
              </button>
            )}
            {onToggleDirExcluded && (
              <button
                type="button"
                className="hub-icon-btn sm flex-shrink-0"
                style={{ height: 20, width: 20 }}
                disabled={excludeToggling}
                title={dirExcluded
                  ? t('pages.rag.excludeRemoveDir', '取消排除本目录全部文件')
                  : t('pages.rag.excludeAddDir', '排除本目录全部文件（更新检查忽略）')}
                onClick={(e) => {
                  e.stopPropagation();
                  onToggleDirExcluded(dirAbsPath);
                }}
              >
                {excludeToggling ? (
                  <Loader2 size={11} className="animate-spin" />
                ) : dirExcluded ? (
                  <Eye size={11} style={{ color: 'var(--hub-accent)' }} />
                ) : (
                  <Ban size={11} />
                )}
              </button>
            )}
          </span>
        )}
      </div>
      {!isCollapsed && (
        <div style={{ paddingLeft: 14 }}>
          {/* 本目录直属文档在前、子目录在后（文件管理器惯例，与导入弹框扫描树一致）。 */}
          {node.children.filter((c) => c.doc).map((c) => (
            <React.Fragment key={c.doc!.id}>{renderRow(c.doc!)}</React.Fragment>
          ))}
          {node.children.filter((c) => !c.doc).map((c) => (
            <DirNode key={c.name} node={c} path={dirPath} sourceKey={sourceKey} sourceKind={sourceKind} sourceRoot={sourceRoot} sourceGitUrl={sourceGitUrl} collapsed={collapsed} onToggle={onToggle} forceExpanded={forceExpanded} override={override} kindIcon={kindIcon} renderRow={renderRow} depth={depth + 1} selectedIds={selectedIds} onToggleDocs={onToggleDocs} onRefreshSource={onRefreshSource} refreshingSourceKey={refreshingSourceKey} excludedPaths={excludedPaths} excludeToggling={excludeToggling} onToggleDirExcluded={onToggleDirExcluded} />
          ))}
        </div>
      )}
      {isCollapsed && (
        <div className="text-[10.5px]" style={{ paddingLeft: 40, color: 'var(--hub-ink-3)' }}>
          {t('pages.rag.treeCollapsedHint', { count: docCount })}
        </div>
      )}
    </div>
  );
};

/** 收集目录子树内全部文档（含子目录，深度优先）。 */
const collectDocs = (node: TreeNode): RagDocInfo[] =>
  node.children.reduce<RagDocInfo[]>((acc, c) => (c.doc ? [...acc, c.doc] : [...acc, ...collectDocs(c)]), []);

/** 统计目录子树内的文档数（含子目录）。 */
const countDocs = (node: TreeNode): number =>
  node.children.reduce((n, c) => (c.doc ? n + 1 : n + countDocs(c)), 0);

export default RagDocTree;
