import i18n from '@/i18n';
import { apiPut } from '@/utils/fetchInterceptor';

/** Persist the current UI language to `system_config.mcp.uiLanguage` so the
 *  backend can resolve `mcp.toolLanguage: "auto"` (builtin tool description
 *  language follows the interface language). Best-effort: the endpoint is
 *  admin-gated — a non-admin session's sync fails silently and "auto" falls
 *  back to English server-side. */
export function initUiLanguageSync(): void {
  const sync = (lng: string) => {
    const primary = (lng || 'en').split('-')[0].toLowerCase();
    apiPut('system-config', { mcp: { uiLanguage: primary } }).catch(() => undefined);
  };
  sync(i18n.language || 'en');
  i18n.on('languageChanged', sync);
}
