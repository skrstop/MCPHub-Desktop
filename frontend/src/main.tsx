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

// Load runtime configuration before starting the app
async function initializeApp() {
  try {
    console.log('Loading runtime configuration...');
    const config = await loadRuntimeConfig();
    console.log('Runtime configuration loaded:', config);

    // Store config in window object
    window.__MCPHUB_CONFIG__ = config;

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