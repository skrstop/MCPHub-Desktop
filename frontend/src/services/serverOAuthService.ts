import { apiGet, apiPost } from '../utils/fetchInterceptor';

export type OAuthDisconnectScope = 'tokens' | 'all';

export const disconnectServerOAuth = (
  serverName: string,
  scope: OAuthDisconnectScope = 'tokens',
) =>
  apiPost(`/servers/${encodeURIComponent(serverName)}/oauth/disconnect`, {
    scope,
  });

/**
 * Start the upstream OAuth authorization flow (desktop): the Rust side runs
 * discovery + registration + PKCE, opens the system browser and completes the
 * exchange via an ephemeral loopback listener. Resolves once the flow is
 * armed; the actual token lands asynchronously (`server://oauth-connected` /
 * `server://oauth-error` events).
 */
export const connectServerOAuth = (serverName: string, challenge?: string) =>
  apiPost(`/servers/${encodeURIComponent(serverName)}/oauth/connect`, {
    challenge: challenge ?? null,
  });

/** Persisted flow status: disabled | pending | connected | error. */
export const getServerOAuthStatus = async (serverName: string): Promise<string> => {
  try {
    const res = await apiGet<{ success: boolean; data?: { status?: string } }>(
      `/servers/${encodeURIComponent(serverName)}/oauth/status`,
    );
    return res.data?.status ?? 'disabled';
  } catch {
    return 'disabled';
  }
};
