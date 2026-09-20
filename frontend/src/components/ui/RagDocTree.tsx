import React, { useMemo, useState, useRef, useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { ChevronDown, ChevronsDownUp, ChevronsUpDown, FileText, FolderOpen, FolderTree, GitBranch, Globe, Wrench } from 'lucide-react';
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
}>(({ docs, renderRow, selectedIds, onToggleDocs, forceExpanded, onAllCollapsedChange }, ref) => {
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
    const map = new Map<string, { kind: string; label: string; docs: RagDocInfo[] }>();
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
        entry = { kind, label, docs: [] };
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
    if (kind === 'git') return <GitBranch size={size} style={style} />;
    if (kind === 'folder') return <FolderOpen size={size} style={style} />;
    if (kind === 'tool') return <Wrench size={size} style={style} />;
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
        const rootDocs = dirTree.filter((n) => n.doc);
        const subDirs = dirTree.filter((n) => !n.doc);
        return (
          <div key={srcKey} className="border-b last:border-b-0" style={{ borderColor: 'var(--hub-line-2)' }}>
            {/* 数据源节点 */}
            <div
              className="flex items-center gap-2 select-none cursor-pointer"
              style={{ padding: '8px 14px', background: 'var(--hub-bg-2)' }}
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
                  {src.kind === 'git' ? <GitBranch size={10} /> : <FolderOpen size={10} />}
                  {src.kind === 'git'
                    ? t('pages.rag.dataSourceGit', 'Git')
                    : t('pages.rag.dataSourceFolder', '文件夹')}
                </span>
              )}
              <span className="hub-tag ml-1 flex-shrink-0" style={{ fontSize: 10 }}>
                {t('pages.rag.treeDocCount', { count: src.docs.length })}
              </span>
            </div>
            {/* 数据源下的目录树 + 根级文档 */}
            {!srcCollapsed && (
              <div style={{ paddingLeft: 18 }}>
                {subDirs.map((n) => (
                  <DirNode key={n.name} node={n} path="" sourceKey={srcKey} collapsed={collapsed} onToggle={toggle} forceExpanded={forceExpanded} override={collapseOverride} kindIcon={kindIcon} renderRow={renderRow} depth={0} selectedIds={selectedIds} onToggleDocs={onToggleDocs} />
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
}> = ({ node, path, sourceKey, collapsed, onToggle, forceExpanded, override, kindIcon, renderRow, depth, selectedIds, onToggleDocs }) => {
  const { t } = useTranslation();
  const dirPath = path ? `${path}/${node.name}` : node.name;
  const key = `${sourceKey}\n${dirPath}`;
  const isCollapsed = forceExpanded
    ? false
    : override
      ? override.all
      : collapsed.has(key);
  const docCount = countDocs(node);
  return (
    <div>
      <div
        className="flex items-center gap-1.5 select-none cursor-pointer"
        style={{ padding: '4px 14px 4px 6px' }}
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
        <span className="hub-mono text-[10.5px]" style={{ color: 'var(--hub-ink-3)' }}>
          {docCount}
        </span>
      </div>
      {!isCollapsed && (
        <div style={{ paddingLeft: 14 }}>
          {node.children.filter((c) => !c.doc).map((c) => (
            <DirNode key={c.name} node={c} path={dirPath} sourceKey={sourceKey} collapsed={collapsed} onToggle={onToggle} forceExpanded={forceExpanded} override={override} kindIcon={kindIcon} renderRow={renderRow} depth={depth + 1} selectedIds={selectedIds} onToggleDocs={onToggleDocs} />
          ))}
          {node.children.filter((c) => c.doc).map((c) => (
            <React.Fragment key={c.doc!.id}>{renderRow(c.doc!)}</React.Fragment>
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
