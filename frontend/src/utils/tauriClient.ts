import { invoke } from '@tauri-apps/api/core';

/**
 * Detect whether running inside a Tauri desktop app.
 */
export const isTauri = (): boolean =>
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

// ---------------------------------------------------------------------------
// REST → Tauri command routing
// ---------------------------------------------------------------------------

interface RouteResult {
  command: string;
  args: Record<string, unknown>;
}

/**
 * Map an HTTP method + endpoint path to a Tauri command name + args.
 * Endpoint is the path WITHOUT the /api prefix, e.g. "/servers", "/auth/login".
 */
export function mapRestToCommand(method: string, endpoint: string, body?: unknown): RouteResult {
  const m = method.toUpperCase();
  // Normalize: strip leading slash and query string (Tauri commands don't use query params)
  const p = endpoint.replace(/^\//, '').split('?')[0];
  const segs = p.split('/');

  // Auth
  if (p === 'auth/login' && m === 'POST')
    return { command: 'login', args: { request: body } };
  if (p === 'auth/register' && m === 'POST') {
    const b = body as Record<string, unknown>;
    return { command: 'register', args: { username: b?.username, password: b?.password } };
  }
  if (p === 'auth/logout' && m === 'POST')
    return { command: 'logout', args: {} };
  // /auth/user and /auth/me both resolve current session
  if ((p === 'auth/user' || p === 'auth/me') && m === 'GET')
    return { command: 'get_current_user', args: {} };
  // /better-auth/user delegates to the same session lookup in Tauri
  if (p === 'better-auth/user' && m === 'GET')
    return { command: 'get_current_user', args: {} };
  if (p === 'auth/change-password' && m === 'POST') {
    const b = body as Record<string, unknown>;
    return {
      command: 'change_password',
      args: { oldPassword: b?.currentPassword, newPassword: b?.newPassword },
    };
  }
  // legacy path variant
  if (p === 'auth/password' && m === 'PUT') {
    const b = body as Record<string, unknown>;
    return {
      command: 'change_password',
      args: { oldPassword: b?.oldPassword ?? b?.currentPassword, newPassword: b?.newPassword },
    };
  }

  // Servers
  if (p === 'servers' && m === 'GET') return { command: 'list_servers', args: {} };
  if (p === 'servers' && m === 'POST') {
    // Frontend sends { name, config: { type, command, ... } } — flatten into a single ServerConfig
    const b = body as { name?: string; config?: Record<string, unknown> } | null;
    return { command: 'add_server', args: { config: { name: b?.name, ...b?.config } } };
  }
  // Batch add — handled client-side in invokeMapped to avoid a separate Rust command
  if (p === 'servers/batch' && m === 'POST') {
    const b = body as { servers?: Array<{ name: string; config?: Record<string, unknown> }> } | null;
    return { command: '__batch_servers__', args: { servers: b?.servers ?? [] } };
  }
  if (segs[0] === 'servers' && segs.length === 2 && m === 'GET')
    return { command: 'get_server', args: { name: decodeURIComponent(segs[1]) } };
  if (segs[0] === 'servers' && segs.length === 2 && m === 'PUT') {
    // Frontend sends { config: { type, command, ... }, newName?: '...' } — flatten
    const b = body as { config?: Record<string, unknown>; newName?: string } | null;
    const originalName = decodeURIComponent(segs[1]);
    return {
      command: 'update_server',
      args: {
        name: originalName,
        config: { name: b?.newName ?? originalName, ...b?.config },
      },
    };
  }
  if (segs[0] === 'servers' && segs.length === 2 && m === 'DELETE')
    return { command: 'delete_server', args: { name: decodeURIComponent(segs[1]) } };
  // frontend uses POST for toggle (apiPost), accept both PUT and POST
  if (segs[0] === 'servers' && segs[2] === 'toggle' && (m === 'PUT' || m === 'POST'))
    return { command: 'toggle_server', args: { name: decodeURIComponent(segs[1]) } };
  if (segs[0] === 'servers' && segs[2] === 'reload' && m === 'POST')
    return { command: 'reload_server', args: { name: decodeURIComponent(segs[1]) } };

  // Per-server tool/prompt/resource toggle & description overrides.
  if (
    segs[0] === 'servers' &&
    segs.length >= 5 &&
    (segs[2] === 'tools' || segs[2] === 'prompts' || segs[2] === 'resources') &&
    segs[4] === 'toggle' &&
    m === 'POST'
  ) {
    const itemType = segs[2] === 'prompts' ? 'prompt' : segs[2] === 'resources' ? 'resource' : 'tool';
    const b = body as { enabled?: boolean } | null;
    return {
      command: 'toggle_server_item',
      args: {
        serverName: decodeURIComponent(segs[1]),
        itemType,
        itemName: decodeURIComponent(segs[3]),
        enabled: b?.enabled ?? true,
      },
    };
  }
  if (
    segs[0] === 'servers' &&
    segs.length >= 5 &&
    (segs[2] === 'tools' || segs[2] === 'prompts' || segs[2] === 'resources') &&
    segs[4] === 'description'
  ) {
    const itemType = segs[2] === 'prompts' ? 'prompt' : segs[2] === 'resources' ? 'resource' : 'tool';
    if (m === 'PUT') {
      const b = body as { description?: string } | null;
      return {
        command: 'update_server_item_description',
        args: {
          serverName: decodeURIComponent(segs[1]),
          itemType,
          itemName: decodeURIComponent(segs[3]),
          description: b?.description ?? null,
        },
      };
    }
    if (m === 'DELETE') {
      return {
        command: 'reset_server_item_description',
        args: {
          serverName: decodeURIComponent(segs[1]),
          itemType,
          itemName: decodeURIComponent(segs[3]),
        },
      };
    }
    if (m === 'GET') {
      return {
        command: 'list_server_item_configs',
        args: {
          serverName: decodeURIComponent(segs[1]),
          itemType,
        },
      };
    }
  }

  // Groups
  // Helper: normalize servers array — accepts string[] or IGroupServerConfig[] and returns string[]
  const toServerNames = (arr: Array<unknown>): string[] =>
    arr
      .map(s => (typeof s === 'string' ? s : (s as Record<string, unknown>)?.name ?? ''))
      .filter(Boolean) as string[];

  if (p === 'groups' && m === 'GET') return { command: 'list_groups', args: {} };
  if (p === 'groups' && m === 'POST') {
    // Rust GroupPayload.servers: Vec<String> — always flatten IGroupServerConfig[] to string[]
    const b = body as { name?: string; description?: string; servers?: Array<unknown> } | null;
    return {
      command: 'add_group',
      args: {
        payload: {
          name: b?.name ?? '',
          description: b?.description,
          servers: toServerNames(b?.servers ?? []),
        },
      },
    };
  }
  // Batch group import — loop client-side
  if (p === 'groups/batch' && m === 'POST') {
    const b = body as { groups?: Array<Record<string, unknown>> } | null;
    return { command: '__batch_groups__', args: { groups: b?.groups ?? [] } };
  }
  if (segs[0] === 'groups' && segs.length === 2 && m === 'PUT') {
    // Rust GroupPayload.servers: Vec<String> — flatten IGroupServerConfig[] to string[]
    const b = body as { name?: string; description?: string; servers?: Array<unknown> } | null;
    return {
      command: 'update_group',
      args: {
        id: segs[1],
        payload: {
          name: b?.name ?? '',
          description: b?.description,
          servers: toServerNames(b?.servers ?? []),
        },
      },
    };
  }
  if (segs[0] === 'groups' && segs.length === 2 && m === 'DELETE')
    return { command: 'delete_group', args: { id: segs[1] } };
  // Add server to group: POST /groups/:id/servers { serverName }
  if (segs[0] === 'groups' && segs[2] === 'servers' && segs.length === 3 && m === 'POST') {
    const b = body as { serverName?: string } | null;
    return {
      command: '__group_add_server__',
      args: { id: decodeURIComponent(segs[1]), serverName: b?.serverName ?? '' },
    };
  }
  // Batch update servers in group: PUT /groups/:id/servers/batch { servers }
  if (
    segs[0] === 'groups' &&
    segs[2] === 'servers' &&
    segs[3] === 'batch' &&
    segs.length === 4 &&
    m === 'PUT'
  ) {
    const b = body as { servers?: Array<unknown> } | null;
    return {
      command: '__group_update_servers__',
      args: { id: decodeURIComponent(segs[1]), servers: b?.servers ?? [] },
    };
  }
  // Remove server from group: DELETE /groups/:id/servers/:serverName
  if (segs[0] === 'groups' && segs[2] === 'servers' && segs.length === 4 && m === 'DELETE') {
    return {
      command: '__group_remove_server__',
      args: {
        id: decodeURIComponent(segs[1]),
        serverName: decodeURIComponent(segs[3]),
      },
    };
  }

  // Tools
  if (p === 'tools' && m === 'GET') return { command: 'list_tools', args: {} };
  // Express form: POST /tools/call/:server with body { toolName, arguments }
  if (segs[0] === 'tools' && segs[1] === 'call' && segs.length === 3 && m === 'POST') {
    const b = body as { toolName?: string; arguments?: unknown } | null;
    return {
      command: 'call_tool',
      args: {
        serverName: decodeURIComponent(segs[2]),
        toolName: b?.toolName ?? '',
        arguments: b?.arguments ?? {},
      },
    };
  }
  // OpenAPI form: POST /tools/:server/:toolName with body = arguments
  if (segs[0] === 'tools' && segs.length === 3 && segs[1] !== 'call' && m === 'POST') {
    return {
      command: 'call_tool',
      args: {
        serverName: decodeURIComponent(segs[1]),
        toolName: decodeURIComponent(segs[2]),
        arguments: body ?? {},
      },
    };
  }
  // Generic: POST /tools/call with body { serverName, toolName, arguments }
  if (p === 'tools/call' && m === 'POST') {
    const b = body as { serverName?: string; toolName?: string; arguments?: unknown } | null;
    return {
      command: 'call_tool',
      args: {
        serverName: b?.serverName ?? '',
        toolName: b?.toolName ?? '',
        arguments: b?.arguments ?? {},
      },
    };
  }

  // Users
  if (p === 'users' && m === 'GET') return { command: 'list_users', args: {} };
  if (p === 'users' && m === 'POST') {
    const b = body as Record<string, unknown>;
    return { command: 'add_user', args: { payload: { ...b, role: b?.isAdmin ? 'admin' : 'user' } } };
  }
  if (segs[0] === 'users' && segs.length === 2 && m === 'PUT') {
    const b = body as Record<string, unknown>;
    return {
      command: 'update_user',
      args: { username: segs[1], isAdmin: b?.isAdmin, newPassword: b?.newPassword },
    };
  }
  if (segs[0] === 'users' && segs.length === 2 && m === 'DELETE')
    return { command: 'delete_user', args: { username: segs[1] } };

  // Settings (full config + bearerKeys, used by SettingsContext)
  if (p === 'settings' && m === 'GET') return { command: 'get_settings', args: {} };
  // System config partial-merge update (used by all updateXxxConfig calls)
  if (p === 'system-config' && m === 'PUT')
    return { command: 'update_system_config', args: { config: body } };
  // MCP settings export (query string included in segs[1])
  if (segs[0] === 'mcp-settings' && segs[1]?.startsWith('export') && m === 'GET')
    return { command: 'export_settings', args: {} };

  // Config (legacy paths kept for compatibility)
  if (p === 'config' && m === 'GET') return { command: 'get_system_config', args: {} };
  if (p === 'config' && m === 'PUT')
    return { command: 'update_system_config', args: { config: body } };
  if (p === 'config/import' && m === 'POST')
    return { command: 'import_settings', args: { json: JSON.stringify(body) } };
  if (p === 'config/export' && m === 'GET') return { command: 'export_settings', args: {} };

  // Logs
  if (p === 'logs' && m === 'GET') return { command: 'get_logs', args: { query: {} } };
  if (p === 'logs' && m === 'DELETE') return { command: 'clear_logs', args: {} };
  if (p === 'logs/activity' && m === 'GET')
    return { command: 'get_tool_activities', args: { page: 1, pageSize: 50 } };

  // Bearer key management
  if (segs[0] === 'auth' && segs[1] === 'keys') {
    if (m === 'GET') return { command: 'list_bearer_keys', args: {} };
    if (m === 'POST') return { command: 'create_bearer_key', args: { payload: body } };
    if (m === 'PUT') return { command: 'update_bearer_key', args: { id: segs[2], payload: body } };
    if (m === 'DELETE') return { command: 'delete_bearer_key', args: { id: segs[2] } };
  }

  // Builtin prompts CRUD
  if (segs[0] === 'prompts') {
    if (segs[1] === 'call') return { command: 'call_builtin_prompt', args: { id: segs[2] ?? '', args: body ?? {} } };
    if (m === 'GET' && segs.length === 1) return { command: 'list_builtin_prompts', args: {} };
    if (m === 'GET' && segs.length === 2) return { command: 'get_builtin_prompt', args: { id: segs[1] } };
    if (m === 'POST') return { command: 'create_builtin_prompt', args: { payload: body } };
    if (m === 'PUT') return { command: 'update_builtin_prompt', args: { id: segs[1], payload: body } };
    if (m === 'DELETE') return { command: 'delete_builtin_prompt', args: { id: segs[1] } };
  }

  // Builtin resources CRUD
  if (segs[0] === 'resources') {
    if (m === 'GET' && segs.length === 1) return { command: 'list_builtin_resources', args: {} };
    if (m === 'GET' && segs.length === 2) return { command: 'get_builtin_resource', args: { id: segs[1] } };
    if (m === 'POST') return { command: 'create_builtin_resource', args: { payload: body } };
    if (m === 'PUT') return { command: 'update_builtin_resource', args: { id: segs[1], payload: body } };
    if (m === 'DELETE') return { command: 'delete_builtin_resource', args: { id: segs[1] } };
  }

  // MCP HTTP pass-through endpoints (/mcp/*) — not available; tools are accessed via invoke directly
  if (segs[0] === 'mcp')
    return { command: '__stub__', args: { __response: { success: false, message: 'MCP HTTP proxy is not available in desktop mode' } } };

  // Configuration template import/export — not implemented in desktop
  if (segs[0] === 'templates')
    return { command: '__stub__', args: { __response: { success: false, message: 'Configuration templates are not available in desktop mode' } } };

  // MCPB upload — multipart upload not supported in desktop
  if (segs[0] === 'mcpb')
    return { command: '__stub__', args: { __response: { success: false, message: 'MCPB upload is not available in desktop mode' } } };

  // User stats endpoint — desktop doesn't track user stats
  if (p === 'users-stats' && m === 'GET')
    return { command: '__stub__', args: { __response: { success: true, data: { totalUsers: 0, adminUsers: 0 } } } };

  // Activity log endpoints
  if (segs[0] === 'activities') {
    if (p === 'activities/available') return { command: 'get_activity_available', args: {} };
    if (p === 'activities/filters') return { command: 'get_activity_filters', args: {} };
    if (segs[1] === 'stats') return { command: 'get_activity_stats', args: {} };
    if (m === 'GET') {
      const qsIdx = endpoint.indexOf('?');
      const qs = qsIdx >= 0 ? new URLSearchParams(endpoint.slice(qsIdx + 1)) : new URLSearchParams();
      return {
        command: 'get_tool_activities',
        args: {
          page: Number(qs.get('page') ?? 1),
          pageSize: Number(qs.get('pageSize') ?? qs.get('page_size') ?? 20),
          server: qs.get('server') ?? null,
          status: qs.get('status') ?? null,
          tool: qs.get('tool') ?? null,
        },
      };
    }
    if (m === 'DELETE') return { command: 'clear_tool_activities', args: {} };
    return { command: '__stub__', args: { __response: { success: false, message: 'Not found' } } };
  }

  // Market endpoints — reads from bundled servers.json catalog
  if (segs[0] === 'market') {
    if (p === 'market/categories') return { command: 'get_market_categories', args: {} };
    if (p === 'market/tags') return { command: 'get_market_tags', args: {} };
    // /market/categories/:cat and /market/tags/:tag — filter list
    if (segs[1] === 'categories' && segs[2])
      return { command: 'list_market_servers', args: { category: decodeURIComponent(segs[2]) } };
    if (segs[1] === 'tags' && segs[2])
      return { command: 'list_market_servers', args: { tag: decodeURIComponent(segs[2]) } };
    // /market/servers/search?query=...
    if (segs[1] === 'servers' && segs[2] === 'search') {
      const qsIdx = endpoint.indexOf('?');
      const qs = qsIdx >= 0 ? new URLSearchParams(endpoint.slice(qsIdx + 1)) : new URLSearchParams();
      return { command: 'list_market_servers', args: { q: qs.get('query') ?? '' } };
    }
    // /market/servers/:name
    if (segs[1] === 'servers' && segs[2])
      return { command: 'get_market_server', args: { name: segs[2] } };
    // /market/servers (list all)
    if (m === 'GET') return { command: 'list_market_servers', args: {} };
    return { command: '__stub__', args: { __response: { success: true, data: null } } };
  }

  if (segs[0] === 'registry') {
    if (segs[1] === 'servers' && segs[2] && segs[3] === 'versions' && m === 'GET')
      return { command: 'get_registry_server_versions', args: { name: decodeURIComponent(segs[2]) } };
    if (segs[1] === 'servers' && m === 'GET') {
      const qsIdx = endpoint.indexOf('?');
      const qs = qsIdx >= 0 ? new URLSearchParams(endpoint.slice(qsIdx + 1)) : new URLSearchParams();
      return {
        command: 'list_registry_servers',
        args: {
          limit: qs.get('limit') ? Number(qs.get('limit')) : null,
          cursor: qs.get('cursor') ?? null,
          search: qs.get('search') ?? null,
        },
      };
    }
    return { command: '__stub__', args: { __response: { success: true, data: null } } };
  }

  if (segs[0] === 'cloud') {
    // /cloud/servers/search?query=...
    if (segs[1] === 'servers' && segs[2] === 'search' && m === 'GET') {
      const qsIdx = endpoint.indexOf('?');
      const qs = qsIdx >= 0 ? new URLSearchParams(endpoint.slice(qsIdx + 1)) : new URLSearchParams();
      return { command: '__cloud_server_search__', args: { query: qs.get('query') ?? '' } };
    }
    // /cloud/servers/:name/tools
    if (segs[1] === 'servers' && segs[2] && segs[3] === 'tools' && m === 'GET')
      return { command: 'get_cloud_server_tools', args: { server: decodeURIComponent(segs[2]) } };
    // /cloud/servers/:name — return single server object from list
    if (segs[1] === 'servers' && segs[2] && m === 'GET')
      return { command: '__cloud_server_by_name__', args: { name: decodeURIComponent(segs[2]) } };
    // /cloud/servers
    if (segs[1] === 'servers' && m === 'GET')
      return { command: 'list_cloud_servers', args: {} };
    // /cloud/categories and /cloud/tags — no separate cloud equivalents, return empty
    if (segs[1] === 'categories' || segs[1] === 'tags')
      return { command: '__stub__', args: { __response: { success: true, data: [] } } };
    return { command: '__stub__', args: { __response: { success: true, data: null } } };
  }

  throw new Error(`[tauriClient] Unmapped route: ${m} /${p}`);
}

// ---------------------------------------------------------------------------
// Response transformation: Tauri raw results → HTTP-API-compatible shapes
// ---------------------------------------------------------------------------

/**
 * Transform a raw Tauri invoke result into the same JSON shape the HTTP API
 * returns, so the existing frontend code works without modification.
 */
export function transformTauriResponse(command: string, result: unknown): unknown {
  // ── Auth commands ─────────────────────────────────────────────────────────
  if (command === 'login' || command === 'register') {
    const t = result as { token: string; userId: string; username: string; role: string } | null;
    if (!t) return { success: false, message: 'Authentication failed' };
    return {
      success: true,
      token: t.token,
      user: { username: t.username, isAdmin: t.role === 'admin' },
    };
  }
  if (command === 'get_current_user') {
    const u = result as { id: string; username: string; role: string } | null;
    if (!u) return { success: false, message: 'Not authenticated' };
    return { success: true, user: { username: u.username, isAdmin: u.role === 'admin' } };
  }
  if (command === 'logout' || command === 'change_password') {
    return { success: true };
  }

  // ── Void-return commands ──────────────────────────────────────────────────
  if (result === null || result === undefined) {
    return { success: true };
  }

  // ── Config commands ───────────────────────────────────────────────────────
  // get_settings returns { systemConfig, bearerKeys } already – just wrap in success envelope
  if (command === 'get_settings') {
    return { success: true, data: result };
  }
  if (command === 'get_system_config' || command === 'update_system_config') {
    return { success: true, data: { systemConfig: result } };
  }

  // ── User list: map role → isAdmin ─────────────────────────────────────────
  if (command === 'list_users') {
    const arr = result as Array<{ id: string; username: string; role: string; createdAt: string }>;
    const users = arr.map(u => ({ ...u, isAdmin: u.role === 'admin' }));
    return { success: true, data: users, total: users.length, page: 1, pageSize: users.length };
  }

  // ── Server commands: Rust ServerInfo { config, status: obj, tools }
  //    → Frontend Server { name, status: string, tools, config, enabled }
  const toFrontendServer = (si: Record<string, unknown>) => {
    const cfg = si.config as Record<string, unknown> | undefined;
    const st = si.status as Record<string, unknown> | undefined;
    return {
      name: cfg?.name ?? st?.name ?? '',
      status: st?.connected ? 'connected' : 'disconnected',
      error: st?.error ?? null,
      tools: si.tools ?? [],
      config: cfg,
      enabled: cfg?.enabled ?? true,
    };
  };
  if (command === 'list_servers') {
    const servers = (result as Record<string, unknown>[]).map(toFrontendServer);
    return { success: true, data: servers, total: servers.length, page: 1, pageSize: servers.length };
  }
  if (command === 'get_server') {
    if (!result) return { success: false, message: 'Server not found' };
    return { success: true, data: toFrontendServer(result as Record<string, unknown>) };
  }
  if (command === 'add_server' || command === 'update_server') {
    return { success: true, data: toFrontendServer(result as Record<string, unknown>) };
  }
  if (command === 'delete_server' || command === 'toggle_server' || command === 'reload_server') {
    return { success: true };
  }

  // ── Activity log commands ─────────────────────────────────────────────────
  if (command === 'get_activity_available') {
    return { success: true, data: result };
  }
  if (command === 'get_activity_filters') {
    const arr = Array.isArray(result) ? result : [];
    return { success: true, data: arr };
  }
  if (command === 'get_activity_stats') {
    return { success: true, data: result };
  }
  if (command === 'get_tool_activities') {
    const r = result as { data: unknown[]; page: number; pageSize: number; total: number } | null;
    if (!r) return { success: true, data: [], pagination: { page: 1, limit: 20, total: 0, totalPages: 1, hasNextPage: false, hasPrevPage: false } };
    const totalPages = Math.max(1, Math.ceil(r.total / (r.pageSize || 20)));
    return {
      success: true,
      data: r.data,
      pagination: {
        page: r.page,
        limit: r.pageSize,
        total: r.total,
        totalPages,
        hasNextPage: r.page < totalPages,
        hasPrevPage: r.page > 1,
      },
    };
  }
  if (command === 'clear_tool_activities') {
    return { success: true };
  }

  // ── call_tool: frontend reads response.content directly (not response.data.content)
  if (command === 'call_tool') {
    const r = result as { content?: unknown[]; isError?: boolean } | null;
    if (r?.isError) {
      return { success: false, content: r.content ?? [], message: 'Tool returned an error' };
    }
    return { success: true, content: r?.content ?? [] };
  }

  // ── list_tools: keep as plain array (consumers iterate or use .data)
  if (command === 'list_tools') {
    const arr = Array.isArray(result) ? result : [];
    return { success: true, data: arr, total: arr.length };
  }

  // ── get_logs: Rust LogEntry { id, level, message, serverName, createdAt }
  //    → Frontend LogEntry { timestamp, type, source, message, processId }
  if (command === 'get_logs') {
    const arr = Array.isArray(result) ? result : [];
    const logs = arr.map((e: Record<string, unknown>) => ({
      timestamp: e.createdAt ? new Date(e.createdAt as string).getTime() : Date.now(),
      type: (e.level as string) ?? 'info',
      source: (e.serverName as string) ?? 'system',
      message: (e.message as string) ?? '',
      processId: e.id as string,
    }));
    return { success: true, data: logs, total: logs.length };
  }

  // ── List commands ─────────────────────────────────────────────────────────
  if (Array.isArray(result)) {
    return { success: true, data: result, total: result.length, page: 1, pageSize: result.length };
  }

  // ── Generic object / scalar ───────────────────────────────────────────────
  return { success: true, data: result };
}

/**
 * Invoke a Tauri command and return an HTTP-API-compatible response object.
 * Commands prefixed with __stub__ never call invoke — they return args.__response directly.
 */
export async function invokeMapped<T>(command: string, args: Record<string, unknown>): Promise<T> {
  if (command === '__stub__') {
    return (args.__response as T) ?? ({ success: true } as T);
  }
  // Cloud: fetch all then filter by name
  if (command === '__cloud_server_by_name__') {
    const servers = await invoke<unknown[]>('list_cloud_servers', {});
    const name = String(args.name ?? '');
    const found = (servers ?? []).find((s: unknown) => {
      const srv = s as Record<string, unknown>;
      return srv.name === name || srv.config_name === name;
    });
    if (found) return { success: true, data: found } as T;
    return { success: false, message: 'Server not found' } as T;
  }
  // Cloud: search servers client-side
  if (command === '__cloud_server_search__') {
    const servers = await invoke<unknown[]>('list_cloud_servers', {});
    const query = String(args.query ?? '').toLowerCase();
    const filtered = query
      ? (servers ?? []).filter((s: unknown) => {
          const srv = s as Record<string, unknown>;
          return (
            String(srv.name ?? '').toLowerCase().includes(query) ||
            String(srv.description ?? '').toLowerCase().includes(query)
          );
        })
      : (servers ?? []);
    return { success: true, data: filtered } as T;
  }
  // Batch server add — loop client-side rather than adding a dedicated Rust command
  if (command === '__batch_servers__') {
    const servers = args.servers as Array<{ name: string; config?: Record<string, unknown> }>;
    const results: Array<{ name: string; success: boolean; message?: string }> = [];
    for (const server of servers) {
      try {
        const config = { name: server.name, ...server.config };
        await invoke('add_server', { config });
        results.push({ name: server.name, success: true });
      } catch (e) {
        results.push({ name: server.name, success: false, message: String(e) });
      }
    }
    const successCount = results.filter(r => r.success).length;
    const failureCount = results.length - successCount;
    return { success: true, data: { successCount, failureCount, results } } as T;
  }
  // Batch group import — loop client-side
  if (command === '__batch_groups__') {
    const groups = args.groups as Array<Record<string, unknown>>;
    const results: Array<{ name: string; success: boolean; message?: string }> = [];
    for (const g of groups) {
      const name = String(g?.name ?? '');
      try {
        // Normalize servers to string[] before passing to Rust
        const rawServers = Array.isArray(g.servers) ? g.servers as Array<unknown> : [];
        const servers = rawServers
          .map(s => (typeof s === 'string' ? s : (s as Record<string, unknown>)?.name ?? ''))
          .filter(Boolean);
        await invoke('add_group', { payload: { name, description: g.description, servers } });
        results.push({ name, success: true });
      } catch (e) {
        results.push({ name, success: false, message: String(e) });
      }
    }
    const successCount = results.filter(r => r.success).length;
    const failureCount = results.length - successCount;
    return { success: true, data: { successCount, failureCount, results } } as T;
  }
  // Group server-membership operations: synthesized via list_groups + update_group
  if (
    command === '__group_add_server__' ||
    command === '__group_remove_server__' ||
    command === '__group_update_servers__'
  ) {
    const id = String(args.id ?? '');
    const groups = (await invoke<Array<Record<string, unknown>>>('list_groups', {})) ?? [];
    const group = groups.find(g => String(g.id) === id || String(g.name) === id);
    if (!group) {
      return { success: false, message: `Group '${id}' not found` } as T;
    }
    const currentServers = Array.isArray(group.servers) ? (group.servers as unknown[]) : [];
    const namesOf = (list: unknown[]): string[] =>
      list
        .map(s => (typeof s === 'string' ? s : (s as { name?: string })?.name ?? ''))
        .filter(Boolean);
    let nextNames: string[];
    if (command === '__group_add_server__') {
      const sn = String(args.serverName ?? '');
      const set = new Set(namesOf(currentServers));
      if (sn) set.add(sn);
      nextNames = Array.from(set);
    } else if (command === '__group_remove_server__') {
      const sn = String(args.serverName ?? '');
      nextNames = namesOf(currentServers).filter(n => n !== sn);
    } else {
      nextNames = namesOf((args.servers as unknown[]) ?? []);
    }
    const payload = {
      name: group.name,
      description: group.description,
      servers: nextNames,
    };
    try {
      const updated = await invoke<Record<string, unknown>>('update_group', {
        id: String(group.id),
        payload,
      });
      return { success: true, data: updated } as T;
    } catch (e) {
      return { success: false, message: String(e) } as T;
    }
  }
  const raw = await invoke<unknown>(command, args);
  return transformTauriResponse(command, raw) as T;
}

