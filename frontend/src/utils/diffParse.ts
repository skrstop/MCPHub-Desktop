/**
 * Unified-diff → side-by-side row parser for the RAG history DiffDialog.
 *
 * Pure function (no DOM) so the alignment/pairing rules are unit-testable:
 * within each hunk a consecutive run of deletions is paired row-by-row with
 * the following run of additions (shorter side padded with empty cells);
 * context lines render on both sides; `@@` hunk headers and `---/+++` file
 * headers span both columns.
 */

export type DiffSideKind = 'del' | 'add' | 'ctx' | 'hunk' | 'empty';
export interface DiffSide {
  kind: DiffSideKind;
  text: string;
}
export interface DiffRow {
  left: DiffSide;
  right: DiffSide;
  isChange: boolean;
}

const emptySide = (): DiffSide => ({ kind: 'empty', text: '' });

export function parseUnifiedDiff(diffText: string): DiffRow[] {
  if (diffText === '') return [];
  const out: DiffRow[] = [];
  const lines = diffText.split('\n');
  // A trailing newline produces a final empty element — drop it (it is the
  // line terminator, not a rendered line).
  if (lines.length > 1 && lines[lines.length - 1] === '') {
    lines.pop();
  }
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (line.startsWith('@@') || line.startsWith('---') || line.startsWith('+++')) {
      const head: DiffSide = { kind: 'hunk', text: line };
      out.push({ left: head, right: { ...head }, isChange: false });
      i += 1;
      continue;
    }
    // Collect a consecutive run of deletions, then a run of additions, and
    // pair them row-by-row (shorter side padded with empty cells).
    if (line.startsWith('-') || line.startsWith('+')) {
      const dels: string[] = [];
      const adds: string[] = [];
      while (i < lines.length && lines[i].startsWith('-')) {
        dels.push(lines[i].slice(1));
        i += 1;
      }
      while (i < lines.length && lines[i].startsWith('+')) {
        adds.push(lines[i].slice(1));
        i += 1;
      }
      const n = Math.max(dels.length, adds.length);
      for (let k = 0; k < n; k += 1) {
        const l = k < dels.length ? { kind: 'del' as const, text: dels[k] } : emptySide();
        const r = k < adds.length ? { kind: 'add' as const, text: adds[k] } : emptySide();
        out.push({ left: l, right: r, isChange: true });
      }
      continue;
    }
    // Context (or any unparsed line) renders on both sides.
    const text = line.startsWith(' ') ? line.slice(1) : line;
    const ctx: DiffSide = { kind: 'ctx', text };
    out.push({ left: ctx, right: { ...ctx }, isChange: false });
    i += 1;
  }
  return out;
}
