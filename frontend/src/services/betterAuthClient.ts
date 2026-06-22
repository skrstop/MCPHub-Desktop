import { createAuthClient } from 'better-auth/react';
import { getBasePath } from '../utils/runtime';

const normalizePath = (value: string): string => {
  if (!value) {
    return '/api/auth/better';
  }
  return value.startsWith('/') ? value : `/${value}`;
};

const resolveBaseUrl = (basePathOverride?: string): string => {
  if (typeof window === 'undefined') {
    return '';
  }
  const basePath = getBasePath();
  const authPath = normalizePath(basePathOverride || '/api/auth/better');
  // In Tauri, window.location.origin is "tauri://localhost" which BetterAuth rejects.
  // Fall back to http://localhost so the client can be created without errors.
  const origin = window.location.origin.startsWith('tauri://')
    ? 'http://localhost'
    : window.location.origin;
  return `${origin}${basePath}${authPath}`;
};

interface StartOidcLoginOptions {
  providerId: string;
  callbackURL: string;
  errorCallbackURL: string;
  basePathOverride?: string;
}

export const createBetterAuthClient = (basePathOverride?: string) =>
  createAuthClient({
    baseURL: resolveBaseUrl(basePathOverride),
    fetchOptions: {
      credentials: 'include',
    },
  });

export const startOidcLogin = async ({
  providerId,
  callbackURL,
  errorCallbackURL,
  basePathOverride,
}: StartOidcLoginOptions): Promise<void> => {
  const response = await fetch(`${resolveBaseUrl(basePathOverride)}/sign-in/oauth2`, {
    method: 'POST',
    credentials: 'include',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({
      providerId,
      callbackURL,
      errorCallbackURL,
    }),
  });

  if (!response.ok) {
    throw new Error('OIDC sign-in request failed.');
  }

const data = await response.json().catch(() => ({})) as { url?: string; redirect?: boolean };
  if (!data.redirect || !data.url) {
    throw new Error('OIDC sign-in did not return a redirect URL.');
  }

  window.location.assign(data.url);
};

export const authClient = createBetterAuthClient();
