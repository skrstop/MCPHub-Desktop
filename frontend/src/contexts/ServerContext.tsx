import React, { createContext, useState, useEffect, useRef, useCallback, useContext } from 'react';
import { useTranslation } from 'react-i18next';
import { Server, ApiResponse } from '@/types';
import { createServerPollingPolicy } from '@/utils/serverPolling';
import { disconnectServerOAuth as disconnectServerOAuthRequest } from '@/services/serverOAuthService';
import { apiGet, apiPost, apiDelete } from '../utils/fetchInterceptor';
import { useAuth } from './AuthContext';

const SERVERS_PER_PAGE_KEY = 'mcphub_servers_per_page';
const DEFAULT_SERVERS_PER_PAGE = 5;
const VALID_PAGE_SIZES = new Set([5, 10, 20, 50]);

const getInitialServersPerPage = (): number => {
  if (typeof window === 'undefined') {
    return DEFAULT_SERVERS_PER_PAGE;
  }

  const saved = window.localStorage.getItem(SERVERS_PER_PAGE_KEY);
  if (!saved) {
    return DEFAULT_SERVERS_PER_PAGE;
  }

  const parsed = Number(saved);
  return VALID_PAGE_SIZES.has(parsed) ? parsed : DEFAULT_SERVERS_PER_PAGE;
};

// Pagination info type
interface PaginationInfo {
  page: number;
  limit: number;
  total: number;
  totalPages: number;
  hasNextPage: boolean;
  hasPrevPage: boolean;
}

type ServerListResponse = ApiResponse<Server[]> & {
  pagination?: PaginationInfo;
  allServers?: Server[];
};

// Context type definition
interface ServerContextType {
  servers: Server[];
  allServers: Server[]; // All servers without pagination, for Dashboard, Groups, Settings
  error: string | null;
  setError: (error: string | null) => void;
  isLoading: boolean;
  fetchAttempts: number;
  pagination: PaginationInfo | null;
  currentPage: number;
  serversPerPage: number;
  setCurrentPage: (page: number) => void;
  setServersPerPage: (limit: number) => void;
  triggerRefresh: () => void;
  refreshIfNeeded: () => void; // Smart refresh with debounce
  handleServerAdd: () => void;
  handleServerEdit: (server: Server) => Promise<any>;
  handleServerRemove: (serverName: string) => Promise<boolean>;
  handleServerToggle: (server: Server, enabled: boolean) => Promise<boolean>;
  handleServerReload: (server: Server) => Promise<boolean>;
  handleServerReinstall: (server: Server) => Promise<boolean>;
  handleServerOAuthDisconnect: (server: Server) => Promise<boolean>;
}

// Create Context
const ServerContext = createContext<ServerContextType | undefined>(undefined);

// Provider component
export const ServerProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const { t } = useTranslation();
  const { auth } = useAuth();
  const [servers, setServers] = useState<Server[]>([]);
  const [allServers, setAllServers] = useState<Server[]>([]); // All servers without pagination
  const [error, setError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  const [isInitialLoading, setIsInitialLoading] = useState(true);
  const [fetchAttempts, setFetchAttempts] = useState(0);
  const [pagination, setPagination] = useState<PaginationInfo | null>(null);
  const [currentPage, setCurrentPage] = useState(1);
  const [serversPerPage, setServersPerPage] = useState(getInitialServersPerPage);

  // Schedule the next poll only after the current request completes.
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Track last fetch time to implement smart refresh
  const lastFetchTimeRef = useRef<number>(0);
  // Minimum interval between manual refreshes (5 seconds in dev, 3 seconds in prod)
  const MIN_REFRESH_INTERVAL = import.meta.env.DEV ? 5000 : 3000;

  // Clear the timer
  const clearTimer = () => {
    if (timerRef.current) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
  };

  const applyServerResponse = useCallback(
    (paginatedData: ServerListResponse | Server[], allData: ServerListResponse | Server[]) => {
      if (
        paginatedData &&
        !Array.isArray(paginatedData) &&
        paginatedData.success &&
        Array.isArray(paginatedData.data)
      ) {
        setServers(paginatedData.data);
        setPagination(paginatedData.pagination ?? null);
      } else if (Array.isArray(paginatedData)) {
        setServers(paginatedData);
        setPagination(null);
      } else {
        console.error('Invalid server data format', { paginatedData });
        setServers([]);
        setPagination(null);
      }

      if (allData && !Array.isArray(allData) && allData.success && Array.isArray(allData.data)) {
        setAllServers(allData.data);
      } else if (Array.isArray(allData)) {
        setAllServers(allData);
      } else {
        setAllServers([]);
      }
    },
    [],
  );

  useEffect(() => {
    if (!auth.isAuthenticated) {
      setServers([]);
      setAllServers([]);
      setIsInitialLoading(false);
      setError(null);
      return;
    }

    let active = true;
    const controller = new AbortController();
    const policy = createServerPollingPolicy();
    setFetchAttempts(0);

    const fetchServers = async () => {
      let delay: number;
      try {
        const params = new URLSearchParams({
          page: currentPage.toString(),
          limit: serversPerPage.toString(),
        });
        // Fetch both paginated servers and all servers in parallel
        const [paginatedData, allData] = (await Promise.all([
          apiGet(`/servers?${params.toString()}`, { signal: controller.signal }),
          apiGet('/servers', { signal: controller.signal }), // All servers without pagination
        ])) as [ServerListResponse | Server[], ServerListResponse | Server[]];
        if (!active) return;
        if (
          !Array.isArray(paginatedData) &&
          (!paginatedData.success || !Array.isArray(paginatedData.data))
        ) {
          throw new Error('Invalid server list response');
        }
        delay = policy.success(
          (Array.isArray(paginatedData) ? paginatedData : paginatedData.data!) as Server[],
        );
        lastFetchTimeRef.current = Date.now();
        applyServerResponse(paginatedData, allData);
        setIsInitialLoading(false);
        setFetchAttempts(0);
        setError(null);
      } catch (err) {
        if (!active) return;
        delay = policy.failure();
        setFetchAttempts((attempts) => attempts + 1);
        setIsInitialLoading(false);
        console.error('Error fetching servers', { err });
        if (!navigator.onLine) {
          setError(t('errors.network'));
        } else if (
          err instanceof TypeError &&
          (err.message.includes('NetworkError') || err.message.includes('Failed to fetch'))
        ) {
          setError(t('errors.serverConnection'));
        } else {
          setError(t('errors.serverFetch'));
        }
      }
      if (active) timerRef.current = setTimeout(fetchServers, delay);
    };

    void fetchServers();
    return () => {
      active = false;
      controller.abort();
      clearTimer();
    };
  }, [auth.isAuthenticated, applyServerResponse, refreshKey, t, currentPage, serversPerPage]);

  useEffect(() => {
    if (!pagination) {
      return;
    }

    const totalPages = Math.max(1, pagination.totalPages || 1);
    if (currentPage > totalPages) {
      setCurrentPage(totalPages);
    }
  }, [pagination, currentPage]);

  // Manually trigger refresh (always refreshes)
  const triggerRefresh = useCallback(() => {
    // Clear current timer
    clearTimer();

    // Change in refreshKey will trigger useEffect to run again
    setRefreshKey((prevKey) => prevKey + 1);
  }, []);

  // Smart refresh with debounce (only refresh if enough time has passed)
  const refreshIfNeeded = useCallback(() => {
    const now = Date.now();
    const timeSinceLastFetch = now - lastFetchTimeRef.current;

    // Log who is calling this
    console.log('[ServerContext] refreshIfNeeded called', { timeSinceLastFetch });

    // Only refresh if enough time has passed since last fetch
    if (timeSinceLastFetch >= MIN_REFRESH_INTERVAL) {
      console.log('[ServerContext] Triggering refresh after minimum interval', {
        minRefreshInterval: MIN_REFRESH_INTERVAL,
      });
      triggerRefresh();
    } else {
      console.log('[ServerContext] Skipping refresh because minimum interval not reached', {
        minRefreshInterval: MIN_REFRESH_INTERVAL,
        timeSinceLastFetch,
      });
    }
  }, [triggerRefresh]);

  // Server related operations
  const handleServerAdd = useCallback(() => {
    setRefreshKey((prevKey) => prevKey + 1);
  }, []);

  const handleServerEdit = useCallback(
    async (server: Server) => {
      try {
        // Fetch single server config instead of all settings
        const encodedServerName = encodeURIComponent(server.name);
        const serverData: ApiResponse<{
          name: string;
          status: string;
          tools: any[];
          config: Record<string, any>;
        }> = await apiGet(`/servers/${encodedServerName}`);

        if (serverData && serverData.success && serverData.data) {
          return {
            name: serverData.data.name,
            status: serverData.data.status,
            tools: serverData.data.tools || [],
            config: serverData.data.config,
          };
        } else {
          console.error('Failed to get server config', { serverName: server.name, serverData });
          setError(t('server.invalidConfig', { serverName: server.name }));
          return null;
        }
      } catch (err) {
        console.error('Error fetching server config', { serverName: server.name, err });
        setError(err instanceof Error ? err.message : String(err));
        return null;
      }
    },
    [t],
  );

  const handleServerRemove = useCallback(
    async (serverName: string) => {
      try {
        const encodedServerName = encodeURIComponent(serverName);
        const result = await apiDelete(`/servers/${encodedServerName}`);

        if (!result || !result.success) {
          setError(result?.message || t('server.deleteError', { serverName }));
          return false;
        }

        setRefreshKey((prevKey) => prevKey + 1);
        return true;
      } catch (err) {
        setError(t('errors.general') + ': ' + (err instanceof Error ? err.message : String(err)));
        return false;
      }
    },
    [t],
  );

  const handleServerToggle = useCallback(
    async (server: Server, enabled: boolean) => {
      try {
        const encodedServerName = encodeURIComponent(server.name);
        const result = await apiPost(`/servers/${encodedServerName}/toggle`, { enabled });

        if (!result || !result.success) {
          console.error('Failed to toggle server', { serverName: server.name, result });
          setError(result?.message || t('server.toggleError', { serverName: server.name }));
          return false;
        }

        // Update the UI immediately to reflect the change
        setRefreshKey((prevKey) => prevKey + 1);
        return true;
      } catch (err) {
        console.error('Error toggling server', { serverName: server.name, err });
        setError(err instanceof Error ? err.message : String(err));
        return false;
      }
    },
    [t],
  );

  const handleServerReload = useCallback(
    async (server: Server) => {
      try {
        const encodedServerName = encodeURIComponent(server.name);
        const result = await apiPost(`/servers/${encodedServerName}/reload`, {});

        if (!result || !result.success) {
          console.error('Failed to reload server', { serverName: server.name, result });
          setError(t('server.reloadError', { serverName: server.name }));
          return false;
        }

        // Refresh server list after successful reload
        triggerRefresh();
        return true;
      } catch (err) {
        console.error('Error reloading server', { serverName: server.name, err });
        setError(err instanceof Error ? err.message : String(err));
        return false;
      }
    },
    [t, triggerRefresh],
  );

  const handleServerReinstall = useCallback(
    async (server: Server) => {
      try {
        const encodedServerName = encodeURIComponent(server.name);
        const result = await apiPost(`/servers/${encodedServerName}/reinstall`, {});

        if (!result || !result.success) {
          console.error('Failed to reinstall server', { serverName: server.name, result });
          setError(result?.message || t('server.reinstallError', { serverName: server.name }));
          return false;
        }

        // Refresh server list after successful reinstall
        triggerRefresh();
        return true;
      } catch (err) {
        console.error('Error reinstalling server', { serverName: server.name, err });
        setError(err instanceof Error ? err.message : String(err));
        return false;
      }
    },
    [t, triggerRefresh],
  );

  const handleServerOAuthDisconnect = useCallback(
    async (server: Server) => {
      try {
        const result = await disconnectServerOAuthRequest(server.name);

        if (!result || !result.success) {
          console.error('Failed to disconnect server OAuth', { serverName: server.name, result });
          setError(result?.message || t('server.disconnectOAuthError', { serverName: server.name }));
          return false;
        }

        triggerRefresh();
        return true;
      } catch (err) {
        console.error('Error disconnecting server OAuth', { serverName: server.name, err });
        setError(err instanceof Error ? err.message : String(err));
        return false;
      }
    },
    [t, triggerRefresh],
  );

  // Handle page change
  const handlePageChange = useCallback((page: number) => {
    setCurrentPage(page);
  }, []);

  // Handle servers per page change
  const handleServersPerPageChange = useCallback((limit: number) => {
    const normalizedLimit = VALID_PAGE_SIZES.has(limit) ? limit : DEFAULT_SERVERS_PER_PAGE;

    if (typeof window !== 'undefined') {
      window.localStorage.setItem(SERVERS_PER_PAGE_KEY, String(normalizedLimit));
    }

    setServersPerPage(normalizedLimit);
    setCurrentPage(1); // Reset to first page when changing page size
  }, []);

  const value: ServerContextType = {
    servers,
    allServers,
    error,
    setError,
    isLoading: isInitialLoading,
    fetchAttempts,
    pagination,
    currentPage,
    serversPerPage,
    setCurrentPage: handlePageChange,
    setServersPerPage: handleServersPerPageChange,
    triggerRefresh,
    refreshIfNeeded,
    handleServerAdd,
    handleServerEdit,
    handleServerRemove,
    handleServerToggle,
    handleServerReload,
    handleServerReinstall,
    handleServerOAuthDisconnect,
  };

  return <ServerContext.Provider value={value}>{children}</ServerContext.Provider>;
};

// Custom hook to use the Server context
export const useServerContext = () => {
  const context = useContext(ServerContext);
  if (context === undefined) {
    throw new Error('useServerContext must be used within a ServerProvider');
  }
  return context;
};
