import React, {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { useTranslation } from 'react-i18next';
import { ChangelogUpdateInfo } from '@/types';
import {
  buildChangelogFromTauriUpdate,
  fetchChangelogUpdateInfo,
  shouldShowUpdateBadge,
} from '@/services/changelogService';
import { checkForAppUpdate, logUpdateEvent } from '@/utils/version';
import { isTauri } from '@/utils/tauriClient';
import AboutDialog from '@/components/ui/AboutDialog';

/**
 * The app version displayed in the About dialog. On desktop this is the Tauri
 * app version (PACKAGE_VERSION, defined from tauri.conf.json by vite). On web it
 * falls back to the bundler-resolved version (same define).
 */
const APP_VERSION = (import.meta.env.PACKAGE_VERSION as string) || 'dev';

interface UpdateCheckContextValue {
  /** Latest update info from the startup check (null = not checked / no data). */
  updateInfo: ChangelogUpdateInfo | null;
  /** Whether a new version was detected — drives the menu badge. */
  showUpdateBadge: boolean;
  /** Open the About dialog (e.g. from the user menu "关于" button). */
  openAbout: () => void;
}

const UpdateCheckContext = createContext<UpdateCheckContextValue | null>(null);

/**
 * Provider that runs the application update check once on startup — independent
 * of route/auth — and renders a single root-level About dialog. The check uses
 * the real Tauri updater on desktop; the changelog API is only used on web.
 *
 * The About dialog is rendered here (not in the user menu) so the startup
 * auto-open can show it even before the user is authenticated / on the login
 * page, and so there is exactly one dialog instance app-wide.
 */
export const UpdateCheckProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const { i18n } = useTranslation();
  const [updateInfo, setUpdateInfo] = useState<ChangelogUpdateInfo | null>(null);
  const [showAbout, setShowAbout] = useState(false);
  // Guard so the startup check only auto-opens the dialog once per session.
  const autoOpenedRef = useRef(false);

  const refreshBadge = useCallback((info: ChangelogUpdateInfo | null) => {
    setUpdateInfo(info);
  }, []);

  // Startup update check — runs on mount, regardless of login state.
  //
  // NOTE: no run-once guard here. Under React StrictMode (dev) effects
  // double-invoke: setup1 -> cleanup (cancelled=true) -> setup2. A run-once
  // guard would make setup2 bail, leaving setup1's in-flight check as the only
  // one — but its `cancelled` is already true, so it returns before setting
  // state / opening the dialog. That left the check running (logs appeared) with
  // NO badge and NO auto-open in dev. Dropping the guard lets the surviving
  // (non-cancelled) StrictMode run perform the check and update state. The
  // `autoOpenedRef` still caps auto-open at once per session.
  useEffect(() => {
    let cancelled = false;
    const run = async () => {
      try {
        if (isTauri()) {
          const update = await checkForAppUpdate('startup');
          if (cancelled) return;
          if (update) {
            const info = buildChangelogFromTauriUpdate(update);
            setUpdateInfo(info);
            // Always prompt the user when a new version is found — there is no
            // "dismiss" action; the choice to update is the user's. Auto-open the
            // About dialog once per session so the prompt isn't re-shown on
            // every effect re-run (e.g. on language change).
            const willOpen = !autoOpenedRef.current;
            logUpdateEvent(
              'info',
              `[update] startup result: new version ${update.version}, autoOpened=${willOpen}`,
            );
            if (willOpen) {
              autoOpenedRef.current = true;
              setShowAbout(true);
            }
          } else {
            // Up to date (or check returned nothing) — still record it as a
            // completed check so opening the About dialog shows "已是最新版本"
            // directly instead of re-running the check on every open.
            setUpdateInfo({
              hasUpdate: false,
              latestVersion: '',
              entries: [],
              totalUpdateCount: 0,
              source: 'no-update',
            });
          }
        } else {
          // Web: use the changelog API (server-side update detection).
          const info = await fetchChangelogUpdateInfo({
            currentVersion: APP_VERSION,
            locale: i18n.language,
          });
          if (cancelled) return;
          setUpdateInfo(info);
        }
      } catch (error) {
        console.error('[UpdateCheckProvider] startup update check failed:', error);
      }
    };
    run();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Keep the native tray/app menu language in sync with the frontend i18n
  // (the menu strings come from the same locale files, resolved in Rust via
  // the `set_menu_language` command). Runs at startup and on every language
  // change (i18n.language is stable; the language switcher reloads the page,
  // so startup covers switches too).
  useEffect(() => {
    if (!isTauri()) return;
    import('@tauri-apps/api/core')
      .then(({ invoke }) => invoke('set_menu_language', { lang: i18n.language }))
      .catch((e) => console.warn('[UpdateCheckProvider] set_menu_language failed:', e));
  }, [i18n.language]);

  // "Check for Updates" / "About" from the tray icon menu and the native app
  // menu: the Rust side has already shown + focused the main window; open the
  // About dialog. Both events behave identically here - the dialog shows
  // existing update info directly and only checks when it has none yet.
  useEffect(() => {
    if (!isTauri()) return;
    let unlistenCheck: (() => void) | undefined;
    let unlistenAbout: (() => void) | undefined;
    let cancelled = false;
    import('@tauri-apps/api/event')
      .then(async ({ listen }) => {
        const open = () => setShowAbout(true);
        const fns = await Promise.all([
          listen('updater://check-update', open),
          listen('updater://open-about', open),
        ]);
        if (cancelled) fns.forEach((fn) => fn());
        else {
          [unlistenCheck, unlistenAbout] = fns;
        }
      })
      .catch((e) => console.warn('[UpdateCheckProvider] menu listener failed:', e));
    return () => {
      cancelled = true;
      unlistenCheck?.();
      unlistenAbout?.();
    };
  }, []);

  const openAbout = useCallback(() => {
    // Opened from the user profile menu "关于" — the badge already reflects
    // the startup check's result, so show what we have without re-checking
    // (AboutDialog still checks itself when no valid info exists yet, e.g.
    // the startup check failed or returned no data).
    setShowAbout(true);
  }, []);

  const showUpdateBadge = useMemo(
    () => shouldShowUpdateBadge(updateInfo),
    [updateInfo],
  );

  const value = useMemo<UpdateCheckContextValue>(
    () => ({ updateInfo, showUpdateBadge, openAbout }),
    [updateInfo, showUpdateBadge, openAbout],
  );

  return (
    <UpdateCheckContext.Provider value={value}>
      {children}
      <AboutDialog
        isOpen={showAbout}
        onClose={() => setShowAbout(false)}
        version={APP_VERSION}
        initialUpdateInfo={updateInfo}
        onUpdateInfoChange={refreshBadge}
      />
    </UpdateCheckContext.Provider>
  );
};

/** Access the update-check context. Throws if used outside the provider. */
export const useUpdateCheck = (): UpdateCheckContextValue => {
  const ctx = useContext(UpdateCheckContext);
  if (!ctx) throw new Error('useUpdateCheck must be used within UpdateCheckProvider');
  return ctx;
};

export default UpdateCheckProvider;
