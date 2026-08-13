import React, { useEffect, useRef, useState } from 'react';
import mermaid from 'mermaid';

// Initialize once. `theme: 'dark'` matches the app's dark surfaces;
// `securityLevel: 'loose'` allows click callbacks and full syntax (the RAG
// docs are user-authored local content, not untrusted remote input).
mermaid.initialize({
  startOnLoad: false,
  theme: 'dark',
  securityLevel: 'loose',
  fontFamily: 'inherit',
});

// Monotonic counter for unique render ids (mermaid.render needs a DOM-id-like
// string; React.useId() contains colons which aren't valid ids).
let mermaidIdCounter = 0;

interface MermaidDiagramProps {
  chart: string;
  className?: string;
}

/**
 * Render a Mermaid diagram string (`graph TD ...`, `sequenceDiagram`, ...)
 * into SVG via `mermaid.render`. Async: renders on mount / when `chart`
 * changes; shows the raw source on error so the user can see what failed
 * instead of a blank box.
 *
 * Used by the Markdown component for ```mermaid fenced code blocks. The SVG
 * is injected via dangerouslySetInnerHTML - this is safe because Mermaid
 * generates the SVG from its own parser (no raw HTML pass-through under
 * `securityLevel: 'strict'`; we use 'loose' for click support but the input
 * is local RAG document content, not remote).
 */
const MermaidDiagram: React.FC<MermaidDiagramProps> = ({ chart, className }) => {
  const [svg, setSvg] = useState<string>('');
  const [error, setError] = useState<string>('');
  const idRef = useRef<string>(`mermaid-diagram-${mermaidIdCounter++}`);

  useEffect(() => {
    let cancelled = false;
    const id = idRef.current;
    setError('');
    mermaid
      .render(id, chart)
      .then(({ svg }) => {
        if (!cancelled) setSvg(svg);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        const msg = e instanceof Error ? e.message : String(e);
        setError(msg);
        setSvg('');
      });
    return () => {
      cancelled = true;
    };
  }, [chart]);

  if (error) {
    return (
      <div
        className={
          'hub-mono text-[12px] p-3 rounded whitespace-pre-wrap break-words ' + (className ?? '')
        }
        style={{ color: 'var(--hub-err)', background: 'var(--hub-bg-2)' }}
      >
        Mermaid render error: {error}
        {'\n\n--- source ---\n'}
        {chart}
      </div>
    );
  }

  if (!svg) {
    return (
      <div
        className={'p-3 rounded ' + (className ?? '')}
        style={{ background: 'var(--hub-bg-2)', minHeight: 40 }}
      />
    );
  }

  return (
    <div
      className={className}
      style={{ background: 'var(--hub-bg-2)', padding: 12, borderRadius: 6, overflowX: 'auto' }}
      dangerouslySetInnerHTML={{ __html: svg }}
    />
  );
};

export default MermaidDiagram;
