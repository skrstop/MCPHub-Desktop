/** Pure helpers for the ServersPage relevance-search dialog (unit tested).
 *  Scope values are `all` | `group:{id}` | `server:{name}` — the prefix is
 *  sent to the backend VERBATIM and parsed there (resolve_search_scope),
 *  so a server named "all" or containing colons resolves correctly. */

export interface SmartSearchScopeOption {
  value: string;
  label: string;
  kind: 'all' | 'group' | 'server';
}

export interface SmartSearchScopeLabels {
  all: string;
  group: string;
  server: string;
}

export interface SmartSearchScopeGroup {
  id: string;
  name: string;
}

export interface SmartSearchScopeServer {
  name: string;
}

/** Build the scope dropdown options: all first, then groups, then servers. */
export const buildSmartSearchScopes = (
  groups: SmartSearchScopeGroup[],
  servers: SmartSearchScopeServer[],
  labels: SmartSearchScopeLabels,
): SmartSearchScopeOption[] => {
  const opts: SmartSearchScopeOption[] = [
    { value: 'all', label: labels.all, kind: 'all' },
  ];
  for (const g of groups) {
    opts.push({
      value: `group:${g.id}`,
      label: `${labels.group}: ${g.name}`,
      kind: 'group',
    });
  }
  for (const s of servers) {
    opts.push({
      value: `server:${s.name}`,
      label: `${labels.server}: ${s.name}`,
      kind: 'server',
    });
  }
  return opts;
};

/** The scope value is sent to the backend verbatim (prefix included):
 *  `all` | `group:{id}` | `server:{name}`. The Rust side parses the prefix
 *  explicitly (resolve_search_scope), so a server named "all" or containing
 *  colons resolves correctly. Kept as a named function so the contract has a
 *  single seam. */
export const scopeForRequest = (scope: string): string => scope;

/** Map known backend error sentinels to i18n keys; anything else (unexpected
 *  upstream text) falls back to the raw string. The sentinels must match the
 *  Rust `anyhow!` literals verbatim — pinned by smartSearchSentinels.test.ts
 *  against the Rust sources. */
export const SMART_SEARCH_SENTINELS = {
  mvNotRunning: 'mv runtime not running',
  disabled: 'smart routing is not enabled',
} as const;

export const localizeSmartSearchError = (
  message: string,
  t: (key: string) => string,
): string => {
  if (message.includes(SMART_SEARCH_SENTINELS.mvNotRunning)) {
    return t('server.smartSearch.errMvNotRunning');
  }
  if (message.includes(SMART_SEARCH_SENTINELS.disabled)) {
    return t('server.smartSearch.errDisabled');
  }
  return message;
};
