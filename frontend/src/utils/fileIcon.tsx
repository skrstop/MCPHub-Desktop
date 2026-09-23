/**
 * Per-type file icons for the RAG doc list / dialogs. A single mapper so the
 * main list (flat + tree leaf rows), delete dialog and details dialog all
 * show the same glyph for the same file type.
 */
import React from 'react';
import {
  FileCode,
  FileImage,
  FileJson,
  FileSpreadsheet,
  FileText,
  FileType,
  FileArchive,
  Presentation,
  BookOpen,
  FileTerminal,
  File,
} from 'lucide-react';
import { extOf } from './fileType';

const S = (size: number): React.CSSProperties => ({ color: 'var(--hub-ink-3)', flexShrink: 0 });

const MARKDOWN_EXTS = new Set(['.md', '.markdown', '.mdx', '.mdown', '.mkd']);
const ARCHIVE_EXTS = new Set(['.zip', '.gz', '.tar', '.7z', '.rar', '.bz2', '.xz', '.tgz']);
const IMAGE_EXTS = new Set(['.png', '.jpg', '.jpeg', '.gif', '.bmp', '.webp', '.tiff', '.tif', '.svg', '.ico']);
const SHELL_EXTS = new Set(['.sh', '.bash', '.zsh', '.fish', '.ps1', '.bat', '.cmd', '.csh']);
const DOC_EXTS = new Set(['.doc', '.docx', '.odt', '.rtf', '.pages']);
const SHEET_EXTS = new Set(['.xls', '.xlsx', '.ods', '.csv', '.tsv']);
const SLIDE_EXTS = new Set(['.ppt', '.pptx', '.odp', '.key']);
const TEX_EXTS = new Set(['.tex', '.latex', '.sty', '.cls']);
const DATA_EXTS = new Set(['.xml', '.yaml', '.yml', '.toml', '.ini', '.cfg', '.conf', '.env', '.properties', '.plist']);
const FRONTEND_EXTS = new Set(['.html', '.htm', '.vue', '.svelte', '.astro', '.ejs', '.hbs', '.jsx', '.tsx', '.css', '.scss', '.less', '.styl']);
const PYTHON_EXTS = new Set(['.py', '.pyw', '.pyi', '.ipynb']);
const WEB_EXTS = new Set(['.js', '.mjs', '.cjs', '.ts', '.mts', '.cts']);
const C_EXTS = new Set(['.c', '.h', '.cpp', '.cc', '.cxx', '.hpp', '.hh', '.hxx']);
const RUST_EXTS = new Set(['.rs']);
const GO_EXTS = new Set(['.go']);
const JAVA_EXTS = new Set(['.java', '.kt', '.kts', '.scala', '.groovy', '.gradle']);
const DOTNET_EXTS = new Set(['.cs', '.fs', '.fsx', '.vb']);
const SCRIPTING_EXTS = new Set(['.rb', '.php', '.pl', '.lua', '.r', '.jl', '.ex', '.exs', '.dart', '.swift']);

const EXACT_NAME_ICONS: Record<string, (size: number) => React.ReactNode> = {
  dockerfile: (s) => <FileTerminal size={s} style={S(s)} />,
  makefile: (s) => <FileTerminal size={s} style={S(s)} />,
  'cmakelists.txt': (s) => <FileTerminal size={s} style={S(s)} />,
  license: (s) => <BookOpen size={s} style={S(s)} />,
  readme: (s) => <BookOpen size={s} style={S(s)} />,
};

const EXT_ICONS: Record<string, (size: number) => React.ReactNode> = {
  '.md': (s) => <BookOpen size={s} style={S(s)} />,
  '.pdf': (s) => <BookOpen size={s} style={{ color: 'var(--hub-err)', opacity: 0.55, flexShrink: 0 }} />,
  '.json': (s) => <FileJson size={s} style={S(s)} />,
};

/** Pick the icon for a file by name (extension). Falls back to a plain
 *  FileText glyph for unknown/extensionless text files. */
export function fileIcon(fileName?: string, size = 14): React.ReactNode {
  if (!fileName) return <FileText size={size} style={S(size)} />;
  const lower = fileName.toLowerCase();
  const base = lower.split('/').pop() ?? lower;
  const stem = base.replace(/\.[^.]+$/, '');
  const exact = EXACT_NAME_ICONS[base] ?? EXACT_NAME_ICONS[stem];
  if (exact) return exact(size);
  const ext = extOf(fileName);
  const byExt = EXT_ICONS[ext];
  if (byExt) return byExt(size);
  if (ext === '') return <FileText size={size} style={S(size)} />;
  if (MARKDOWN_EXTS.has(ext)) return <BookOpen size={size} style={S(size)} />;
  if (IMAGE_EXTS.has(ext)) return <FileImage size={size} style={S(size)} />;
  if (ARCHIVE_EXTS.has(ext)) return <FileArchive size={size} style={S(size)} />;
  if (SHELL_EXTS.has(ext)) return <FileTerminal size={size} style={S(size)} />;
  if (DOC_EXTS.has(ext)) return <FileText size={size} style={S(size)} />;
  if (SHEET_EXTS.has(ext)) return <FileSpreadsheet size={size} style={S(size)} />;
  if (SLIDE_EXTS.has(ext)) return <Presentation size={size} style={S(size)} />;
  if (TEX_EXTS.has(ext)) return <BookOpen size={size} style={S(size)} />;
  if (DATA_EXTS.has(ext)) return <FileJson size={size} style={S(size)} />;
  if (
    FRONTEND_EXTS.has(ext) || PYTHON_EXTS.has(ext) || WEB_EXTS.has(ext) || C_EXTS.has(ext) ||
    RUST_EXTS.has(ext) || GO_EXTS.has(ext) || JAVA_EXTS.has(ext) || DOTNET_EXTS.has(ext) ||
    SCRIPTING_EXTS.has(ext)
  ) return <FileCode size={size} style={S(size)} />;
  return <FileType size={size} style={S(size)} />;
}
