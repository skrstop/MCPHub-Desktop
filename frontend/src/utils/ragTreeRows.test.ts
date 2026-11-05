import { describe, it, expect } from 'vitest';
import { buildDirTree, buildFlatRows, collectDirKeys, ROW_EST, type SourceGroup } from './ragTreeRows';
import type { RagDocInfo } from '@/types';

const doc = (id: string, name: string, relPath = '', uploadedAt = '2026-01-01T00:00:00Z'): RagDocInfo => ({
  id,
  name,
  size: 1,
  uploadedAt,
  tags: [],
  chunkCount: 0,
  fileType: '',
  version: 1,
  fileName: '',
  relPath: relPath || undefined,
});

const src = (kind: string, label: string, docs: RagDocInfo[], extra: Partial<SourceGroup> = {}): SourceGroup => ({
  kind,
  label,
  docs,
  ...extra,
});

describe('buildDirTree', () => {
  it('nests docs by relPath directory chain', () => {
    const tree = buildDirTree([
      doc('1', 'a.md', 'docs/guide'),
      doc('2', 'b.md', 'docs'),
      doc('3', 'c.md', ''),
    ]);
    // root: dir "docs" first, then root-level doc
    expect(tree).toHaveLength(2);
    expect(tree[0].name).toBe('docs');
    expect(tree[0].children.map((c) => c.name)).toEqual(['guide', 'b.md']);
    expect(tree[0].children[0].children[0].doc?.id).toBe('1');
    expect(tree[1].doc?.id).toBe('3');
  });

  it('sorts dirs by name and files by uploadedAt desc', () => {
    const tree = buildDirTree([
      doc('old', 'old.md', '', '2025-01-01'),
      doc('new', 'new.md', '', '2026-06-01'),
      doc('x', 'x.md', 'zeta'),
      doc('y', 'y.md', 'alpha'),
    ]);
    const files = tree.filter((n) => n.doc).map((n) => n.doc!.id);
    expect(files).toEqual(['new', 'old']);
    const dirs = tree.filter((n) => !n.doc).map((n) => n.name);
    expect(dirs).toEqual(['alpha', 'zeta']);
  });
});

describe('buildFlatRows', () => {
  const sources: SourceGroup[] = [
    src('git', 'repo', [
      doc('d1', 'a.md', 'sub'),
      doc('d2', 'b.md', ''),
    ], { gitUrl: 'https://x/repo.git' }),
  ];

  it('emits source row, then dirs, then root docs (source level)', () => {
    const rows = buildFlatRows(sources, () => false);
    expect(rows.map((r) => r.type)).toEqual(['source', 'dir', 'doc', 'doc']);
    expect(rows[0].key).toBe('src:git\nrepo');
    expect(rows[0].srcIdx).toBe(0);
    // dir "sub"（含其子树内容）先渲染，根级文档在后（与原实现一致）
    expect(rows[1].dirPath).toBe('sub');
    expect(rows[2].doc?.id).toBe('d1');
    expect(rows[3].doc?.id).toBe('d2');
  });

  it('orders docs before subdirs inside a directory', () => {
    const rows = buildFlatRows([
      src('folder', 'root', [
        doc('deep', 'deep.md', 'parent/child'),
        doc('direct', 'direct.md', 'parent'),
      ], { sourceRoot: '/data' }),
    ], () => false);
    const types = rows.map((r) => `${r.type}:${r.key}`);
    // source → dir parent → doc direct → dir child → doc deep
    expect(types).toEqual([
      'source:src:folder\nroot',
      'dir:dir:folder\nroot\nparent',
      'doc:doc:direct',
      'dir:dir:folder\nroot\nparent/child',
      'doc:doc:deep',
    ]);
  });

  it('collapses a source to a single row', () => {
    const rows = buildFlatRows(sources, (key) => key === 'git\nrepo');
    expect(rows).toHaveLength(1);
    expect(rows[0].type).toBe('source');
  });

  it('collapsed dir emits dir + hint rows and prunes children', () => {
    const rows = buildFlatRows(sources, (key) => key === 'git\nrepo\nsub');
    expect(rows.map((r) => r.type)).toEqual(['source', 'dir', 'hint', 'doc']);
    expect(rows[2].key).toBe('hint:git\nrepo\nsub');
    expect(rows[2].node?.name).toBe('sub');
    // deep doc d1 pruned; only root doc remains
    expect(rows.filter((r) => r.type === 'doc').map((r) => r.doc!.id)).toEqual(['d2']);
  });

  it('accumulates indent: source children 18, nested dirs +14', () => {
    const rows = buildFlatRows([
      src('folder', 'r', [doc('a', 'a.md', 'p/c'), doc('b', 'b.md', 'p')], { sourceRoot: '/x' }),
    ], () => false);
    const byKey = new Map(rows.map((r) => [r.type === 'doc' ? r.doc!.id : r.dirPath, r]));
    expect(byKey.get('p')!.indent).toBe(18);
    expect(byKey.get('p/c')!.indent).toBe(32);
    // 缩进逐层嵌套累计（原实现：每层目录子容器 paddingLeft 14）
    expect(byKey.get('a')!.indent).toBe(46);
    expect(byKey.get('b')!.indent).toBe(32);
  });

  it('carries source metadata on dir rows for refresh/exclude buttons', () => {
    const rows = buildFlatRows(sources, () => false);
    const dir = rows.find((r) => r.type === 'dir')!;
    expect(dir.sourceKind).toBe('git');
    expect(dir.sourceGitUrl).toBe('https://x/repo.git');
    expect(dir.sourceRoot).toBe('');
    expect(dir.srcKey).toBe('git\nrepo');
  });

  it('uses per-type estimates and unique keys', () => {
    const rows = buildFlatRows(sources, () => false);
    for (const r of rows) expect(r.est).toBe(ROW_EST[r.type]);
    const keys = new Set(rows.map((r) => r.key));
    expect(keys.size).toBe(rows.length);
  });
});

describe('collectDirKeys', () => {
  it('collects every directory key with sourceKey prefix', () => {
    const tree = buildDirTree([doc('1', 'a.md', 'p/c')]);
    const out = new Set<string>();
    collectDirKeys(tree, '', 'git\nrepo', out);
    expect(out).toEqual(new Set(['git\nrepo\np', 'git\nrepo\np/c']));
  });
});
