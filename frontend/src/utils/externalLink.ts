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

/**
 * Document-level fallback interceptor for external links.
 *
 * Replaces the one tauri-plugin-shell used to inject (the plugin was removed:
 * its `open` command spawns via pre_exec/double-fork, which crashes in
 * _malloc_fork_child with mimalloc's malloc-zone override on macOS 26).
 * Bare `<a target="_blank">` links without their own onClick (Markdown,
 * Dashboard, Market pages, Header, ...) are routed through `openExternal`,
 * which spawns the OS opener via posix_spawn - no fork, no crash.
 *
 * Links whose own onClick already called preventDefault (e.g. AboutDialog's
 * handleExternalClick) are skipped, so the URL only opens once.
 *
 * No-op on web: native target="_blank" navigation works there.
 */
export function installExternalLinkInterceptor(): void {
  if (!isTauri()) return;
  document.addEventListener('click', (e) => {
    if (e.defaultPrevented) return;
    let node = e.target as HTMLElement | null;
    while (node) {
      if (node instanceof HTMLAnchorElement) {
        const a = node as HTMLAnchorElement;
        if (a.target === '_blank' && a.href && /^(https?|mailto|tel):/.test(a.href)) {
          e.preventDefault();
          void openExternal(a.href);
        }
        break;
      }
      node = node.parentElement;
    }
  });
}
