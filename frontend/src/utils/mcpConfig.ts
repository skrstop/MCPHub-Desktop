/**
 * Builds a ready-to-paste MCP client config (mcpServers block) pointing at a
 * hub endpoint URL — drop it into Claude Desktop / Cursor. When bearer auth is
 * enabled, the auth header is included:
 *  - exactly one enabled key → fill its real token (paste-and-use);
 *  - multiple keys (or none) → placeholder `<your-bearer-token>`, because we
 *    can't pick for the user — keys have different access scopes, and the list
 *    is created_at DESC so silently picking the newest would be arbitrary (and
 *    leaking a real token into a shareable config is unsafe).
 *
 * Shared by the Dashboard endpoint rows and the SettingsPage SMART endpoint
 * (both render it via `EndpointCopy`'s per-row config-copy button).
 */
export type BearerRoutingInfo = {
  enableBearerAuth?: boolean;
  bearerAuthHeaderName?: string;
};

/** Enabled bearer keys with a token, in the order received (created_at DESC). */
const enabledKeys = (bearerKeys?: { token: string; enabled: boolean }[]) =>
  (bearerKeys ?? []).filter((k) => k.enabled && k.token);

/**
 * True when the copied config uses a `<your-bearer-token>` placeholder (i.e.
 * auth is on AND there isn't exactly one enabled key). Callers use this to pick
 * a toast that tells the user to substitute their own token.
 */
export const mcpConfigUsesTokenPlaceholder = (
  routing?: BearerRoutingInfo,
  bearerKeys?: { token: string; enabled: boolean }[],
): boolean =>
  !!routing?.enableBearerAuth && enabledKeys(bearerKeys).length !== 1;

export const buildMcpClientConfig = (
  url: string,
  routing?: BearerRoutingInfo,
  bearerKeys?: { token: string; enabled: boolean }[],
): string => {
  const server: Record<string, unknown> = { url, type: 'streamable-http' };
  if (routing?.enableBearerAuth) {
    const headerName = routing.bearerAuthHeaderName || 'Authorization';
    const keys = enabledKeys(bearerKeys);
    if (keys.length === 1) {
      // Single key: fill the real token so the config is usable as-is.
      server.headers = { [headerName]: `Bearer ${keys[0].token}` };
    } else {
      // Multiple keys (or none): don't pick for the user — placeholder.
      server.headers = { [headerName]: 'Bearer <your-bearer-token>' };
    }
  }
  return JSON.stringify({ mcpServers: { mcphub: server } }, null, 2);
};
