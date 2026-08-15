import React, { useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { isTauri } from '@/utils/tauriClient';

/**
 * Listens for `nav://navigate` events emitted by the Rust side (tray icon
 * menu / native app menu items like "Settings") and navigates to the
 * requested route. Must render INSIDE the Router — that's why it's a
 * standalone component rather than living in UpdateCheckProvider, which
 * mounts outside it.
 *
 * Payload is a route without the leading slash (e.g. "settings"). Unauthen-
 * ticated users are handled by ProtectedRoute (redirected to /login, and the
 * navigation happens post-login when they click the menu again).
 */
const NativeMenuNavListener: React.FC = () => {
  const navigate = useNavigate();

  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    import('@tauri-apps/api/event')
      .then(({ listen }) =>
        listen<string>('nav://navigate', (e) => {
          const route = e.payload?.replace(/^\/+/, '');
          if (route) navigate(`/${route}`);
        }),
      )
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch((e) => console.warn('[NativeMenuNavListener] listener failed:', e));
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [navigate]);

  return null;
};

export default NativeMenuNavListener;
