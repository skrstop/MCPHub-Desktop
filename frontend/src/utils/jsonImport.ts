import type { ServerConfig } from '../types';

export interface ImportJsonFormat {
  mcpServers: Record<string, ServerConfig>;
}

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

export const normalizeImportedServers = (parsed: ImportJsonFormat) => {
  return Object.entries(parsed.mcpServers).map(([name, config]) => {
    const normalizedConfig: Partial<ServerConfig> = {};

    // Detect the server type using multiple strategies
    const detectedType = autoDetectType(config);
    normalizedConfig.type = parseServerType(detectedType);

    if (normalizedConfig.type === 'sse' || normalizedConfig.type === 'streamable-http') {
      normalizedConfig.url = config.url;
      if (config.headers) {
        normalizedConfig.headers = config.headers;
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
    }

    return { name, config: normalizedConfig };
  });
};
