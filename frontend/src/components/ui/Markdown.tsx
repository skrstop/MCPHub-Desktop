import React from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';

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
  const blockComponents = {
    h1: ({ node, ...props }) => (
      <h1 className="text-[15px] font-medium" style={{ color: 'var(--hub-ink)' }} {...props} />
    ),
    h2: ({ node, ...props }) => (
      <h2 className="text-[14px] font-medium mt-3" style={{ color: 'var(--hub-ink)' }} {...props} />
    ),
    h3: ({ node, ...props }) => (
      <h3 className="text-[13.5px] font-medium mt-3" style={{ color: 'var(--hub-ink)' }} {...props} />
    ),
    p: ({ node, ...props }) => <p {...props} />,
    a: ({ node, ...props }) => (
      <a
        className="underline underline-offset-2"
        style={{ color: 'var(--hub-accent)' }}
        target="_blank"
        rel="noopener noreferrer"
        {...props}
      />
    ),
    ul: ({ node, ...props }) => <ul className="list-disc pl-5 space-y-1" {...props} />,
    ol: ({ node, ...props }) => <ol className="list-decimal pl-5 space-y-1" {...props} />,
    li: ({ node, ...props }) => <li {...props} />,
    strong: ({ node, ...props }) => (
      <strong className="font-medium" style={{ color: 'var(--hub-ink)' }} {...props} />
    ),
    code: ({ node, ...props }) => (
      <code
        className="hub-mono px-1 py-0.5 rounded text-[12px]"
        style={{ background: 'var(--hub-bg-2)' }}
        {...props}
      />
    ),
    pre: ({ node, ...props }) => (
      <pre
        className="hub-mono text-[12px] p-3 rounded overflow-x-auto"
        style={{ background: 'var(--hub-bg-2)' }}
        {...props}
      />
    ),
    hr: ({ node, ...props }) => (
      <hr className="border-0 border-t" style={{ borderColor: 'var(--hub-line)' }} {...props} />
    ),
    blockquote: ({ node, ...props }) => (
      <blockquote
        className="pl-3 border-l-2"
        style={{ borderColor: 'var(--hub-line)' }}
        {...props}
      />
    ),
    table: ({ node, ...props }) => <table className="w-full border-collapse text-[12.5px]" {...props} />,
    th: ({ node, ...props }) => (
      <th className="px-2 py-1 text-left border" style={{ borderColor: 'var(--hub-line)' }} {...props} />
    ),
    td: ({ node, ...props }) => (
      <td className="px-2 py-1 border" style={{ borderColor: 'var(--hub-line)' }} {...props} />
    ),
  };

  // Inline mode: flatten block wrappers so the content sits inline (e.g. inside
  // a <li> next to a bullet). Only p/h tags are flattened; inline markdown
  // (bold, links, code) still renders normally.
  const inlineComponents = {
    ...blockComponents,
    p: ({ node, ...props }) => <span {...props} />,
    h1: ({ node, ...props }) => <span className="font-medium" {...props} />,
    h2: ({ node, ...props }) => <span className="font-medium" {...props} />,
    h3: ({ node, ...props }) => <span className="font-medium" {...props} />,
    ul: ({ node, ...props }) => <span {...props} />,
    ol: ({ node, ...props }) => <span {...props} />,
  };

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
        components={inline ? inlineComponents : blockComponents}
      >
        {children}
      </ReactMarkdown>
    </Wrapper>
  );
};

export default Markdown;
