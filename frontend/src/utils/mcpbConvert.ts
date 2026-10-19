/**
 * MCPB manifest → MCPHub stdio ServerConfig conversion.
 *
 * Extracted from McpbUploadForm.tsx as a pure function so the per-type
 * mapping (node / python / binary / uv) can be unit-tested (vitest).
 * Behavior mirrors origin's convertMcpbToMcpConfig, plus the desktop-only
 * uv synthesis: `type: "uv"` bundles may omit `mcp_config` (the host manages
 * execution per the MCPB spec) — desktop bundles uv, so we synthesize
 * `uv run --directory <extractDir> <extractDir>/<entry_point>`. The Rust
 * upload gate (mcpb_service::validate_server_section) guarantees that a uv
 * manifest without mcp_config.command carries a non-empty entry_point.
 */
export const convertMcpbToMcpConfig = (
  manifest: any,
  extractPath: string,
  _serverName: string,
) => {
  const mcpConfig = manifest.server?.mcp_config || {};

  // uv bundles may omit mcp_config (the host manages execution per the MCPB
  // spec). Desktop bundles uv, so synthesize `uv run --directory <dir>` —
  // the Rust upload gate guarantees entry_point exists in this case.
  if (manifest.server?.type === 'uv' && !mcpConfig.command) {
    return {
      type: 'stdio',
      command: 'uv',
      args: ['run', '--directory', extractPath, `${extractPath}/${manifest.server.entry_point}`],
    };
  }

  // Convert MCPB manifest to MCPHub stdio configuration
  const config: any = {
    type: 'stdio',
    command: mcpConfig.command || 'node',
    args: (mcpConfig.args || []).map((arg: string) => arg.replace('${__dirname}', extractPath)),
  };

  // Add environment variables if they exist
  if (mcpConfig.env && Object.keys(mcpConfig.env).length > 0) {
    config.env = { ...mcpConfig.env };

    // Replace ${__dirname} in environment variables
    Object.keys(config.env).forEach((key) => {
      if (typeof config.env[key] === 'string') {
        config.env[key] = config.env[key].replace('${__dirname}', extractPath);
      }
    });
  }

  return config;
};
