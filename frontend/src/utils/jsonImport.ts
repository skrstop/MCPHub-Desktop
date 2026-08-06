import type { ServerConfig } from '../types';

export interface ImportJsonFormat {
  mcpServers: Record<string, ServerConfig>;
}

export interface NormalizedServer {
  name: string;
  config: Partial<ServerConfig>;
}

export interface ImportIssue {
  name: string;
  message: string;
}

export interface NormalizeResult {
  servers: NormalizedServer[];
  issues: ImportIssue[];
}

// Keys that mcphub understands on a server config. Anything else is either a
// typo or copied from another tool's schema (e.g. an `auth` block), and would
// otherwise be silently dropped during import - leaving the user confused when
// the imported server does not behave as their JSON suggested.
const KNOWN_KEYS = new Set<keyof ServerConfig | string>([
  'type',
  'description',
  'url',
  'command',
  'args',
  'env',
  'headers',
  'passthroughHeaders',
  'enabled',
  'visibility',
  'enableKeepAlive',
  'keepAliveInterval',
  'perSessionClient',
  'startOnDemand',
  'idleTimeoutMs',
  'tools',
  'prompts',
  'options',
  'proxy',
  'oauth',
  'openapi',
]);

/**
 * Parse server type from string, handling various formats
 */
function parseServerType(typeStr: string | undefined): string {
  if (!typeStr) return 'stdio';

  const normalized = typeStr
    .trim()
    .toLowerCase()
    .replace(/_/g, '-')
    .replace(/\s+/g, '-');

  // Direct matches
  if (normalized === 'sse') return 'sse';
  if (normalized === 'streamable-http' || normalized === 'streamablehttp' || normalized === 'streamable') {
    return 'streamable-http';
  }
  if (normalized === 'openapi' || normalized === 'open-api') return 'openapi';
  if (normalized === 'stdio') return 'stdio';

  // Pattern-based detection
  if (normalized.includes('sse')) return 'sse';
  if (normalized.includes('http') || normalized.includes('stream')) return 'streamable-http';
  if (normalized.includes('openapi') || normalized.includes('open-api')) return 'openapi';

  return 'stdio';
}

/**
 * Auto-detect server type based on config properties
 */
function autoDetectType(config: Partial<ServerConfig>): string {
  // If type is explicitly set and valid, use it
  if (config.type && config.type !== 'stdio') {
    return parseServerType(config.type);
  }

  // Auto-detect from URL presence
  if (config.url && !config.command) {
    // Has URL but no command - likely SSE or HTTP
    return 'sse';
  }

  // Auto-detect from openapi config
  if (config.openapi) {
    return 'openapi';
  }

  return 'stdio';
}

/**
 * Normalize imported server configs and collect human-readable issues.
 *
 * Keeps the desktop's lenient type detection (fuzzy `type` matching + auto-detect
 * from url/openapi, see commit 74e3f17) while surfacing problems the old code
 * silently dropped: unknown top-level keys (e.g. an `auth` block), remote servers
 * missing a `url`, and stdio servers missing a `command`. Also carries `oauth`
 * through for remote servers so it is no longer lost on import.
 */
export const normalizeImportedServers = (parsed: ImportJsonFormat): NormalizeResult => {
  const servers: NormalizedServer[] = [];
  const issues: ImportIssue[] = [];

  for (const [name, rawConfig] of Object.entries(parsed.mcpServers)) {
    const config = (rawConfig ?? {}) as ServerConfig & Record<string, unknown>;
    const normalizedConfig: Partial<ServerConfig> = {};

    // Surface unknown top-level keys (e.g. the `auth` block some tools use).
    const unknownKeys = Object.keys(config).filter((key) => !KNOWN_KEYS.has(key));
    if (unknownKeys.length > 0) {
      issues.push({
        name,
        message: `unknown field(s) "${unknownKeys.join('", "')}" - not part of the mcphub server schema. For OAuth, use an "oauth" object (e.g. {"scopes":["email"]}).`,
      });
      continue;
    }

    // Detect the server type using multiple strategies (desktop: lenient detection).
    const detectedType = autoDetectType(config);
    normalizedConfig.type = parseServerType(detectedType);

    if (normalizedConfig.type === 'sse' || normalizedConfig.type === 'streamable-http') {
      normalizedConfig.url = config.url;
      if (config.headers) {
        normalizedConfig.headers = config.headers;
      }
      if (config.oauth) {
        normalizedConfig.oauth = config.oauth;
      }
      if (!config.url) {
        issues.push({
          name,
          message: `"${normalizedConfig.type}" servers require a "url" field.`,
        });
        continue;
      }
    } else if (normalizedConfig.type === 'openapi') {
      normalizedConfig.openapi = config.openapi;
    } else {
      normalizedConfig.type = 'stdio';
      normalizedConfig.command = config.command;
      normalizedConfig.args = config.args || [];
      if (config.env) {
        normalizedConfig.env = config.env;
      }
      if (config.options) {
        normalizedConfig.options = config.options;
      }
      if (!config.command) {
        issues.push({
          name,
          message: `stdio servers require a "command" field.`,
        });
        continue;
      }
    }

    servers.push({ name, config: normalizedConfig });
  }

  return { servers, issues };
};
