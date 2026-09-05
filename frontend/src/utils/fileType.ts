/**
 * File-type helpers for the RAG viewer. Determine how to render a document's
 * content (Markdown / syntax-highlighted code / plain text) from its fileType
 * label (backend-persisted) or filename extension.
 *
 * The backend (`src-tauri/runtimes/rag/file_support.json`) is the source of
 * truth for the fileType *label*; here we map extensions to highlight.js
 * language aliases for the code-renderer path. Markdown is handled separately
 * (rendered via the Markdown component, not highlighted as a code block).
 */

// Extension (lowercase, with dot) -> highlight.js language alias.
// `undefined`-valued entries aren't possible in a Record, so omit plain text.
const EXT_TO_HL: Record<string, string> = {
  '.py': 'python', '.pyi': 'python', '.pyw': 'python',
  '.java': 'java',
  '.rs': 'rust',
  '.ts': 'typescript', '.tsx': 'tsx',
  '.js': 'javascript', '.jsx': 'jsx', '.cjs': 'javascript', '.mjs': 'javascript',
  '.json': 'json', '.json5': 'json', '.jsonc': 'json',
  '.yaml': 'yaml', '.yml': 'yaml',
  '.html': 'html', '.htm': 'html',
  '.css': 'css', '.scss': 'scss', '.less': 'less', '.sass': 'sass',
  '.go': 'go',
  '.kt': 'kotlin', '.kts': 'kotlin',
  '.swift': 'swift',
  '.c': 'c', '.h': 'c',
  '.cpp': 'cpp', '.cc': 'cpp', '.cxx': 'cpp', '.hpp': 'cpp', '.hh': 'cpp',
  '.sh': 'bash', '.bash': 'bash', '.zsh': 'bash', '.fish': 'bash',
  '.sql': 'sql',
  '.xml': 'xml', '.svg': 'xml',
  '.toml': 'toml', '.ini': 'ini', '.conf': 'ini', '.cfg': 'ini',
  '.vue': 'vue', '.svelte': 'svelte',
  '.php': 'php',
  '.rb': 'ruby',
  '.scala': 'scala', '.sc': 'scala',
  '.dart': 'dart',
  '.lua': 'lua',
  '.r': 'r',
  '.pl': 'perl', '.pm': 'perl',
  '.tex': 'latex',
  '.graphql': 'graphql', '.gql': 'graphql',
  '.dockerfile': 'dockerfile',
};

// Files with no extension (or special names) -> highlight.js language.
const NAME_TO_HL: Record<string, string> = {
  dockerfile: 'dockerfile',
  makefile: 'makefile',
  gemfile: 'ruby',
  'rakefile': 'ruby',
};

const MARKDOWN_EXTS = new Set(['.md', '.markdown', '.mdx', '.rmd']);

// Desktop-only: these source formats are imported THROUGH text extraction
// (backend parses PDF/Office files to Markdown; see
// src-tauri/src/rag/extract/), so the stored/viewable content is always
// Markdown regardless of the original extension. (Image imports (.png/...)
// also go through extraction, but their OCR output is plain text — rendered
// as such, not Markdown.)
const EXTRACTED_MARKDOWN_EXTS = new Set([
  '.pdf', '.docx', '.doc', '.xlsx', '.xls', '.pptx', '.ppt',
]);
const MERMAID_EXTS = new Set(['.mmd', '.mermaid']);

// Image sources imported through OCR extraction (content = recognized text,
// stored as `{id}.md` like the other extractable kinds).
const IMAGE_EXTRACTED_EXTS = new Set([
  '.png', '.jpg', '.jpeg', '.gif', '.bmp', '.webp', '.tiff', '.tif',
]);

/** Lowercase extension (with dot) of a filename, or '' if none. */
export function extOf(fileName?: string): string {
  if (!fileName) return '';
  const lower = fileName.toLowerCase();
  const dot = lower.lastIndexOf('.');
  return dot >= 0 ? lower.slice(dot) : '';
}

/** Whether the doc should be rendered as Markdown (not as a code block). */
export function isMarkdown(fileName?: string, fileType?: string): boolean {
  if (fileType === 'Markdown') return true;
  const ext = extOf(fileName);
  return MARKDOWN_EXTS.has(ext) || EXTRACTED_MARKDOWN_EXTS.has(ext);
}

/** Whether this file kind is imported via SECOND-PASS parsing (PDF/Office
 *  text extraction, image OCR) — the doc has BOTH a source file
 *  (originalPath) and a derived content file, so "open location"/view can
 *  target either. Drives the two-entry open-folder menu + the
 *  "view source file" button in the View dialog. */
export function isExtractedSource(fileName?: string): boolean {
  const ext = extOf(fileName);
  return EXTRACTED_MARKDOWN_EXTS.has(ext) || IMAGE_EXTRACTED_EXTS.has(ext);
}

/** Whether the doc is a standalone Mermaid diagram (.mmd / .mermaid). */
export function isMermaid(fileName?: string, fileType?: string): boolean {
  if (fileType === 'Mermaid') return true;
  return MERMAID_EXTS.has(extOf(fileName));
}

/**
 * The highlight.js language alias for the doc, or `undefined` if it should be
 * rendered as plain text (no highlighting). Returns `undefined` for Markdown
 * too (the caller renders that via the Markdown component, not a code block).
 */
export function hlLangFor(fileName?: string, fileType?: string): string | undefined {
  // Special whole-filename matches first (Dockerfile, Makefile, ...).
  const base = fileName?.split('/').pop()?.toLowerCase() ?? '';
  if (NAME_TO_HL[base]) return NAME_TO_HL[base];
  const ext = extOf(fileName);
  if (MARKDOWN_EXTS.has(ext) || fileType === 'Markdown') return undefined;
  // Extracted sources render via the Markdown component, never highlighted.
  if (EXTRACTED_MARKDOWN_EXTS.has(ext)) return undefined;
  return EXT_TO_HL[ext];
}
