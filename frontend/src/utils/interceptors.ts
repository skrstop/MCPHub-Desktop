import { addInterceptor, removeInterceptor, type FetchInterceptor } from './fetchInterceptor';
import { isTauri } from './tauriClient';

// Token key in localStorage
const TOKEN_KEY = 'mcphub_token';

// Get token from localStorage
export const getToken = (): string | null => {
  return localStorage.getItem(TOKEN_KEY);
};

// Set token in localStorage
export const setToken = (token: string): void => {
  localStorage.setItem(TOKEN_KEY, token);
};

// Remove token from localStorage
export const removeToken = (): void => {
  localStorage.removeItem(TOKEN_KEY);
};

// Auth interceptor for automatically adding authorization headers
export const authInterceptor: FetchInterceptor = {
  request: async (url: string, config: RequestInit) => {
    const headers = new Headers(config.headers);
    const language = localStorage.getItem('i18nextLng') || 'en';
    headers.set('Accept-Language', language);

    const token = getToken();
    if (token) {
      headers.set('x-auth-token', token);
    }

    return {
      url,
      config: {
        ...config,
        headers,
        credentials: config.credentials ?? 'include',
      },
    };
  },

  response: async (response: Response) => {
    // Handle unauthorized responses
    if (response.status === 401) {
      // Token might be expired or invalid: clear it AND leave the zombie
      // session — otherwise ProtectedRoute keeps the user "logged in" while
      // every request silently fails until a manual reload.
      removeToken();
      // Desktop (skipAuth guest mode) has no /login route and a tauri://
      // origin — force-redirecting there breaks the webview (review round
      // 10). ProtectedRoute handles the unauthenticated state instead.
      if (!isTauri() && !window.location.pathname.startsWith('/login')) {
        window.location.assign('/login');
      }
    }

    return response;
  },

  error: async (error: Error) => {
    console.error('Auth interceptor error:', error);
    return error;
  },
};

// Install the auth interceptor
export const installAuthInterceptor = (): void => {
  addInterceptor(authInterceptor);
};

// Uninstall the auth interceptor
export const uninstallAuthInterceptor = (): void => {
  removeInterceptor(authInterceptor);
};

// Logging interceptor for development
export const loggingInterceptor: FetchInterceptor = {
  request: async (url: string, config: RequestInit) => {
    console.log('🚀 Request', { method: config.method || 'GET', url, config });
    return { url, config };
  },

  response: async (response: Response) => {
    console.log(`✅ [${response.status}] ${response.url}`);
    return response;
  },

  error: async (error: Error) => {
    console.error('❌ Fetch error', { error });
    return error;
  },
};

// Install the logging interceptor (only in development)
export const installLoggingInterceptor = (): void => {
  if (import.meta.env.DEV) {
    addInterceptor(loggingInterceptor);
  }
};

// Uninstall the logging interceptor
export const uninstallLoggingInterceptor = (): void => {
  removeInterceptor(loggingInterceptor);
};
