import React from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import MermaidDiagram from './MermaidDiagram';

interface MarkdownProps {
  children: string;
  className?: string;
  /**
   * Render inline (flatten block elements like `<p>` to `<span>`) so the
   * markdown can sit inside a `<li>` beside a bullet without producing nested
   * block paragraphs. Used for short highlight lines.
   */
  inline?: boolean;
}

/**
 * If a <pre>'s child <code> is a ```mermaid fenced block, extract the chart
 * source so the caller can render it as a diagram instead of a code block.
 * Returns the chart string, or undefined if not mermaid.
 */
function extractMermaid(children: React.ReactNode): string | undefined {
  const codeEl = React.Children.toArray(children)[0];
  if (!React.isValidElement(codeEl)) return undefined;
  const className = (codeEl.props as { className?: string } | undefined)?.className ?? '';
  const match = /language-(\w+)/.exec(className);
  if (!match || match[1] !== 'mermaid') return undefined;
  const inner = (codeEl.props as { children?: React.ReactNode } | undefined)?.children;
  return typeof inner === 'string' ? inner : Array.isArray(inner) ? inner.join('') : String(inner ?? '');
}

// Component maps are module-level (NOT re-created per render). react-markdown
// uses `components` to render AST nodes; if the map (or the component functions
// inside) has a new identity each render, React treats the node types as
// changed and remounts the subtree - which reset the MermaidDiagram's SVG state
// and made diagrams flicker on every parent re-render (e.g. zoom changes).
const PRE: React.FC<{ node?: unknown; children?: React.ReactNode } & React.HTMLAttributes<HTMLPreElement>> = ({
  node,
  children,
  ...props
}) => {
  const mermaidChart = extractMermaid(children);
  if (mermaidChart !== undefined) {
    return <MermaidDiagram chart={mermaidChart} />;
  }
  return (
    <pre
      className="hub-mono text-[12px] p-3 rounded overflow-x-auto"
      style={{ background: 'var(--hub-bg-2)' }}
      {...props}
    >
      {children}
    </pre>
  );
};

const BLOCK_COMPONENTS = {
  h1: ({ node, ...props }: { node?: unknown } & React.HTMLAttributes<HTMLHeadingElement>) => (
    <h1 className="text-[15px] font-medium" style={{ color: 'var(--hub-ink)' }} {...props} />
  ),
  h2: ({ node, ...props }: { node?: unknown } & React.HTMLAttributes<HTMLHeadingElement>) => (
    <h2 className="text-[14px] font-medium mt-3" style={{ color: 'var(--hub-ink)' }} {...props} />
  ),
  h3: ({ node, ...props }: { node?: unknown } & React.HTMLAttributes<HTMLHeadingElement>) => (
    <h3 className="text-[13.5px] font-medium mt-3" style={{ color: 'var(--hub-ink)' }} {...props} />
  ),
  p: ({ node, ...props }: { node?: unknown } & React.HTMLAttributes<HTMLParagraphElement>) => <p {...props} />,
  a: ({ node, ...props }: { node?: unknown } & React.HTMLAttributes<HTMLAnchorElement>) => (
    <a
      className="underline underline-offset-2"
      style={{ color: 'var(--hub-accent)' }}
      target="_blank"
      rel="noopener noreferrer"
      {...props}
    />
  ),
  ul: ({ node, ...props }: { node?: unknown } & React.HTMLAttributes<HTMLUListElement>) => (
    <ul className="list-disc pl-5 space-y-1" {...props} />
  ),
  ol: ({ node, ...props }: { node?: unknown } & React.HTMLAttributes<HTMLOListElement>) => (
    <ol className="list-decimal pl-5 space-y-1" {...props} />
  ),
  li: ({ node, ...props }: { node?: unknown } & React.HTMLAttributes<HTMLLIElement>) => <li {...props} />,
  strong: ({ node, ...props }: { node?: unknown } & React.HTMLAttributes<HTMLElement>) => (
    <strong className="font-medium" style={{ color: 'var(--hub-ink)' }} {...props} />
  ),
  code: ({ node, ...props }: { node?: unknown } & React.HTMLAttributes<HTMLElement>) => (
    <code
      className="hub-mono px-1 py-0.5 rounded text-[12px]"
      style={{ background: 'var(--hub-bg-2)' }}
      {...props}
    />
  ),
  pre: PRE,
  hr: ({ node, ...props }: { node?: unknown } & React.HTMLAttributes<HTMLHRElement>) => (
    <hr className="border-0 border-t" style={{ borderColor: 'var(--hub-line)' }} {...props} />
  ),
  blockquote: ({ node, ...props }: { node?: unknown } & React.HTMLAttributes<HTMLQuoteElement>) => (
    <blockquote className="pl-3 border-l-2" style={{ borderColor: 'var(--hub-line)' }} {...props} />
  ),
  table: ({ node, ...props }: { node?: unknown } & React.HTMLAttributes<HTMLTableElement>) => (
    <table className="w-full border-collapse text-[12.5px]" {...props} />
  ),
  th: ({ node, ...props }: { node?: unknown } & React.HTMLAttributes<HTMLTableCellElement>) => (
    <th className="px-2 py-1 text-left border" style={{ borderColor: 'var(--hub-line)' }} {...props} />
  ),
  td: ({ node, ...props }: { node?: unknown } & React.HTMLAttributes<HTMLTableCellElement>) => (
    <td className="px-2 py-1 border" style={{ borderColor: 'var(--hub-line)' }} {...props} />
  ),
};

// Inline mode: flatten block wrappers so the content sits inline (e.g. inside
// a <li> next to a bullet). Only p/h tags are flattened; inline markdown
// (bold, links, code) still renders normally.
const INLINE_COMPONENTS = {
  ...BLOCK_COMPONENTS,
  p: ({ node, ...props }: { node?: unknown } & React.HTMLAttributes<HTMLSpanElement>) => <span {...props} />,
  h1: ({ node, ...props }: { node?: unknown } & React.HTMLAttributes<HTMLSpanElement>) => (
    <span className="font-medium" {...props} />
  ),
  h2: ({ node, ...props }: { node?: unknown } & React.HTMLAttributes<HTMLSpanElement>) => (
    <span className="font-medium" {...props} />
  ),
  h3: ({ node, ...props }: { node?: unknown } & React.HTMLAttributes<HTMLSpanElement>) => (
    <span className="font-medium" {...props} />
  ),
  ul: ({ node, ...props }: { node?: unknown } & React.HTMLAttributes<HTMLSpanElement>) => <span {...props} />,
  ol: ({ node, ...props }: { node?: unknown } & React.HTMLAttributes<HTMLSpanElement>) => <span {...props} />,
};

/**
 * Render a markdown string as React elements.
 *
 * Used to display release notes (`latest.json` -> `notes`) from the updater.
 * `react-markdown` renders to React nodes and never injects raw HTML, so this
 * is safe for remote-sourced content without DOMPurify / dangerouslySetInnerHTML.
 * GFM (tables, strikethrough, task lists, autolinks) is enabled via remark-gfm.
 *
 * Styling follows the hub design tokens (CSS variables) so it matches the
 * surrounding About dialog.
 */
const Markdown: React.FC<MarkdownProps> = ({ children, className, inline }) => {
  const Wrapper = inline ? 'span' : 'div';

  return (
    <Wrapper
      className={
        inline
          ? className ?? ''
          : `text-[13px] leading-relaxed [&>*:first-child]:mt-0 [&>*:last-child]:mb-0 [&_*+*]:mt-2 ${className ?? ''}`
      }
      style={inline ? undefined : { color: 'var(--hub-ink-2)' }}
    >
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={inline ? INLINE_COMPONENTS : BLOCK_COMPONENTS}
      >
        {children}
      </ReactMarkdown>
    </Wrapper>
  );
};

export default Markdown;
