import { describe, it, expect, vi, beforeEach } from 'vitest';
import { parseUnifiedDiff } from './diffParse';

describe('parseUnifiedDiff — side-by-side row alignment', () => {
  it('empty diff text yields no rows', () => {
    expect(parseUnifiedDiff('')).toEqual([]);
  });

  it('a trailing newline is a terminator, not an extra rendered line', () => {
    expect(parseUnifiedDiff(' a\n')).toHaveLength(1);
    expect(parseUnifiedDiff(' a\n\n')).toHaveLength(2); // blank line IS content
  });

  it('file headers and hunk headers span both columns as hunk kind', () => {
    const rows = parseUnifiedDiff('--- v1\n+++ v2\n@@ -1,2 +1,2 @@\n a\n');
    expect(rows).toHaveLength(4);
    for (const i of [0, 1, 2]) {
      expect(rows[i].left.kind).toBe('hunk');
      expect(rows[i].right.kind).toBe('hunk');
      expect(rows[i].right.text).toBe(rows[i].left.text);
      expect(rows[i].isChange).toBe(false);
    }
    expect(rows[0].left.text).toBe('--- v1');
    expect(rows[2].left.text).toBe('@@ -1,2 +1,2 @@');
  });

  it('context lines render on both sides with the leading space stripped', () => {
    const rows = parseUnifiedDiff(' hello\n');
    expect(rows).toHaveLength(1);
    expect(rows[0].left).toEqual({ kind: 'ctx', text: 'hello' });
    expect(rows[0].right).toEqual({ kind: 'ctx', text: 'hello' });
    expect(rows[0].isChange).toBe(false);
  });

  it('a plain (unprefixed) line is treated as context verbatim', () => {
    const rows = parseUnifiedDiff('plain line\n');
    expect(rows[0].left).toEqual({ kind: 'ctx', text: 'plain line' });
  });

  it('one deletion + one addition pair into a single change row', () => {
    const rows = parseUnifiedDiff('-old\n+new\n');
    expect(rows).toHaveLength(1);
    expect(rows[0].left).toEqual({ kind: 'del', text: 'old' });
    expect(rows[0].right).toEqual({ kind: 'add', text: 'new' });
    expect(rows[0].isChange).toBe(true);
  });

  it('a pure addition pads the left side with an empty cell', () => {
    const rows = parseUnifiedDiff('+added\n');
    expect(rows).toHaveLength(1);
    expect(rows[0].left).toEqual({ kind: 'empty', text: '' });
    expect(rows[0].right).toEqual({ kind: 'add', text: 'added' });
    expect(rows[0].isChange).toBe(true);
  });

  it('a pure deletion pads the right side with an empty cell', () => {
    const rows = parseUnifiedDiff('-removed\n');
    expect(rows).toHaveLength(1);
    expect(rows[0].left).toEqual({ kind: 'del', text: 'removed' });
    expect(rows[0].right).toEqual({ kind: 'empty', text: '' });
    expect(rows[0].isChange).toBe(true);
  });

  it('a del-run followed by an add-run pairs row-by-row and pads the shorter side', () => {
    // 2 deletions vs 1 addition: row 1 pairs, row 2 left-only.
    const rows = parseUnifiedDiff('-a1\n-a2\n+b1\n ctx\n');
    expect(rows).toHaveLength(3);
    expect(rows[0]).toEqual({
      left: { kind: 'del', text: 'a1' },
      right: { kind: 'add', text: 'b1' },
      isChange: true,
    });
    expect(rows[1].left).toEqual({ kind: 'del', text: 'a2' });
    expect(rows[1].right).toEqual({ kind: 'empty', text: '' });
    expect(rows[1].isChange).toBe(true);
    expect(rows[2].isChange).toBe(false);
  });

  it('an add-run longer than the del-run pads the left side', () => {
    const rows = parseUnifiedDiff('-a1\n+b1\n+b2\n');
    expect(rows).toHaveLength(2);
    expect(rows[1].left).toEqual({ kind: 'empty', text: '' });
    expect(rows[1].right).toEqual({ kind: 'add', text: 'b2' });
  });

  it('parses a full realistic diff end to end', () => {
    const diff = [
      '--- v3',
      '+++ current',
      '@@ -1,6 +1,7 @@',
      ' # 接入指南',
      ' ',
      '-旧版说明文字',
      '+新版说明文字',
      ' ',
      ' ## 接入步骤',
      ' 1. 获取访问端点',
      '+2. 配置鉴权',
      ' 3. 发起 initialize 握手',
    ].join('\n');
    const rows = parseUnifiedDiff(diff);
    // 2 headers + 1 hunk + 2 ctx + 1 paired change + 1 ctx + 2 ctx + 1 add + 1 ctx
    expect(rows).toHaveLength(11);
    const changes = rows.filter((r) => r.isChange);
    expect(changes).toHaveLength(2);
    expect(changes[0].left.text).toBe('旧版说明文字');
    expect(changes[0].right.text).toBe('新版说明文字');
    expect(changes[1].left.kind).toBe('empty');
    expect(changes[1].right.text).toBe('2. 配置鉴权');
  });
});
