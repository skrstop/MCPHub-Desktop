import { invoke } from '@tauri-apps/api/core';
import { isTauri } from '@/utils/tauriClient';

/**
 * Open an external URL in the user's default browser.
 *
 * Tauri 2's webview does not reliably open `target="_blank"` http(s) links in
 * the system browser on its own (the inner webview has no navigation target),
 * so links rendered as plain `<a target="_blank">` appear to do nothing.
 * Instead we route through our own `open_external_url` Tauri command (defined
 * in src-tauri/src/tray.rs), which spawns the OS opener directly — no shell
 * plugin JS package or capability scope needed beyond the command registration.
 *
 * On web (non-Tauri) this falls back to `window.open` so the same call sites
 * work in the browser preview.
 */
export const openExternal = async (url: string): Promise<void> => {
  if (isTauri()) {
    try {
      await invoke('open_external_url', { url });
      return;
    } catch (e) {
      console.warn('[openExternal] command failed, falling back to window.open:', e);
    }
  }
  window.open(url, '_blank', 'noopener,noreferrer');
};
