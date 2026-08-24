import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import './index.css';
// Import the i18n configuration
import './i18n';
// Setup fetch interceptors
import './utils/setupInterceptors';
import { loadRuntimeConfig } from './utils/runtime';

/** Remove the splash loading screen (index.html) with a fade-out animation */
function removeSplash() {
  const splash = document.getElementById('splash-loader');
  if (splash) {
    splash.classList.add('fade-out');
    setTimeout(() => splash.remove(), 350);
  }
}

/**
 * Production builds disable the webview context menu (right-click) so packaged
 * users don't get the browser's "Save image / Inspect / Back" menu over app
 * content. Dev keeps it for debugging. Implemented as a capture-phase
 * contextmenu listener so it wins over any component's own handler.
 */
function setupProductionContextMenuGuard() {
  if (process.env.NODE_ENV !== 'production') return;
  document.addEventListener(
    'contextmenu',
    (e) => {
      // Allow the native menu inside editable fields + the dev-rendered code/
      // snippet viewers where the user may legitimately want copy/paste.
      const target = e.target as HTMLElement | null;
      const editable =
        target &&
        (target.isContentEditable ||
          target.tagName === 'INPUT' ||
          target.tagName === 'TEXTAREA' ||
          target.closest('input,textarea,[contenteditable]'));
      if (editable) return;
      e.preventDefault();
    },
    { capture: true },
  );
}

// Load runtime configuration before starting the app
async function initializeApp() {
  try {
    console.log('Loading runtime configuration...');
    const config = await loadRuntimeConfig();
    console.log('Runtime configuration loaded:', config);

    // Store config in window object
    window.__MCPHUB_CONFIG__ = config;
    // Disable the right-click context menu in packaged builds (dev keeps it).
    setupProductionContextMenuGuard();
    // Start React app
    ReactDOM.createRoot(document.getElementById('root')!).render(
      <React.StrictMode>
        <App />
      </React.StrictMode>,
    );
  } catch (error) {
    console.error('Failed to initialize app:', error);

    // Fallback: start app with default config
    console.log('Starting app with default configuration...');
    window.__MCPHUB_CONFIG__ = {
      basePath: '',
      version: 'dev',
      name: 'mcphub',
    };

    ReactDOM.createRoot(document.getElementById('root')!).render(
      <React.StrictMode>
        <App />
      </React.StrictMode>,
    );
  }

  // Hide splash once React has mounted
  removeSplash();
}

// Initialize the app
initializeApp();