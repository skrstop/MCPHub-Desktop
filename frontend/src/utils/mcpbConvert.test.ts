import { describe, it, expect } from 'vitest';
import { convertMcpbToMcpConfig } from './mcpbConvert';

const EXTRACT = '/data/uploads/mcpb/server-demo';

describe('convertMcpbToMcpConfig — per MCPB server type', () => {
  it('type=node: stdio + node command, ${__dirname} replaced in args', () => {
    const cfg = convertMcpbToMcpConfig(
      {
        server: {
          type: 'node',
          entry_point: 'server/index.js',
          mcp_config: { command: 'node', args: ['${__dirname}/server/index.js', '--verbose'] },
        },
      },
      EXTRACT,
      'demo',
    );
    expect(cfg).toEqual({
      type: 'stdio',
      command: 'node',
      args: [`${EXTRACT}/server/index.js`, '--verbose'],
    });
    expect(cfg.env).toBeUndefined();
  });

  it('type=python: stdio + python command, ${__dirname} replaced in args and env', () => {
    const cfg = convertMcpbToMcpConfig(
      {
        server: {
          type: 'python',
          entry_point: 'server/main.py',
          mcp_config: {
            command: 'python',
            args: ['${__dirname}/server/main.py'],
            env: { PYTHONPATH: '${__dirname}/server/lib', STATIC: 'keep-me' },
          },
        },
      },
      EXTRACT,
      'demo',
    );
    expect(cfg).toEqual({
      type: 'stdio',
      command: 'python',
      args: [`${EXTRACT}/server/main.py`],
      env: { PYTHONPATH: `${EXTRACT}/server/lib`, STATIC: 'keep-me' },
    });
  });

  it('type=binary: stdio + direct executable path from mcp_config', () => {
    const cfg = convertMcpbToMcpConfig(
      {
        server: {
          type: 'binary',
          entry_point: 'server/my-tool',
          mcp_config: { command: 'server/my-tool', args: ['--config', 'server/config.json'] },
        },
      },
      EXTRACT,
      'demo',
    );
    expect(cfg).toEqual({
      type: 'stdio',
      command: 'server/my-tool',
      args: ['--config', 'server/config.json'],
    });
  });

  it('type=uv with explicit mcp_config: used as-is (no synthesis)', () => {
    const cfg = convertMcpbToMcpConfig(
      {
        server: {
          type: 'uv',
          entry_point: 'src/server.py',
          mcp_config: { command: 'uv', args: ['run', 'src/server.py'] },
        },
      },
      EXTRACT,
      'demo',
    );
    expect(cfg).toEqual({ type: 'stdio', command: 'uv', args: ['run', 'src/server.py'] });
  });

  it('type=uv without mcp_config: synthesizes `uv run --directory <dir> <dir>/<entry_point>`', () => {
    const cfg = convertMcpbToMcpConfig(
      { server: { type: 'uv', entry_point: 'src/server.py' } },
      EXTRACT,
      'demo',
    );
    expect(cfg).toEqual({
      type: 'stdio',
      command: 'uv',
      args: ['run', '--directory', EXTRACT, `${EXTRACT}/src/server.py`],
    });
  });

  it('type=uv with mcp_config lacking command but entry_point present: synthesizes', () => {
    const cfg = convertMcpbToMcpConfig(
      { server: { type: 'uv', entry_point: 'src/server.py', mcp_config: { env: {} } } },
      EXTRACT,
      'demo',
    );
    expect(cfg).toEqual({
      type: 'stdio',
      command: 'uv',
      args: ['run', '--directory', EXTRACT, `${EXTRACT}/src/server.py`],
    });
  });

  it('origin parity: missing mcp_config.command on non-uv type defaults to node (Rust gate rejects this upstream)', () => {
    const cfg = convertMcpbToMcpConfig(
      { server: { type: 'node', entry_point: 'server/index.js' } },
      EXTRACT,
      'demo',
    );
    expect(cfg).toEqual({ type: 'stdio', command: 'node', args: [] });
  });

  it('origin parity: missing server section entirely defaults to node', () => {
    const cfg = convertMcpbToMcpConfig({}, EXTRACT, 'demo');
    expect(cfg).toEqual({ type: 'stdio', command: 'node', args: [] });
  });

  it('env with non-string values: only string values get ${__dirname} replaced', () => {
    const cfg = convertMcpbToMcpConfig(
      {
        server: {
          mcp_config: {
            command: 'node',
            args: [],
            env: { PATH_LIKE: '${__dirname}/bin', RETRIES: 3, DEBUG: true, EMPTY: null },
          },
        },
      },
      EXTRACT,
      'demo',
    );
    expect(cfg.env).toEqual({
      PATH_LIKE: `${EXTRACT}/bin`,
      RETRIES: 3,
      DEBUG: true,
      EMPTY: null,
    });
  });

  it('env object is copied, not referenced (no mutation of the manifest)', () => {
    const manifest = {
      server: { mcp_config: { command: 'node', args: [], env: { A: '${__dirname}/x' } } },
    };
    const cfg = convertMcpbToMcpConfig(manifest, EXTRACT, 'demo');
    expect(cfg.env.A).toBe(`${EXTRACT}/x`);
    // Shallow copy — the source manifest object is never mutated.
    expect(manifest.server.mcp_config.env.A).toBe('${__dirname}/x');
  });

  it('args containing ${__dirname} multiple times: only first occurrence replaced (origin parity — String.replace semantics)', () => {
    const cfg = convertMcpbToMcpConfig(
      {
        server: {
          mcp_config: { command: 'node', args: ['${__dirname}/a ${__dirname}/b'] },
        },
      },
      EXTRACT,
      'demo',
    );
    expect(cfg.args).toEqual([`${EXTRACT}/a ` + '${__dirname}/b']);
  });
});
