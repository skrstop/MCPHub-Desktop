import React from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import rehypeHighlight from 'rehype-highlight';
// highlight.js theme CSS - the rehype-highlight plugin only adds token classes
// (hljs-keyword, hljs-string, ...); the colors come from this stylesheet.
// atom-one-dark-reasonable reads well on the app's dark surfaces.
import 'highlight.js/styles/atom-one-dark-reasonable.css';
import Markdown from './Markdown';
import MermaidDiagram from './MermaidDiagram';
import { isMarkdown, isMermaid, hlLangFor } from '@/utils/fileType';

interface FileTypeRendererProps {
  content: string;
  /** Filename (used to derive type when fileType is absent, e.g. search hits). */
  fileName?: string;
  /** Backend-persisted type label ("Markdown", "Java", ...). Takes precedence. */
  fileType?: string;
  /**
   * Compact mode for search snippets: no card padding, tighter line height.
   * Still applies Markdown / code highlighting; the caller constrains height.
   */
  inline?: boolean;
  /**
   * Show the raw source as a <pre> instead of the rendered view (the
   * "view source" toggle in the View dialog).
   */
  raw?: boolean;
  className?: string;
}

/**
 * Render RAG document content with a viewer appropriate to its file type:
 *  - Markdown -> the Markdown component (headings, lists, code, tables, ...)
 *  - Code (python/java/rust/...) -> react-markdown + rehype-highlight on a
 *    fenced ```{lang}``` block, so syntax is colored
 *  - Plain text / unknown -> a <pre> (preserves whitespace, no highlighting)
 *
 * Used by the RAG View dialog (full doc) and the vector-search result list
 * (per-hit snippet). The backend persists `fileType` for docs created via
 * rag_file_create; for uploaded docs and search hits the type is derived from
 * the filename extension.
 */
const FileTypeRenderer: React.FC<FileTypeRendererProps> = ({
  content,
  fileName,
  fileType,
  inline,
  raw,
  className,
}) => {
  // "View source" mode: show the raw text in a <pre>, bypassing all rendering.
  if (raw) {
    return (
      <pre
        className={
          'hub-mono text-[12.5px] whitespace-pre-wrap break-words ' + (className ?? '')
        }
        style={{ color: 'var(--hub-ink-2)' }}
      >
        {content}
      </pre>
    );
  }

  // Markdown: render via the shared Markdown component (headings, tables, ...
  // and ```mermaid blocks -> MermaidDiagram, handled inside Markdown).
  if (isMarkdown(fileName, fileType)) {
    return (
      <Markdown className={className} inline={inline}>
        {content}
      </Markdown>
    );
  }

  // Standalone Mermaid diagram (.mmd / .mermaid): render the whole content.
  if (isMermaid(fileName, fileType)) {
    return <MermaidDiagram chart={content} className={className} />;
  }

  const lang = hlLangFor(fileName, fileType);

  // Code: wrap in a fenced code block with a language hint so rehype-highlight
  // picks the right grammar. `ignoreMissing: true` falls back gracefully if the
  // language isn't registered; `detect: true` lets it auto-detect when the hint
  // is absent.
  if (lang) {
    const fenced = '```' + lang + '\n' + content + '\n```';
    return (
      <div className={className} style={inline ? { margin: 0 } : undefined}>
        <ReactMarkdown
          remarkPlugins={[remarkGfm]}
          rehypePlugins={[[rehypeHighlight, { detect: true, ignoreMissing: true }]]}
          components={{
            // Inline snippets want no card padding; full view wants the padded card look.
            pre: ({ node, ...props }) => (
              <pre
                className={
                  'hub-mono text-[12.5px] overflow-x-auto ' +
                  (inline ? 'm-0' : 'p-3 rounded')
                }
                style={{
                  background: 'var(--hub-bg-2)',
                  color: 'var(--hub-ink-2)',
                }}
                {...props}
              />
            ),
            code: ({ node, ...props }) => <code className="hub-mono" {...props} />,
          }}
        >
          {fenced}
        </ReactMarkdown>
      </div>
    );
  }

  // Plain text / unknown type: preserve whitespace, no highlighting.
  return (
    <pre
      className={
        'hub-mono text-[12.5px] whitespace-pre-wrap break-words ' + (className ?? '')
      }
      style={{ color: 'var(--hub-ink-2)' }}
    >
      {content}
    </pre>
  );
};

export default FileTypeRenderer;
