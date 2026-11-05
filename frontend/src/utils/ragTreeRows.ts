import type { RagDocInfo } from '@/types';

/**
 * RAG 树形视图的摊平（flatten）纯函数层——供 RagDocTree 虚拟滚动消费。
 * 把「数据源 → 目录链 → 文档」树按当前折叠状态摊平为可见行列表，
 * 每行带稳定 key（行高实测缓存的键）、累计缩进与估算行高。
 * 纯函数、无 React 依赖，便于单测覆盖折叠/排序/缩进语义。
 */

/** 一棵文档树的节点（目录或文档叶子）。 */
export interface TreeNode {
  name: string;
  /** 目录节点的子节点（按名称排序：目录在前，文档在后）。 */
  children: TreeNode[];
  /** 叶子：文档。 */
  doc?: RagDocInfo;
}

/** 摊平后的可见行。 */
export interface FlatRow {
  key: string;
  type: 'source' | 'dir' | 'doc' | 'hint';
  /** 累计缩进（px），渲染时作为行包装器的 paddingLeft。 */
  indent: number;
  /** 估算行高（实测前兜底）。 */
  est: number;
  /** type=source：数据源下标。 */
  srcIdx?: number;
  /** type=dir/hint：所属数据源信息（目录行刷新/排除按钮需要）。 */
  srcKey?: string;
  sourceKind?: string;
  sourceRoot?: string;
  sourceGitUrl?: string;
  /** type=dir/hint：目录树节点与目录链。 */
  node?: TreeNode;
  dirPath?: string;
  /** type=doc：文档。 */
  doc?: RagDocInfo;
}

/** 各类型行的估算高度（px）——仅首帧兜底，实测后按 key 覆盖。 */
export const ROW_EST: Record<FlatRow['type'], number> = { source: 36, dir: 26, doc: 40, hint: 17 };

/** 数据源分组条目（RagDocTree 的 sources memo 产物）。 */
export interface SourceGroup {
  kind: string;
  label: string;
  docs: RagDocInfo[];
  gitUrl?: string;
  sourceRoot?: string;
}

/** 把一组文档按 relPath 目录链构建子树。 */
export const buildDirTree = (groupDocs: RagDocInfo[]): TreeNode[] => {
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

/** 收集目录子树全部折叠 key（目录的 key = sourceKey + \n + dirPath）。 */
export const collectDirKeys = (nodes: TreeNode[], path: string, sourceKey: string, out: Set<string>) => {
  for (const n of nodes) {
    if (n.doc) continue;
    const dirPath = path ? `${path}/${n.name}` : n.name;
    out.add(`${sourceKey}\n${dirPath}`);
    collectDirKeys(n.children, dirPath, sourceKey, out);
  }
};

/**
 * 摊平可见行。折叠语义（与组件 effCollapsed 一致由调用方传入）：
 * 数据源直属层级 = 目录在前、根级文档在后；目录内部 = 直属文档在前、
 * 子目录在后（文件管理器惯例）。收起的目录产出一条 hint 行（「N 个文件」）。
 */
export const buildFlatRows = (
  sources: SourceGroup[],
  isCollapsed: (key: string) => boolean,
): FlatRow[] => {
  const rows: FlatRow[] = [];
  sources.forEach((src, srcIdx) => {
    const srcKey = `${src.kind}\n${src.label}`;
    rows.push({ key: `src:${srcKey}`, type: 'source', indent: 0, est: ROW_EST.source, srcIdx });
    if (isCollapsed(srcKey)) return;
    const dirTree = buildDirTree(src.docs);
    const srcMeta = {
      srcKey,
      sourceKind: src.kind,
      sourceRoot: src.kind === 'folder' ? (src.sourceRoot || '') : '',
      sourceGitUrl: src.kind === 'git' ? (src.gitUrl || '') : '',
    };
    const pushDir = (n: TreeNode, dirPath: string, indent: number) => {
      const key = `${srcKey}\n${dirPath}`;
      rows.push({ key: `dir:${key}`, type: 'dir', indent, est: ROW_EST.dir, node: n, dirPath, ...srcMeta });
      if (isCollapsed(key)) {
        rows.push({ key: `hint:${key}`, type: 'hint', indent, est: ROW_EST.hint, node: n, dirPath, ...srcMeta });
      }
      return !isCollapsed(key);
    };
    const walk = (nodes: TreeNode[], path: string, indent: number) => {
      // 目录内部：直属文档在前、子目录在后（文件管理器惯例，与原 DirNode 一致）。
      for (const n of nodes) {
        if (n.doc) {
          rows.push({ key: `doc:${n.doc.id}`, type: 'doc', indent, est: ROW_EST.doc, doc: n.doc });
        }
      }
      for (const n of nodes) {
        if (n.doc) continue;
        const dirPath = path ? `${path}/${n.name}` : n.name;
        if (pushDir(n, dirPath, indent)) walk(n.children, dirPath, indent + 14);
      }
    };
    for (const n of dirTree) {
      if (n.doc) continue;
      if (pushDir(n, n.name, 18)) walk(n.children, n.name, 18 + 14);
    }
    for (const n of dirTree) {
      if (!n.doc) continue;
      rows.push({ key: `doc:${n.doc.id}`, type: 'doc', indent: 18, est: ROW_EST.doc, doc: n.doc });
    }
  });
  return rows;
};
