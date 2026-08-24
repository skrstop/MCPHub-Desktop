import React, { createContext, useContext, useState, useEffect, useCallback, ReactNode } from 'react';
import { AuthState } from '../types';
import * as authService from '../services/authService';
import { getPublicConfig } from '../services/configService';
import { apiPut } from '../utils/fetchInterceptor';

// Initial auth state
const initialState: AuthState = {
  isAuthenticated: false,
  loading: true,
  user: null,
  error: null,
};

// Create auth context
const AuthContext = createContext<{
  auth: AuthState;
  login: (
    username: string,
    password: string,
  ) => Promise<{ success: boolean; isUsingDefaultPassword?: boolean; message?: string }>;
  register: (username: string, password: string, isAdmin?: boolean) => Promise<boolean>;
  logout: () => void;
  /** Exit guest mode: disable `skipAuth` server-side so the dashboard requires
   *  login again. No-op outside guest mode (falls back to normal `logout`).
   *  `skipServerUpdate`: skip the `PUT /system-config { routing:{skipAuth:false} }`
   *  call — use when the caller has already persisted that change (e.g. the
   *  Settings page toggle writes via `updateRoutingConfig` first). Either way
   *  local auth state is cleared (without a Better Auth `signOut()` HTTP call,
   *  which has no backing server in the desktop app). */
  exitGuestMode: (opts?: { skipServerUpdate?: boolean }) => Promise<void>;
  reloadAuth: () => Promise<void>;
}>({
  auth: initialState,
  login: async () => ({ success: false }),
  register: async () => false,
  logout: () => {},
  exitGuestMode: async () => {},
  reloadAuth: async () => {},
});

// Auth provider component
export const AuthProvider: React.FC<{ children: ReactNode }> = ({ children }) => {
  const [auth, setAuth] = useState<AuthState>(initialState);

  const loadUser = useCallback(async () => {
    try {
      setAuth((prev) => ({ ...prev, loading: true }));
      // First check if authentication should be skipped
      const { skipAuth, permissions } = await getPublicConfig();

      if (skipAuth) {
        // If authentication is disabled, set user as authenticated with a dummy user
        setAuth({
          isAuthenticated: true,
          loading: false,
          skipAuth: true,
          user: {
            username: '免登陆模式',
            isAdmin: true,
            permissions,
          },
          error: null,
        });
        return;
      }

      // Normal authentication flow
      const token = authService.getToken();

      if (!token) {
        const betterAuthResponse = await authService.getBetterAuthUser();
        if (betterAuthResponse.success && betterAuthResponse.user) {
          setAuth({
            isAuthenticated: true,
            loading: false,
            user: betterAuthResponse.user,
            error: null,
          });
          return;
        }

        setAuth({
          ...initialState,
          loading: false,
        });
        return;
      }

      try {
        const response = await authService.getCurrentUser();

        if (response.success && response.user) {
          setAuth({
            isAuthenticated: true,
            loading: false,
            user: response.user,
            error: null,
          });
        } else {
          authService.removeToken();
          const betterAuthResponse = await authService.getBetterAuthUser();
          if (betterAuthResponse.success && betterAuthResponse.user) {
            setAuth({
              isAuthenticated: true,
              loading: false,
              user: betterAuthResponse.user,
              error: null,
            });
          } else {
            setAuth({
              ...initialState,
              loading: false,
            });
          }
        }
      } catch (error) {
        authService.removeToken();
        const betterAuthResponse = await authService.getBetterAuthUser();
        if (betterAuthResponse.success && betterAuthResponse.user) {
          setAuth({
            isAuthenticated: true,
            loading: false,
            user: betterAuthResponse.user,
            error: null,
          });
        } else {
          setAuth({
            ...initialState,
            loading: false,
          });
        }
      }
    } catch (outerError) {
      // Safety net: ensure loading is always cleared even if unexpected error occurs
      console.error('Unexpected error in loadUser:', outerError);
      setAuth({ ...initialState, loading: false });
    }
  }, []);

  // Load user if token exists
  useEffect(() => {
    loadUser();
  }, [loadUser]);

  // Login function
  const login = async (
    username: string,
    password: string,
  ): Promise<{ success: boolean; isUsingDefaultPassword?: boolean; message?: string }> => {
    try {
      const response = await authService.login({ username, password });

      if (response.success && response.token && response.user) {
        setAuth({
          isAuthenticated: true,
          loading: false,
          user: response.user,
          error: null,
        });
        return {
          success: true,
          isUsingDefaultPassword: response.isUsingDefaultPassword,
        };
      } else {
        setAuth({
          ...initialState,
          loading: false,
          error: response.message || 'Authentication failed',
        });
        return { success: false, message: response.message };
      }
    } catch (error) {
      setAuth({
        ...initialState,
        loading: false,
        error: 'Authentication failed',
      });
      return { success: false, message: error instanceof Error ? error.message : undefined };
    }
  };

  // Register function
  const register = async (
    username: string,
    password: string,
    isAdmin = false,
  ): Promise<boolean> => {
    try {
      const response = await authService.register({ username, password, isAdmin });

      if (response.success && response.token && response.user) {
        setAuth({
          isAuthenticated: true,
          loading: false,
          user: response.user,
          error: null,
        });
        return true;
      } else {
        setAuth({
          ...initialState,
          loading: false,
          error: response.message || 'Registration failed',
        });
        return false;
      }
    } catch (error) {
      setAuth({
        ...initialState,
        loading: false,
        error: 'Registration failed',
      });
      return false;
    }
  };

  // Logout function
  const logout = (): void => {
    authService.logout();
    setAuth({
      ...initialState,
      loading: false,
    });
  };

  // Exit guest mode: disable `skipAuth` on the server so the dashboard
  // requires a real login again, then clear local auth state. In guest mode
  // there's no Better Auth HTTP session (and no backing server in the desktop
  // app), so we skip the `signOut()` call that would otherwise hit
  // `/api/auth/better/sign-out` and fail with ECONNREFUSED.
  //
  // `skipServerUpdate`: skip the PUT /system-config call (caller already did it).
  const exitGuestMode = useCallback(
    async (opts?: { skipServerUpdate?: boolean }): Promise<void> => {
      if (!opts?.skipServerUpdate) {
        try {
          await apiPut('/system-config', { routing: { skipAuth: false } });
        } catch (error) {
          console.debug('Failed to disable skipAuth on exit', { error });
        }
      }
      await authService.logout({ skipBetterAuthSignOut: true });
      setAuth({
        ...initialState,
        loading: false,
      });
    },
    [],
  );

  return (
    <AuthContext.Provider
      value={{ auth, login, register, logout, exitGuestMode, reloadAuth: loadUser }}
    >
      {children}
    </AuthContext.Provider>
  );
};

// Custom hook to use auth context
export const useAuth = () => useContext(AuthContext);
