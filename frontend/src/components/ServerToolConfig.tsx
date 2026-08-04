import React, { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { IGroupServerConfig, Prompt, Resource, Server, ServerCost, Tool } from '@/types';
import { Wrench, MessageSquare, FileText, Search, X } from 'lucide-react';
import { cn } from '@/utils/cn';
import { useSettingsData } from '@/hooks/useSettingsData';
import { formatTokens } from '@/utils/contextCost';
import { getToolDescriptionInfo } from '@/utils/toolDescription';

type CapabilityKey = 'tools' | 'prompts' | 'resources';

const EMPTY_SELECTIONS: Pick<IGroupServerConfig, CapabilityKey> = {
  tools: [],
  prompts: [],
  resources: [],
};

const FULL_SELECTIONS: Pick<IGroupServerConfig, CapabilityKey> = {
  tools: 'all',
  prompts: 'all',
  resources: 'all',
};

interface ServerToolConfigProps {
  servers: Server[];
  value: string[] | IGroupServerConfig[];
  onChange: (value: IGroupServerConfig[]) => void;
  className?: string;
  serverCosts?: ServerCost[];
}

interface CapabilityItem {
  key: string;
  value: string;
  description?: string;
  defaultDescription?: string;
  hasDescriptionOverride?: boolean;
}

interface PaginatedItemsProps<T> {
  items: T[];
  pageSize?: number;
  listClassName?: string;
  children: (item: T, index: number) => React.ReactNode;
}

/** Paginated list of capability items with prev/next controls.
 *  Keeps the expanded card compact instead of scrolling a tall list. */
function PaginatedItems<T>({ items, pageSize = 5, listClassName = 'grid grid-cols-1 gap-2', children }: PaginatedItemsProps<T>) {
  const { t } = useTranslation();
  const [page, setPage] = useState(1);
  const totalPages = Math.max(1, Math.ceil(items.length / pageSize));
  // Clamp page when items shrink (filter/selection changes).
  const safePage = Math.min(page, totalPages);
  const start = (safePage - 1) * pageSize;
  const visible = items.slice(start, start + pageSize);

  React.useEffect(() => {
    if (page > totalPages) setPage(totalPages);
  }, [totalPages, page]);

  return (
    <div>
      <div className={listClassName}>
        {visible.map((item, i) => (
          <React.Fragment key={(item as { key?: string })?.key ?? i}>
            {children(item, i)}
          </React.Fragment>
        ))}
      </div>
      {items.length > pageSize && (
        <div className="flex items-center justify-center gap-3 mt-2 text-[12px]" style={{ color: 'var(--hub-ink-3)' }}>
          <button
            type="button"
            onClick={() => setPage(Math.max(1, safePage - 1))}
            disabled={safePage <= 1}
            className="hub-icon-btn sm"
            style={{ opacity: safePage <= 1 ? 0.4 : 1 }}
          >
            ‹
          </button>
          <span className="hub-mono">{safePage}/{totalPages}</span>
          <button
            type="button"
            onClick={() => setPage(Math.min(totalPages, safePage + 1))}
            disabled={safePage >= totalPages}
            className="hub-icon-btn sm"
            style={{ opacity: safePage >= totalPages ? 0.4 : 1 }}
          >
            ›
          </button>
        </div>
      )}
    </div>
  );
}

export const ServerToolConfig: React.FC<ServerToolConfigProps> = ({
  servers,
  value,
  onChange,
  className,
  serverCosts = [],
}) => {
  const { t } = useTranslation();
  const { nameSeparator } = useSettingsData();
  const [activeTab, setActiveTab] = useState<CapabilityKey>('tools');
  const [expandedServers, setExpandedServers] = useState<Set<string>>(new Set());

  const toggleServerExpanded = (serverName: string) => {
    setExpandedServers((prev) => {
      const next = new Set(prev);
      if (next.has(serverName)) {
        next.delete(serverName);
      } else {
        next.add(serverName);
      }
      return next;
    });
  };

  // Normalize current value to IGroupServerConfig[] format
  const normalizedValue: IGroupServerConfig[] = React.useMemo(() => {
    return value.map((item) => {
      if (typeof item === 'string') {
        return { name: item, ...FULL_SELECTIONS };
      }
      return {
        ...item,
        tools: item.tools || 'all',
        prompts: item.prompts || 'all',
        resources: item.resources || 'all',
      };
    });
  }, [value]);

  // Get available servers (enabled only)
  const availableServers = React.useMemo(
    () => servers.filter((server) => server.enabled !== false),
    [servers],
  );

  const toggleServer = (serverName: string) => {
    const existingIndex = normalizedValue.findIndex((config) => config.name === serverName);

    if (existingIndex >= 0) {
      // Remove server - this also removes all capability selections
      const newValue = normalizedValue.filter((config) => config.name !== serverName);
      onChange(newValue);
    } else {
      // Add server with all capabilities by default
      const newValue = [...normalizedValue, { name: serverName, ...FULL_SELECTIONS }];
      onChange(newValue);
    }
  };

  const hasAnyCapabilitySelection = (config: IGroupServerConfig) => {
    return (['tools', 'prompts', 'resources'] as CapabilityKey[]).some((capability) => {
      const selection = config[capability];
      return selection === 'all' || (Array.isArray(selection) && selection.length > 0);
    });
  };

  const updateServerCapability = (
    serverName: string,
    capability: CapabilityKey,
    selection: string[] | 'all',
  ) => {
    const existingServer = normalizedValue.find((config) => config.name === serverName);
    const baseConfig: IGroupServerConfig = existingServer
      ? { ...existingServer }
      : { name: serverName, ...EMPTY_SELECTIONS };
    const nextConfig: IGroupServerConfig = {
      ...baseConfig,
      [capability]: selection,
    };

    if (!hasAnyCapabilitySelection(nextConfig)) {
      const newValue = normalizedValue.filter((config) => config.name !== serverName);
      onChange(newValue);
      return;
    }

    if (existingServer) {
      onChange(normalizedValue.map((config) => (config.name === serverName ? nextConfig : config)));
      return;
    }

    onChange([...normalizedValue, nextConfig]);
  };

  const updateServerAlias = (serverName: string, alias: string) => {
    const existingServer = normalizedValue.find((config) => config.name === serverName);
    if (!existingServer) return;

    const nextConfig: IGroupServerConfig = { ...existingServer };
    if (alias) {
      nextConfig.alias = alias;
    } else {
      delete nextConfig.alias;
    }

    onChange(normalizedValue.map((config) => (config.name === serverName ? nextConfig : config)));
  };

  const normalizeNamedCapability = (serverName: string, name: string) => {
    const prefix = `${serverName}${nameSeparator}`;
    return name.startsWith(prefix) ? name.slice(prefix.length) : name;
  };

  const getCapabilityItems = (server: Server, capability: CapabilityKey): CapabilityItem[] => {
    if (capability === 'tools') {
      return (server.tools || [])
        .filter((tool) => tool.enabled !== false)
        .map((tool: Tool) => ({
          key: tool.name,
          value: normalizeNamedCapability(server.name, tool.name),
          description: tool.description,
          defaultDescription: tool.defaultDescription,
          hasDescriptionOverride: tool.hasDescriptionOverride,
        }));
    }

    if (capability === 'prompts') {
      return (server.prompts || [])
        .filter((prompt) => prompt.enabled !== false)
        .map((prompt: Prompt) => ({
          key: prompt.name,
          value: normalizeNamedCapability(server.name, prompt.name),
          description: prompt.description,
        }));
    }

    return (server.resources || [])
      .filter((resource) => resource.enabled !== false)
      .map((resource: Resource) => ({
        key: resource.uri,
        value: resource.uri,
        description: resource.description,
      }));
  };

  // Build one nested map (server -> item name -> cost) once per serverCosts change,
  // so per-render lookups don't rebuild a Map on every call (avoids O(N^2) churn).
  const serverCostsMap = React.useMemo(() => {
    const outerMap = new Map<string, Map<string, number>>();
    serverCosts.forEach((sc) => {
      const innerMap = new Map<string, number>();
      sc.items.forEach((i) => innerMap.set(i.name, i.cost));
      outerMap.set(sc.name, innerMap);
    });
    return outerMap;
  }, [serverCosts]);

  const costMapForServer = (serverName: string): Map<string, number> =>
    serverCostsMap.get(serverName) ?? new Map<string, number>();

  const getSelectedCapabilityCost = (server: Server, capability: CapabilityKey): number => {
    const costMap = costMapForServer(server.name);
    return getCapabilityItems(server, capability)
      .filter((item) => isCapabilityItemSelected(server.name, capability, item.value))
      .reduce((sum, item) => sum + (costMap.get(item.key) ?? 0), 0);
  };

  const getServerSelectedCost = (server: Server): number =>
    (['tools', 'prompts', 'resources'] as CapabilityKey[]).reduce(
      (sum, cap) => sum + getSelectedCapabilityCost(server, cap),
      0,
    );

  const toggleCapabilityItem = (
    serverName: string,
    capability: CapabilityKey,
    itemValue: string,
  ) => {
    const server = availableServers.find((s) => s.name === serverName);
    if (!server) return;

    const allItems = getCapabilityItems(server, capability).map((item) => item.value);
    const serverConfig = normalizedValue.find((config) => config.name === serverName);

    if (!serverConfig) {
      updateServerCapability(serverName, capability, [itemValue]);
      return;
    }

    const currentSelection = serverConfig[capability];
    if (currentSelection === 'all') {
      const nextSelection = allItems.filter((value) => value !== itemValue);
      updateServerCapability(serverName, capability, nextSelection);
      return;
    }

    if (Array.isArray(currentSelection)) {
      if (currentSelection.includes(itemValue)) {
        updateServerCapability(
          serverName,
          capability,
          currentSelection.filter((value) => value !== itemValue),
        );
        return;
      }

      const nextSelection = [...currentSelection, itemValue];
      updateServerCapability(
        serverName,
        capability,
        nextSelection.length === allItems.length ? 'all' : nextSelection,
      );
      return;
    }

    updateServerCapability(serverName, capability, [itemValue]);
  };

  const isServerSelected = (serverName: string) => {
    const serverConfig = normalizedValue.find((config) => config.name === serverName);
    return Boolean(serverConfig && hasAnyCapabilitySelection(serverConfig));
  };

  const isServerPartiallySelected = (serverName: string) => {
    const serverConfig = normalizedValue.find((config) => config.name === serverName);
    if (!serverConfig) return false;

    return (['tools', 'prompts', 'resources'] as CapabilityKey[]).some((capability) => {
      const selection = serverConfig[capability];
      return Array.isArray(selection) && selection.length > 0;
    });
  };

  const isCapabilityItemSelected = (
    serverName: string,
    capability: CapabilityKey,
    itemValue: string,
  ) => {
    const serverConfig = normalizedValue.find((config) => config.name === serverName);
    if (!serverConfig) return false;

    const selection = serverConfig[capability];
    if (selection === 'all') return true;
    return Array.isArray(selection) ? selection.includes(itemValue) : false;
  };

  const getSelectedCapabilityCount = (server: Server, capability: CapabilityKey) => {
    const serverConfig = normalizedValue.find((config) => config.name === server.name);
    if (!serverConfig) return 0;

    const items = getCapabilityItems(server, capability);
    const selection = serverConfig[capability];
    if (selection === 'all') return items.length;
    if (Array.isArray(selection)) {
      const itemSet = new Set(items.map((item) => item.value));
      return selection.filter((item) => itemSet.has(item)).length;
    }
    return 0;
  };

  const capabilityConfigs: Array<{
    key: CapabilityKey;
    titleKey: string;
    countKey: string;
    allKey: string;
    tabKey: string;
    icon: React.ReactNode;
  }> = [
    {
      key: 'tools',
      titleKey: 'groups.toolSelection',
      countKey: 'groups.toolsSelected',
      allKey: 'groups.allTools',
      tabKey: 'groups.tabTools',
      icon: <Wrench size={13} />,
    },
    {
      key: 'prompts',
      titleKey: 'groups.promptSelection',
      countKey: 'groups.promptsSelected',
      allKey: 'groups.allPrompts',
      tabKey: 'groups.tabPrompts',
      icon: <MessageSquare size={13} />,
    },
    {
      key: 'resources',
      titleKey: 'groups.resourceSelection',
      countKey: 'groups.resourcesSelected',
      allKey: 'groups.allResources',
      tabKey: 'groups.tabResources',
      icon: <FileText size={13} />,
    },
  ];

  // Servers that expose the active capability tab; the tab filters which
  // servers are shown, and each shown server's active-capability items render
  // inline (no manual expand needed) so switching tabs updates content.
  const tabServers = React.useMemo(
    () => availableServers.filter((s) => getCapabilityItems(s, activeTab).length > 0),
    [availableServers, activeTab],
  );

  // Search + type filter for the server list (helps when many servers exist).
  const [serverSearch, setServerSearch] = useState('');
  const [serverTypeFilter, setServerTypeFilter] = useState<'all' | 'custom' | 'builtin'>('all');
  const filteredTabServers = React.useMemo(() => {
    const q = serverSearch.trim().toLowerCase();
    return tabServers.filter((s) => {
      if (serverTypeFilter === 'builtin' && s.config?.type !== 'builtin') return false;
      if (serverTypeFilter === 'custom' && s.config?.type === 'builtin') return false;
      if (q && !s.name.toLowerCase().includes(q)) return false;
      return true;
    });
  }, [tabServers, serverSearch, serverTypeFilter]);

  // Enabled builtin entries available for group-level selection.

  const getServerSummaryBadges = (server: Server) => {
    return capabilityConfigs
      .map(({ key }) => ({ key, count: getSelectedCapabilityCount(server, key) }))
      .filter((entry) => entry.count > 0);
  };

  return (
    <div className={cn('flex flex-col', className)}>
      {/* Capability tab bar: part of the same card as the content below */}
      <div
        className="flex items-center gap-1 flex-wrap -mx-4 -mt-4 mb-4 px-4 pb-3"
        style={{ borderBottom: '1px solid var(--hub-line-2)' }}
      >
        {capabilityConfigs.map((cfg) => {
          // Total selectable items under this tab = per-server capability items
          // across all servers (the builtin "mcphub-desktop" server is included
          // in availableServers, so its builtin prompts/resources count here).
          const total = availableServers.reduce(
            (sum, s) => sum + getCapabilityItems(s, cfg.key).length,
            0,
          );
          const selected = availableServers.reduce(
            (sum, s) => sum + getSelectedCapabilityCount(s, cfg.key),
            0,
          );

          const isActive = activeTab === cfg.key;
          return (
            <button
              key={cfg.key}
              type="button"
              onClick={() => setActiveTab(cfg.key)}
              className="inline-flex items-center gap-1.5 px-3 text-[12px]"
              style={{
                height: 24,
                borderRadius: 5,
                background: isActive ? 'var(--hub-bg-2)' : 'transparent',
                color: isActive ? 'var(--hub-ink)' : 'var(--hub-ink-3)',
                border: '1px solid ' + (isActive ? 'var(--hub-line)' : 'transparent'),
              }}
            >
              {cfg.icon}
              {t(cfg.tabKey)}
              <span className="hub-mono" style={{ fontSize: 11, color: 'var(--hub-ink-3)' }}>
                {selected}/{total}
              </span>
            </button>
          );
        })}
      </div>

      {/* Server search + type filter */}
      <div className="flex items-center gap-2 mb-3 flex-wrap">
        <div
          className="hub-card flex items-center"
          style={{ padding: 2, borderRadius: 7, background: 'var(--hub-surface)' }}
        >
          {(
            [
              ['all', t('common.all') || 'All'],
              ['custom', t('server.typeCustom')],
              ['builtin', t('server.typeBuiltin')],
            ] as ['all' | 'custom' | 'builtin', string][]
          ).map(([k, l]) => (
            <button
              key={k}
              type="button"
              onClick={() => setServerTypeFilter(k)}
              className="inline-flex items-center px-2.5 text-[12px]"
              style={{
                height: 22,
                borderRadius: 5,
                background: serverTypeFilter === k ? 'var(--hub-bg-2)' : 'transparent',
                color: serverTypeFilter === k ? 'var(--hub-ink)' : 'var(--hub-ink-3)',
                border: '1px solid ' + (serverTypeFilter === k ? 'var(--hub-line)' : 'transparent'),
              }}
            >
              {l}
            </button>
          ))}
        </div>
        <div
          className="hub-card flex items-center gap-2 px-2.5 flex-1"
          style={{ height: 28, background: 'var(--hub-surface)', maxWidth: 260 }}
        >
          <Search size={13} style={{ color: 'var(--hub-ink-3)' }} />
          <input
            value={serverSearch}
            onChange={(e) => setServerSearch(e.target.value)}
            className="flex-1 bg-transparent outline-none text-[13px]"
            style={{ color: 'var(--hub-ink)' }}
            placeholder={t('market.searchPlaceholder') || 'Search…'}
          />
          {serverSearch && (
            <button type="button" onClick={() => setServerSearch('')} className="hub-icon-btn sm">
              <X size={11} />
            </button>
          )}
        </div>
      </div>

      <PaginatedItems items={filteredTabServers} pageSize={5} listClassName="space-y-3">
        {(server) => {
          const isSelected = isServerSelected(server.name);
          const isPartiallySelected = isServerPartiallySelected(server.name);
          const serverConfig = normalizedValue.find((config) => config.name === server.name);
          const summaryBadges = getServerSummaryBadges(server);
          const costMap = costMapForServer(server.name);
          const isExpanded = expandedServers.has(server.name);

          return (
            <div
              key={server.name}
              className="border border-gray-200 dark:border-gray-700 rounded-lg hover:border-gray-300 hover:bg-gray-50 dark:bg-gray-800 dark:hover:bg-gray-700 transition-colors"
            >
              <div
                className="flex items-center justify-between p-3 rounded-lg transition-colors cursor-pointer"
                onClick={() => toggleServerExpanded(server.name)}
              >
                <div
                  className="flex items-center space-x-3"
                  onClick={(e) => {
                    e.stopPropagation();
                    toggleServer(server.name);
                  }}
                >
                  <input
                    type="checkbox"
                    checked={isSelected || isPartiallySelected}
                    onChange={() => toggleServer(server.name)}
                    className="hub-checkbox"
                  />
                  <span className="font-medium text-gray-900 cursor-pointer select-none">
                    {server.name}
                  </span>
                  {server.config?.type === 'builtin' && (
                    <span
                      className="hub-tag flex-shrink-0"
                      style={{ background: 'var(--hub-accent-soft)', color: 'var(--hub-accent)' }}
                      title={t('server.builtinServer')}
                    >
                      {t('server.builtinBadge')}
                    </span>
                  )}
                </div>

                <div className="flex items-center space-x-3">
                  {getServerSelectedCost(server) > 0 && (
                    <span className="text-sm text-gray-400 hub-mono" title={t('cost.estimate')}>
                      Σ {formatTokens(getServerSelectedCost(server))}
                    </span>
                  )}
                  {summaryBadges.map(({ key, count }) => (
                    <span key={key} className="text-sm text-green-600 flex items-center gap-1">
                      {key === 'tools' ? (
                        <Wrench size={14} />
                      ) : key === 'prompts' ? (
                        <MessageSquare size={14} />
                      ) : (
                        <FileText size={14} />
                      )}{' '}
                      {count}
                    </span>
                  ))}
                  <svg
                    className={cn('w-5 h-5 text-gray-400 transition-transform', isExpanded && 'rotate-180')}
                    fill="none"
                    stroke="currentColor"
                    viewBox="0 0 24 24"
                  >
                    <path
                      strokeLinecap="round"
                      strokeLinejoin="round"
                      strokeWidth={2}
                      d="M19 9l-7 7-7-7"
                    />
                  </svg>
                </div>
              </div>

              {isExpanded && (
                <div className="border-t border-gray-200 dark:border-gray-700 bg-gray-50 dark:bg-gray-800 p-3">
                  <div className="space-y-4">
                    {serverConfig && (
                    <div className="space-y-1" onClick={(e) => e.stopPropagation()}>
                      <label className="block text-xs font-medium text-gray-600">
                        {t('groups.alias')}
                      </label>
                      <input
                        type="text"
                        value={serverConfig.alias || ''}
                        placeholder={server.name}
                        onChange={(event) => updateServerAlias(server.name, event.target.value)}
                        className="w-full rounded-md border border-gray-300 bg-white px-2.5 py-1.5 text-sm text-gray-800 focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500 dark:border-gray-600 dark:bg-gray-900 dark:text-gray-100"
                      />
                    </div>
                  )}
                  {(() => {
                      const activeCap = capabilityConfigs.find((c) => c.key === activeTab);
                      if (!activeCap) return null;
                      const { key, titleKey, countKey, allKey } = activeCap;
                      const items = getCapabilityItems(server, key);

                      // No items for this server under the active tab → show nothing
                      // (the per-tab empty state below handles the "no prompts/resources
                      // anywhere" case).
                      if (items.length === 0) {
                        return null;
                      }

                      const selectedCount = getSelectedCapabilityCount(server, key);
                      const allSelected =
                        serverConfig?.[key] === 'all' || selectedCount === items.length;

                      return (
                        <div key={key}>
                          <div className="flex items-center justify-between mb-3">
                            <span className="text-sm font-medium text-gray-700">{t(titleKey)}</span>
                            <div className="flex items-center gap-3">
                              {serverConfig && (
                                <span className="text-xs text-green-600">
                                  {allSelected
                                    ? `(${t(allKey)} ${items.length}/${items.length})`
                                    : `(${t(countKey)} ${selectedCount}/${items.length})`}
                                </span>
                              )}
                              {serverConfig && getSelectedCapabilityCost(server, key) > 0 && (
                                <span
                                  className="text-xs text-gray-400 hub-mono"
                                  title={t('cost.estimate')}
                                >
                                  Σ {formatTokens(getSelectedCapabilityCost(server, key))}
                                </span>
                              )}
                              <button
                                type="button"
                                onClick={() => {
                                  updateServerCapability(
                                    server.name,
                                    key,
                                    allSelected ? [] : 'all',
                                  );
                                }}
                                className="text-sm text-blue-600 hover:text-blue-800 transition-colors"
                              >
                                {allSelected ? t('groups.selectNone') : t('groups.selectAll')}
                              </button>
                            </div>
                          </div>

                          <PaginatedItems items={items} pageSize={5}>
                            {(item) => {
                              const isChecked = isCapabilityItemSelected(
                                server.name,
                                key,
                                item.value,
                              );
                              const descriptionInfo =
                                key === 'tools'
                                  ? getToolDescriptionInfo(
                                      {
                                        description: item.description,
                                        defaultDescription: item.defaultDescription,
                                        hasDescriptionOverride: item.hasDescriptionOverride,
                                      },
                                      t('tool.noDescription'),
                                    )
                                  : null;
                              const descriptionTitle = descriptionInfo?.hasDescriptionOverride
                                ? t('tool.defaultDescriptionTooltip', {
                                    description: descriptionInfo.defaultDescription,
                                  })
                                : item.description;

                              return (
                                <label
                                  key={item.key}
                                  className="flex min-w-0 items-center gap-2 text-sm"
                                >
                                  <input
                                    type="checkbox"
                                    checked={isChecked}
                                    onChange={() =>
                                      toggleCapabilityItem(server.name, key, item.value)
                                    }
                                    className="hub-checkbox sm"
                                  />
                                  <span className="text-gray-700 break-all whitespace-nowrap flex-shrink-0">
                                    {item.value}
                                  </span>
                                  {(item.description ||
                                    descriptionInfo?.hasDescriptionOverride) && (
                                    <span className="min-w-0 flex items-center gap-1 text-gray-400 text-xs truncate">
                                      <span
                                        className="truncate"
                                        title={descriptionTitle || undefined}
                                      >
                                        {descriptionInfo
                                          ? descriptionInfo.currentDescription
                                          : item.description}
                                      </span>
                                      {descriptionInfo?.hasDescriptionOverride && (
                                        <span
                                          className="inline-flex flex-shrink-0 items-center rounded-full border border-amber-200 bg-amber-50 px-1.5 py-0.5 text-[10px] font-medium text-amber-700 dark:border-amber-700/60 dark:bg-amber-900/20 dark:text-amber-300"
                                          title={descriptionTitle || undefined}
                                        >
                                          {t('tool.descriptionModifiedBadge')}
                                        </span>
                                      )}
                                    </span>
                                  )}
                                  {costMap.get(item.key) != null && (
                                    <span
                                      className="text-xs text-gray-400 hub-mono whitespace-nowrap ml-auto flex-shrink-0"
                                      title={t('cost.estimate')}
                                    >
                                      Σ {formatTokens(costMap.get(item.key)!)}
                                    </span>
                                  )}
                                </label>
                              );
                            }}
                          </PaginatedItems>
                        </div>
                      );
                    })()}
                  </div>
                </div>
              )}
            </div>
          );
        }}
      </PaginatedItems>

      {availableServers.length === 0 && (
        <p className="text-gray-500 text-sm">{t('groups.noServerOptions')}</p>
      )}
      {availableServers.length > 0 &&
        activeTab === 'prompts' &&
        filteredTabServers.length === 0 && (
          <p className="text-gray-500 text-sm">{t('groups.noPrompts')}</p>
        )}
      {availableServers.length > 0 &&
        activeTab === 'resources' &&
        filteredTabServers.length === 0 && (
          <p className="text-gray-500 text-sm">{t('groups.noResources')}</p>
        )}
    </div>
  );
};
