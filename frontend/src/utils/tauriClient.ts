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

  // Public config (skipAuth check, no auth required)
  if (p === 'public-config' && m === 'GET')
    return { command: 'get_public_config', args: {} };

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
  if (segs[0] === 'servers' && segs[2] === 'reinstall' && m === 'POST')
    return { command: 'reinstall_server', args: { name: decodeURIComponent(segs[1]) } };
  // Single-server package-update check (npx/uvx stdio). segs[2] === 'check-update'
  // on a 3-segment path only (deeper paths like .../tools/toggle must not match).
  if (segs[0] === 'servers' && segs.length === 3 && segs[2] === 'check-update' && m === 'POST')
    return { command: 'check_server_update', args: { name: decodeURIComponent(segs[1]) } };
  // Batch package-update check across all npx/uvx stdio servers.
  if (p === 'servers/check-stdio-updates' && m === 'POST')
    return { command: 'check_stdio_updates', args: {} };
  // Upstream OAuth disconnect (#984) — desktop has no upstream-OAuth token storage,
  // so this is a no-op stub. The UI button stays hidden (oauth.connected is never
  // populated by the Rust backend); the stub only guards against an unmapped-route error.
  if (segs[0] === 'servers' && segs[2] === 'oauth' && segs[3] === 'disconnect' && m === 'POST')
    return {
      command: '__stub__',
      args: { __response: { success: false, message: 'OAuth disconnect is not available in desktop mode' } },
    };

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
    // Rust GroupPayload.servers: Vec<JsonValue> — preserve full IGroupServerConfig[]
    const b = body as { name?: string; description?: string; servers?: Array<unknown> } | null;
    return {
      command: 'add_group',
      args: {
        payload: {
          name: b?.name ?? '',
          description: b?.description,
          servers: b?.servers ?? [],
          // Default [] = expose no builtins until the user explicitly selects.
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
    // Rust GroupPayload.servers: Vec<JsonValue> — preserve full IGroupServerConfig[]
    const b = body as { name?: string; description?: string; servers?: Array<unknown> } | null;
    return {
      command: 'update_group',
      args: {
        id: segs[1],
        payload: {
          name: b?.name ?? '',
          description: b?.description,
          servers: b?.servers ?? [],
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
  if (segs[0] === 'mcp-settings' && segs[1]?.startsWith('export') && m === 'GET') {
    // If serverName is provided, use the no-auth command for single server copy
    const qsIdx = endpoint.indexOf('?');
    const qs = qsIdx >= 0 ? new URLSearchParams(endpoint.slice(qsIdx + 1)) : new URLSearchParams();
    const serverName = qs.get('serverName');
    if (serverName) {
      return { command: 'get_server_config_for_copy', args: { serverName } };
    }
    return { command: 'export_settings', args: {} };
  }

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
  if (p === 'logs/cleanup' && m === 'POST') return { command: 'cleanup_old_logs', args: {} };

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
    if (segs[1] === 'stats') {
      // Pass filter params through to stats query
      const qsIdx = endpoint.indexOf('?');
      const qs = qsIdx >= 0 ? new URLSearchParams(endpoint.slice(qsIdx + 1)) : new URLSearchParams();
      return {
        command: 'get_activity_stats',
        args: {
          server: qs.get('server') ?? null,
          status: qs.get('status') ?? null,
          tool: qs.get('tool') ?? null,
        },
      };
    }
    // Cleanup old activities (by daysOld param) — must come before generic DELETE
    if (segs[1] === 'cleanup' && m === 'DELETE') {
      const qsIdx = endpoint.indexOf('?');
      const qs = qsIdx >= 0 ? new URLSearchParams(endpoint.slice(qsIdx + 1)) : new URLSearchParams();
      return { command: 'cleanup_activity_logs', args: { daysOld: Number(qs.get('daysOld') ?? 30) } };
    }
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

  // Cost endpoints — not implemented in desktop client
  // Context footprint / cost calculation
  if (segs[0] === 'cost' && segs[1] === 'servers') {
    return { command: 'get_server_costs', args: {} };
  }
  if (segs[0] === 'cost' && segs[1] === 'groups') {
    return { command: 'get_group_costs', args: {} };
  }
  if (segs[0] === 'cost') {
    return { command: '__stub__', args: { __response: { success: true, data: [] } } };
  }

  // Cache endpoints
  if (segs[0] === 'cache' && segs[1] === 'clear' && m === 'POST')
    return { command: 'clear_cache', args: {} };

  // Changelog endpoints — not implemented in desktop client
  if (segs[0] === 'changelog') {
    return { command: '__stub__', args: { __response: { success: true, data: { hasUpdate: false, entries: [] } } } };
  }

  // ── Skills (技能) — Phase 1: mock data via __stub__.
  //    Phase 2 will replace these with real Tauri commands
  //    (list_skill_agents, scan_skills_for_import, list_skills, get_skill,
  //     import_skills, export_skills_to_agents, delete_skill, save_skill_agents).
  if (segs[0] === 'skills') {
    // GET /skills/agents — list configured agents (Phase 2.2: real command)
    if (segs[1] === 'agents' && m === 'GET')
      return { command: 'list_skill_agents', args: {} };
    // PUT /skills/agents — save agents (Phase 2.2: real command)
    if (segs[1] === 'agents' && m === 'PUT')
      return { command: 'save_skill_agents', args: { agents: body } };

    // POST /skills/agents/create — create a custom agent (agent-management UI)
    if (segs[1] === 'agents' && segs[2] === 'create' && m === 'POST') {
      const b = body as { name?: string; skillsPath?: string } | null;
      return {
        command: 'create_skill_agent',
        args: { name: b?.name ?? '', skillsPath: b?.skillsPath ?? '' },
      };
    }
    // POST /skills/agents/delete — delete a custom agent by id
    if (segs[1] === 'agents' && segs[2] === 'delete' && m === 'POST') {
      const b = body as { id?: string } | null;
      return { command: 'delete_skill_agent', args: { id: b?.id ?? '' } };
    }

    // GET /skills/scan — scan all agents for importable skills (Phase 2.3: real command)
    if (segs[1] === 'scan' && m === 'GET')
      return { command: 'scan_skills_for_import', args: {} };

    // GET /skills/:id — single skill with exports (Phase 2.3: real command)
    if (m === 'GET' && segs.length === 2)
      return { command: 'get_skill', args: { id: decodeURIComponent(segs[1]) } };

    // GET /skills — list library skills (Phase 2.3: real command)
    if (m === 'GET' && segs.length === 1)
      return { command: 'list_skills', args: {} };

    // POST /skills/import — import selected skills (Phase 2.3: real command)
    if (segs[1] === 'import' && m === 'POST') {
      const b = body as { items?: unknown[] } | null;
      return { command: 'import_skills', args: { items: b?.items ?? [] } };
    }

    // POST /skills/scan-folder — scan a manually-selected folder for skills
    // (2-layer SKILL.md detection). Returns skills with agent_id="__manual__".
    if (segs[1] === 'scan-folder' && m === 'POST') {
      const b = body as { path?: string } | null;
      return { command: 'scan_folder_for_skills', args: { path: b?.path ?? '' } };
    }

    // POST /skills/export — export skills to agents (Phase 2.4: real command)
    if (segs[1] === 'export' && m === 'POST') {
      const b = body as { skillIds?: string[]; agentIds?: string[]; method?: string } | null;
      return {
        command: 'export_skills_to_agents',
        args: {
          skillIds: b?.skillIds ?? [],
          agentIds: b?.agentIds ?? [],
          method: b?.method ?? 'symlink',
        },
      };
    }

    // POST /skills/open-path — reveal an agent's skills path in the OS file
    // manager (Phase 2.6: real command; expands ~).
    if (segs[1] === 'open-path' && m === 'POST') {
      const b = body as { path?: string } | null;
      return { command: 'open_path_in_explorer', args: { path: b?.path ?? '' } };
    }

    // POST /skills/pick-directory — open the OS folder picker and return the
    // chosen path (Phase 2.6: real command; returns null when cancelled).
    if (segs[1] === 'pick-directory' && m === 'POST')
      return { command: 'pick_directory', args: {} };

    // POST /skills/open-library — open a skill's library folder in the OS
    // file manager (the managed copy under $APPDATA/skills).
    if (segs[1] === 'open-library' && m === 'POST') {
      const b = body as { id?: string } | null;
      return { command: 'open_skill_library_dir', args: { id: b?.id ?? '' } };
    }

    // POST /skills/delete — delete a skill from the library, optionally also
    // removing exported copies/symlinks at the given agent paths. Symlink
    // exports are always removed (mandatory); copy exports are optional
    // (caller passes the chosen agentIds in cleanupAgentIds). (Phase 2.5: real)
    if (segs[1] === 'delete' && m === 'POST') {
      const b = body as { id?: string; cleanupAgentIds?: string[] } | null;
      return { command: 'delete_skill', args: { id: b?.id ?? '', cleanupAgentIds: b?.cleanupAgentIds ?? [] } };
    }

    // POST /skills/uninstall — remove a single (skill, agent) install: deletes
    // the symlink/file copy at the agent's path and the skill_exports row.
    // (Phase 2.5: real)
    if (segs[1] === 'uninstall' && m === 'POST') {
      const b = body as { skillId?: string; agentId?: string } | null;
      return { command: 'uninstall_skill', args: { skillId: b?.skillId ?? '', agentId: b?.agentId ?? '' } };
    }

    // DELETE /skills/:id — plain library delete (kept for compatibility)
    if (m === 'DELETE' && segs.length === 2)
      return { command: '__stub__', args: { __response: { success: true } } };

    return { command: '__stub__', args: { __response: { success: false, message: 'Not found' } } };
  }

  // RAG endpoints — (rag_toggle, rag_status, list_rag_docs, get_rag_doc,
  // upload_rag_docs, delete_rag_doc, rag_search_command, get_rag_settings,
  // save_rag_settings, open_rag_file_location).
  if (segs[0] === 'rag') {
    // GET /rag/status — runtime status (switch state)
    if (segs[1] === 'status' && m === 'GET')
      return { command: 'rag_status', args: {} };
    // POST /rag/toggle — enable/disable RAG (blocks until ready on enable)
    if (segs[1] === 'toggle' && m === 'POST') {
      const b = body as { enabled?: boolean } | null;
      return { command: 'rag_toggle', args: { enabled: b?.enabled ?? false } };
    }
    // GET /rag/settings — search weights + max results
    if (segs[1] === 'settings' && m === 'GET')
      return { command: 'get_rag_settings', args: {} };
    // GET /rag/model-limits — model context window (tokens), to cap chunk_size
    if (segs[1] === 'model-limits' && m === 'GET')
      return { command: 'rag_model_limits', args: {} };
    // GET /rag/tools — app-level RAG tool definitions (for the "view tools" dialog)
    if (segs[1] === 'tools' && m === 'GET')
      return { command: 'rag_tools', args: {} };
    // PUT /rag/settings — persist search settings
    if (segs[1] === 'settings' && m === 'PUT')
      return { command: 'save_rag_settings', args: { settings: body } };
    // POST /rag/search — similarity search (optional tag filter)
    if (segs[1] === 'search' && m === 'POST') {
      const b = body as { query?: string; tags?: string[] } | null;
      return {
        command: 'rag_search_command',
        args: { query: b?.query ?? '', tags: b?.tags ?? [] },
      };
    }
    // POST /rag/tags/search — list/search distinct tags (optional search_key)
    if (segs[1] === 'tags' && segs[2] === 'search' && m === 'POST') {
      const b = body as { searchKey?: string[] } | null;
      return { command: 'rag_tag_search', args: { searchKey: b?.searchKey ?? [] } };
    }
    // POST /rag/docs/set-tags — set a document's tag list (re-indexes)
    if (segs[1] === 'docs' && segs[2] === 'set-tags' && m === 'POST') {
      const b = body as { id?: string; tags?: string[] } | null;
      return { command: 'set_rag_tags', args: { id: b?.id ?? '', tags: b?.tags ?? [] } };
    }
    // POST /rag/open-location — reveal a doc's file in the OS file manager
    if (segs[1] === 'open-location' && m === 'POST') {
      const b = body as { id?: string } | null;
      return { command: 'open_rag_file_location', args: { id: b?.id ?? '' } };
    }
    // POST /rag/reindex-all — re-embed every doc with the currently-loaded
    // model (after a model swap recreated the vector table). Emits progress
    // events; returns the count of docs re-embedded.
    if (segs[1] === 'reindex-all' && m === 'POST') {
      return { command: 'rag_reindex_all', args: {} };
    }
    // GET /rag/models - list available model sizes (ready / downloadable).
    if (segs[1] === 'models' && m === 'GET')
      return { command: 'rag_list_models', args: {} };
    // GET /rag/model - the currently-selected model size (or null).
    if (segs[1] === 'model' && m === 'GET')
      return { command: 'rag_current_model', args: {} };
    // POST /rag/select-model - persist + auto-restart RAG with the new model.
    if (segs[1] === 'select-model' && m === 'POST') {
      const b = body as { size?: string } | null;
      return { command: 'rag_select_model', args: { size: b?.size ?? '' } };
    }
    // POST /rag/download-model - stream-download a model .zip + extract.
    if (segs[1] === 'download-model' && m === 'POST') {
      const b = body as { size?: string } | null;
      return { command: 'rag_download_model', args: { size: b?.size ?? '' } };
    }
    // POST /rag/docs/pick — OS multi-file picker (plain-text), returns paths
    if (segs[1] === 'docs' && segs[2] === 'pick' && m === 'POST')
      return { command: 'pick_rag_files', args: {} };
    // POST /rag/docs/upload — upload a single file by disk path (backend reads
    // bytes from disk + detects encoding; no base64/JSON byte transfer).
    if (segs[1] === 'docs' && segs[2] === 'upload' && m === 'POST') {
      const b = body as { filePath?: string; tags?: string[] } | null;
      return {
        command: 'upload_rag_doc',
        args: { filePath: b?.filePath ?? '', tags: b?.tags ?? [] },
      };
    }
    // POST /rag/docs/delete - delete a doc + its vector records
    if (segs[1] === 'docs' && segs[2] === 'delete' && m === 'POST') {
      const b = body as { id?: string } | null;
      return { command: 'delete_rag_doc', args: { id: b?.id ?? '' } };
    }
    // POST /rag/docs/update - replace a doc's content + meta + vectors by id
    // (pick a new file; id preserved; tags preserved; content re-embedded).
    if (segs[1] === 'docs' && segs[2] === 'update' && m === 'POST') {
      const b = body as { id?: string; filePath?: string } | null;
      return { command: 'update_rag_doc', args: { id: b?.id ?? '', filePath: b?.filePath ?? '' } };
    }
    // GET /rag/docs/:id — full document (with content)
    if (segs[1] === 'docs' && m === 'GET' && segs.length === 3)
      return { command: 'get_rag_doc', args: { id: decodeURIComponent(segs[2]) } };
    // POST /rag/docs/chunks - a document's chunks (index + text) for the
    // "view chunks" dialog (RAG must be enabled; chunks live in lancedb).
    if (segs[1] === 'docs' && segs[2] === 'chunks' && m === 'POST') {
      const b = body as { id?: string } | null;
      return { command: 'get_rag_chunks', args: { id: b?.id ?? '' } };
    }
    // GET /rag/docs — list documents (metadata only)
    if (segs[1] === 'docs' && m === 'GET' && segs.length === 2)
      return { command: 'list_rag_docs', args: {} };

    return { command: '__stub__', args: { __response: { success: false, message: 'Not found' } } };
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
  // get_public_config returns { skipAuth, permissions } — wrap in success envelope
  if (command === 'get_public_config') {
    return { success: true, data: result };
  }
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
      status: st?.starting ? 'connecting' : st?.connected ? 'connected' : 'disconnected',
      error: st?.error ?? null,
      version: (st?.serverVersion as string | undefined) ?? undefined,
      tools: si.tools ?? [],
      prompts: si.prompts ?? [],
      resources: si.resources ?? [],
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
    const r = result as Record<string, unknown> | null;
    if (!r) return { success: true, data: { totalCalls: 0, successCount: 0, errorCount: 0, avgDuration: 0 } };
    return {
      success: true,
      data: {
        totalCalls: (r.total as number) ?? 0,
        successCount: (r.success as number) ?? 0,
        errorCount: (r.error as number) ?? 0,
        avgDuration: Math.round((r.avgDuration as number) ?? 0),
      },
    };
  }
  if (command === 'cleanup_activity_logs') {
    const r = result as Record<string, unknown> | null;
    return {
      success: true,
      data: {
        deletedCount: (r?.deletedCount as number) ?? 0,
        cutoffDate: (r?.cutoffDate as string) ?? '',
      },
    };
  }
  if (command === 'get_tool_activities') {
    const r = result as { data: unknown[]; page: number; pageSize: number; total: number } | null;
    if (!r) return { success: true, data: [], pagination: { page: 1, limit: 20, total: 0, totalPages: 1, hasNextPage: false, hasPrevPage: false } };
    const totalPages = Math.max(1, Math.ceil(r.total / (r.pageSize || 20)));
    // Transform backend camelCase fields to frontend expected format
    const activities = (r.data || []).map((e: Record<string, unknown>) => ({
      id: e.id,
      createdAt: e.createdAt,
      server: e.server,
      tool: e.tool,
      duration: (e.durationMs as number) ?? 0,  // durationMs → duration
      status: e.status,
      input: typeof e.input === 'string' ? e.input : JSON.stringify(e.input),
      output: typeof e.output === 'string' ? e.output : JSON.stringify(e.output),
      group: e.groupName,        // groupName → group
      username: e.username,
      keyId: e.keyId,
      keyName: e.keyName,
      sourceIp: e.sourceIp,
      errorMessage: e.errorMessage,
    }));
    return {
      success: true,
      data: activities,
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
    const r = result as Record<string, unknown> | null;
    return { success: true, data: { deletedCount: (r?.deletedCount as number) ?? 0 } };
  }

  // ── call_tool: frontend reads response.content directly (not response.data.content)
  if (command === 'call_tool') {
    const r = result as { content?: unknown[]; isError?: boolean } | null;
    if (r?.isError) {
      // Extract detailed error message from content
      const contentArr = r.content ?? [];
      const errorTexts: string[] = [];
      for (const item of contentArr) {
        if (item && typeof item === 'object') {
          const c = item as Record<string, unknown>;
          if (c.type === 'text' && typeof c.text === 'string') {
            errorTexts.push(c.text);
          } else if (typeof c.text === 'string') {
            errorTexts.push(c.text);
          }
        } else if (typeof item === 'string') {
          errorTexts.push(item);
        }
      }
      const detail = errorTexts.length > 0 ? errorTexts.join('\n') : 'Unknown error';
      return { success: false, content: contentArr, message: detail };
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
      // For __group_update_servers__, preserve the full config format
      nextNames = namesOf((args.servers as unknown[]) ?? []);
    }
    const payload = {
      name: group.name,
      description: group.description,
      servers: command === '__group_update_servers__' ? (args.servers as unknown[] ?? []) : nextNames,
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

