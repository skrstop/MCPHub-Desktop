import React, { useMemo, useState, useEffect, useRef, useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { useNavigate } from 'react-router-dom';
import { Plus, RefreshCw, Search, Upload, FileCode, AlertCircle, X, Loader2 } from 'lucide-react';
import { Server } from '@/types';
import ServerCard from '@/components/ServerCard';
import AddServerForm from '@/components/AddServerForm';
import EditServerForm from '@/components/EditServerForm';
import McpbUploadForm from '@/components/McpbUploadForm';
import JSONImportForm from '@/components/JSONImportForm';
import Pagination from '@/components/ui/Pagination';
import { useToast } from '@/contexts/ToastContext';
import { apiPost } from '@/utils/fetchInterceptor';
import { ApiResponse, ServerPage } from '@/types';
import { useServerData } from '@/hooks/useServerData';
import { useCostData } from '@/hooks/useCostData';
import { selectServerPage, getServerFilterCounts, type ServerFilter } from '@/utils/serverFilters';
import { resolveDuplicateResponse } from '@/utils/serverDuplicate';

const ServersPage: React.FC = () => {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { showToast } = useToast();
  const {
    servers,
    allServers,
    error,
    setError,
    isLoading,
    currentPage,
    serversPerPage,
    setCurrentPage,
    setServersPerPage,
    handleServerAdd,
    handleServerEdit,
    handleServerRemove,
    handleServerToggle,
    handleServerVisibilityChange,
    handleServerReload,
    handleServerReinstall,
    handleServerOAuthDisconnect,
    triggerRefresh,
  } = useServerData({ refreshOnMount: true });

  const { serverCosts, refetch: refetchCost } = useCostData();

  // Re-fetch context footprint whenever server data changes (toggle, reload, edit).
  useEffect(() => {
    refetchCost();
  }, [servers, refetchCost]);

  const [editingServer, setEditingServer] = useState<Server | null>(null);
  const [duplicateServer, setDuplicateServer] = useState<Server | null>(null);
  const [duplicatingServer, setDuplicatingServer] = useState<string | null>(null);
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [isCheckingUpdates, setIsCheckingUpdates] = useState(false);
  const [showMcpbUpload, setShowMcpbUpload] = useState(false);
  const [showJsonImport, setShowJsonImport] = useState(false);
  const [filter, setFilter] = useState<ServerFilter>('all');
  const [search, setSearch] = useState('');
  // Server-type filter: 'custom' (default) hides builtin servers, 'builtin'
  // shows only them, 'all' shows both. Builtin servers (RAG) are virtual.
  const [typeFilter, setTypeFilter] = useState<'custom' | 'builtin' | 'all'>('custom');

  const typeFilteredServers = useMemo(() => {
    if (typeFilter === 'all') return allServers;
    return allServers.filter((s) =>
      typeFilter === 'builtin' ? s.config?.type === 'builtin' : s.config?.type !== 'builtin',
    );
  }, [allServers, typeFilter]);

  const counts = useMemo(() => getServerFilterCounts(typeFilteredServers), [typeFilteredServers]);

  // Whether any npx/uvx stdio server exists — gates the "stdio update check"
  // button (only package-manager-backed stdio servers have a registry to check).
  const hasStdioServer = useMemo(
    () =>
      allServers.some(
        (s) => s.config?.command === 'npx' || s.config?.command === 'uvx',
      ),
    [allServers],
  );

  // ── 搜索后端化（混合模式）──
  // 搜索词或状态筛选非空时走后端 `search_servers`（SQL name/description LIKE
  // 预过滤 + Rust 合并运行时状态/工具名匹配 + 后端分页，防抖 + 竞态守卫）；
  // 空搜索 + all 筛选时保持前端全量路径 —— 服务器列表由轮询实时刷新
  // （连接状态是运行时数据），前端分页可直接吃到轮询更新。
  const searchActive = search.trim() !== '' || filter !== 'all';
  const [serverPageItems, setServerPageItems] = useState<Server[]>([]);
  const [serverPageTotal, setServerPageTotal] = useState(0);
  const [serverPageLoading, setServerPageLoading] = useState(false);
  const serverReqId = useRef(0);

  useEffect(() => {
    if (!searchActive) {
      serverReqId.current += 1;
      setServerPageItems([]);
      setServerPageTotal(0);
      return;
    }
    const id = ++serverReqId.current;
    setServerPageLoading(true);
    const timer = setTimeout(async () => {
      try {
        const res: ApiResponse<ServerPage> = await apiPost('/servers/search', {
          search: search.trim(),
          typeFilter,
          statusFilter: filter,
          page: currentPage,
          pageSize: serversPerPage,
        });
        if (id !== serverReqId.current) return;
        setServerPageItems(res.data?.items ?? []);
        setServerPageTotal(res.data?.total ?? 0);
      } catch {
        if (id !== serverReqId.current) return;
        setServerPageItems([]);
        setServerPageTotal(0);
      } finally {
        if (id === serverReqId.current) setServerPageLoading(false);
      }
    }, 250);
    return () => clearTimeout(timer);
  }, [searchActive, search, filter, typeFilter, currentPage, serversPerPage]);

  // Client-side path (empty search): full list filter + pagination, fed by polling.
  const { servers: clientVisibleServers, pagination: clientPagination } = useMemo(
    () => selectServerPage(typeFilteredServers, filter, search, currentPage, serversPerPage),
    [typeFilteredServers, filter, search, currentPage, serversPerPage],
  );

  const visibleServers = searchActive ? serverPageItems : clientVisibleServers;
  const listTotal = searchActive ? serverPageTotal : clientPagination.total;
  const totalPages = searchActive
    ? Math.max(1, Math.ceil(serverPageTotal / serversPerPage))
    : clientPagination.totalPages;
  const safePage = Math.min(currentPage, totalPages);
  useEffect(() => {
    if (safePage !== currentPage) setCurrentPage(safePage);
  }, [safePage, currentPage, setCurrentPage]);
  const pagination = { page: safePage, limit: serversPerPage, total: listTotal, totalPages };

  // Edit opens the modal from a `GET /servers/<name>` response, so it has the
  // same interleave race the Duplicate B1 guard fixes: click Edit on A, then
  // click Edit on B while A's request is still in flight - A's late response
  // would otherwise overwrite `editingServer` after B's already opened, and
  // `EditServerForm.handleSubmit` would `PUT` B's form values onto A (silent
  // cross-server write, same causal mechanism as the Duplicate race).
  //
  // Fix: same B1 pattern - a monotonically increasing request id that is never
  // reset. Each accepted click bumps it; the response is only committed if its
  // id is still the latest, so a superseded/stale response is dropped.
  const editRequestId = useRef(0); // bumped on every accepted click
  const handleEditClick = async (server: Server) => {
    const requestId = ++editRequestId.current;
    const fullServerData = await handleServerEdit(server);
    // Drop a stale response: a newer click superseded this request, so
    // committing it would open the modal for the wrong server.
    if (requestId !== editRequestId.current) return;
    if (fullServerData) setEditingServer(fullServerData);
  };

  // Duplicate pre-fills the add form, so it needs the same full stored
  // configuration the edit flow loads - the card only carries the list
  // projection, which has no env/headers (#1187).
  //
  // B1: guard against interleaved clicks with a monotonically increasing
  // request id (never reset); a superseded response is dropped and cannot
  // clear a newer request's busy indicator.
  const duplicateRequestId = useRef(0); // bumped on every accepted click
  const handleDuplicateClick = async (server: Server) => {
    // Ignore repeat clicks while this server's stored config is still loading,
    // so a slow request cannot fire twice.
    if (duplicatingServer === server.name) return;
    const requestId = ++duplicateRequestId.current;
    setDuplicatingServer(server.name);
    try {
      const fullServerData = await handleServerEdit(server);
      // Drop a stale response: a newer click superseded this request, so
      // committing it would overwrite the prefill with the wrong server.
      if (resolveDuplicateResponse(requestId, duplicateRequestId.current) === 'stale') return;
      if (fullServerData) setDuplicateServer(fullServerData);
    } finally {
      // Only clear the busy indicator when this request is still the latest.
      if (resolveDuplicateResponse(requestId, duplicateRequestId.current) === 'commit') {
        setDuplicatingServer(null);
      }
    }
  };

  const handleRefresh = async () => {
    setIsRefreshing(true);
    try {
      triggerRefresh();
      await new Promise((resolve) => setTimeout(resolve, 400));
    } finally {
      setIsRefreshing(false);
    }
  };

  const handleCheckStdioUpdates = async () => {
    setIsCheckingUpdates(true);
    try {
      const result = await apiPost<{ success: boolean; checked?: number; message?: string }>(
        '/servers/check-stdio-updates',
        {},
      );
      const count = result?.checked ?? 0;
      if (result?.success) {
        showToast(
          t('server.checkStdioUpdatesStarted', { count }) ||
            `Checking ${count} stdio server(s) for updates`,
          'success',
        );
      } else {
        showToast(result?.message || t('server.checkStdioUpdatesError') || 'Check failed', 'error');
      }
      // The command returns immediately; keep the spinner spinning while the
      // per-server results trickle back via server://update-available (each
      // registry fetch can take a few seconds).
      await new Promise((resolve) => setTimeout(resolve, 1500));
    } finally {
      setIsCheckingUpdates(false);
    }
  };

  return (
    <div>
      {/* Header */}
      <div className="flex items-end justify-between gap-4 mb-6">
        <div>
          <h1 className="hub-h1">{t('pages.servers.title')}</h1>
          <p className="hub-sub">
            <span className="hub-num">{counts.all}</span> {t('nav.servers').toLowerCase()} ·{' '}
            <span className="hub-num">{counts.online}</span> {t('status.online')} ·{' '}
            <span className="hub-num">{counts.issues}</span>{' '}
            {t('common.inactive') || 'issues'}
          </p>
        </div>
        <div className="flex gap-2">
          {hasStdioServer && (
            <button
              className="hub-btn"
              onClick={handleCheckStdioUpdates}
              disabled={isCheckingUpdates}
              aria-label={t('server.checkStdioUpdates') || 'stdio update check'}
            >
              {isCheckingUpdates ? (
                <Loader2 size={13} className="animate-spin" />
              ) : (
                <RefreshCw size={13} />
              )}
              {t('server.checkStdioUpdates') || 'stdio update check'}
            </button>
          )}
          <button className="hub-btn" onClick={() => navigate('/market')}>
            <Plus size={13} /> {t('nav.market')}
          </button>
          <button className="hub-btn" onClick={() => setShowJsonImport(true)}>
            <FileCode size={13} /> {t('jsonImport.button')}
          </button>
          <button className="hub-btn" onClick={() => setShowMcpbUpload(true)}>
            <Upload size={13} /> {t('mcpb.upload')}
          </button>
          <button
            className="hub-btn"
            onClick={handleRefresh}
            disabled={isRefreshing}
            aria-label={t('common.refresh')}
          >
            <RefreshCw size={13} className={isRefreshing ? 'animate-spin' : ''} />
            {t('common.refresh')}
          </button>
          <AddServerForm
            onAdd={handleServerAdd}
            duplicateSource={duplicateServer}
            onDuplicateCancel={() => setDuplicateServer(null)}
          />
        </div>
      </div>

      {error && (
        <div
          className="hub-card flex items-center justify-between gap-3 mb-4"
          style={{
            padding: '10px 14px',
            borderColor: 'oklch(0.85 0.1 25)',
            background: 'oklch(0.97 0.03 25)',
            color: 'oklch(0.4 0.18 25)',
          }}
        >
          <div className="flex items-center gap-2 min-w-0">
            <AlertCircle size={14} className="flex-shrink-0" />
            <span className="truncate text-[13px]">{error}</span>
          </div>
          <button
            className="hub-icon-btn sm"
            onClick={() => setError(null)}
            aria-label={t('app.closeButton')}
          >
            <X size={13} />
          </button>
        </div>
      )}

      {/* Toolbar */}
      <div className="flex items-center gap-2 mb-4 flex-wrap">
        {/* Server-type filter (default: custom - hides builtin servers) */}
        <div
          className="hub-card flex items-center"
          style={{ padding: 2, borderRadius: 7, background: 'var(--hub-surface)' }}
        >
          {(
            [
              ['custom', t('server.typeCustom')],
              ['builtin', t('server.typeBuiltin')],
              ['all', t('common.all') || 'All'],
            ] as ['custom' | 'builtin' | 'all', string][]
          ).map(([k, l]) => (
            <button
              key={k}
              onClick={() => setTypeFilter(k)}
              className="inline-flex items-center gap-1.5 px-3 text-[12px]"
              style={{
                height: 24,
                borderRadius: 5,
                background: typeFilter === k ? 'var(--hub-bg-2)' : 'transparent',
                color: typeFilter === k ? 'var(--hub-ink)' : 'var(--hub-ink-3)',
                border: '1px solid ' + (typeFilter === k ? 'var(--hub-line)' : 'transparent'),
              }}
            >
              {l}
            </button>
          ))}
        </div>

        <div
          className="hub-card flex items-center"
          style={{ padding: 2, borderRadius: 7, background: 'var(--hub-surface)' }}
        >
          {(
            [
              ['all', t('common.all') || 'All', counts.all],
              ['online', t('status.online'), counts.online],
              ['issues', t('common.inactive') || 'Issues', counts.issues],
              ['disabled', t('pages.dashboard.disabledServers') || 'Disabled', counts.disabled],
            ] as [ServerFilter, string, number][]
          ).map(([k, l, n]) => (
            <button
              key={k}
              onClick={() => setFilter(k)}
              className="inline-flex items-center gap-1.5 px-3 text-[12px]"
              style={{
                height: 24,
                borderRadius: 5,
                background: filter === k ? 'var(--hub-bg-2)' : 'transparent',
                color: filter === k ? 'var(--hub-ink)' : 'var(--hub-ink-3)',
                border: '1px solid ' + (filter === k ? 'var(--hub-line)' : 'transparent'),
              }}
            >
              {l}
              <span className="hub-mono" style={{ fontSize: 11, color: 'var(--hub-ink-3)' }}>
                {n}
              </span>
            </button>
          ))}
        </div>

        <div
          className="hub-card flex items-center gap-2 px-2.5 flex-1"
          style={{ height: 30, background: 'var(--hub-surface)', maxWidth: 360 }}
        >
          <Search size={13} style={{ color: 'var(--hub-ink-3)' }} />
          <input
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            className="flex-1 bg-transparent outline-none text-[13px]"
            style={{ color: 'var(--hub-ink)' }}
            placeholder={t('server.searchPlaceholder') || 'Search…'}
          />
          {search && (
            <button onClick={() => setSearch('')} className="hub-icon-btn sm">
              <X size={11} />
            </button>
          )}
        </div>

      </div>

      {/* List */}
      {isLoading && servers.length === 0 ? (
        <div className="hub-card p-6 flex items-center justify-center">
          <div className="flex flex-col items-center gap-2">
            <RefreshCw size={20} className="animate-spin" style={{ color: 'var(--hub-ink-3)' }} />
            <p style={{ color: 'var(--hub-ink-3)' }}>{t('app.loading')}</p>
          </div>
        </div>
      ) : visibleServers.length === 0 ? (
        <div className="hub-card p-10 text-center" style={{ color: 'var(--hub-ink-3)' }}>
          <p>{servers.length === 0 ? t('app.noServers') : t('market.noServers')}</p>
        </div>
      ) : (
        <>
          <div className="flex flex-col">
            {visibleServers.map((server) => (
              <ServerCard
                key={server.name}
                server={server}
                cost={serverCosts.find((c) => c.name === server.name)}
                onRemove={handleServerRemove}
                onEdit={handleEditClick}
                onDuplicate={handleDuplicateClick}
                isDuplicating={duplicatingServer === server.name}
                onToggle={handleServerToggle}
                onVisibilityChange={handleServerVisibilityChange}
                onRefresh={triggerRefresh}
                onReload={handleServerReload}
                onReinstall={handleServerReinstall}
                onOAuthDisconnect={handleServerOAuthDisconnect}
              />
            ))}
          </div>

          <div className="flex items-center mt-4 text-[12px]" style={{ color: 'var(--hub-ink-3)' }}>
            <div className="flex-[2]">
              {t('common.showing', {
                start: (pagination.page - 1) * pagination.limit + 1,
                end: Math.min(pagination.page * pagination.limit, pagination.total),
                total: pagination.total,
              })}
            </div>
            <div className="flex-[4] flex justify-center">
              {pagination.totalPages > 1 && (
                <Pagination
                  currentPage={pagination.page}
                  totalPages={pagination.totalPages}
                  onPageChange={setCurrentPage}
                  disabled={isLoading}
                />
              )}
            </div>
            <div className="flex-[2] flex items-center justify-end gap-2">
              <label htmlFor="perPage">{t('common.itemsPerPage')}:</label>
              <select
                id="perPage"
                value={serversPerPage}
                onChange={(e) => setServersPerPage(Number(e.target.value))}
                disabled={isLoading}
                className="hub-input"
                style={{ height: 26, width: 70, padding: '0 6px', fontSize: 12 }}
              >
                <option value={5}>5</option>
                <option value={10}>10</option>
                <option value={20}>20</option>
                <option value={50}>50</option>
              </select>
            </div>
          </div>
        </>
      )}

      {editingServer && (
        <EditServerForm
          server={editingServer}
          onEdit={() => {
            setEditingServer(null);
            triggerRefresh();
          }}
          onCancel={() => setEditingServer(null)}
        />
      )}
      {showMcpbUpload && (
        <McpbUploadForm
          onSuccess={() => {
            setShowMcpbUpload(false);
            triggerRefresh();
          }}
          onCancel={() => setShowMcpbUpload(false)}
        />
      )}
      {showJsonImport && (
        <JSONImportForm
          onSuccess={() => {
            setShowJsonImport(false);
            triggerRefresh();
          }}
          onCancel={() => setShowJsonImport(false)}
        />
      )}
    </div>
  );
};

export default ServersPage;
