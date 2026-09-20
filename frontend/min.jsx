import React, { useState, StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

const ALL = new Set(['/a.md', '/b.md']);
function App() {
  const [sel, setSel] = useState(null); // null = all
  const toggle = (p) => {
    const base = sel ?? ALL;
    const next = new Set(base);
    if (next.has(p)) next.delete(p); else next.add(p);
    setSel(next);
    console.log('[min] toggle', p, '->', [...next]);
  };
  return (
    <div>
      {[...ALL].map((p) => (
        <div key={p} role="option" aria-selected={sel === null ? true : sel.has(p)}
          onClick={() => toggle(p)} style={{ padding: 8, cursor: 'pointer', border: '1px solid #ccc' }}>
          {p}
        </div>
      ))}
    </div>
  );
}
createRoot(document.getElementById('root')).render(<StrictMode><App /></StrictMode>);
window.__minReady = true;
