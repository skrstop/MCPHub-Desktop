-- MCPHub initial schema

-- Users table
CREATE TABLE IF NOT EXISTS users (
    id          TEXT PRIMARY KEY,
    username    TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    role        TEXT NOT NULL DEFAULT 'user',
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- MCP Servers table
CREATE TABLE IF NOT EXISTS servers (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    server_type TEXT NOT NULL DEFAULT 'stdio',
    description TEXT,
    command     TEXT,
    args        TEXT,   -- JSON array
    env         TEXT,   -- JSON object
    url         TEXT,
    headers     TEXT,   -- JSON object
    options     TEXT,   -- JSON object
    enabled     INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Groups table
CREATE TABLE IF NOT EXISTS groups (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    description TEXT,
    servers     TEXT NOT NULL DEFAULT '[]', -- JSON array of server names
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- System configuration (single row)
CREATE TABLE IF NOT EXISTS system_config (
    id          INTEGER PRIMARY KEY DEFAULT 1,
    proxy       TEXT,
    registry    TEXT,
    log_level   TEXT DEFAULT 'info',
    expose_http INTEGER DEFAULT 0,
    http_port   INTEGER DEFAULT 3000,
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

INSERT OR IGNORE INTO system_config (id) VALUES (1);

-- Bearer API keys
CREATE TABLE IF NOT EXISTS bearer_keys (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    key_hash    TEXT NOT NULL,
    user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at  TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Activity log
CREATE TABLE IF NOT EXISTS activity_log (
    id          TEXT PRIMARY KEY,
    user_id     TEXT,
    action      TEXT NOT NULL,
    resource    TEXT NOT NULL,
    detail      TEXT,  -- JSON
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Application log
CREATE TABLE IF NOT EXISTS app_log (
    id          TEXT PRIMARY KEY,
    level       TEXT NOT NULL,
    message     TEXT NOT NULL,
    server_name TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Builtin prompts (per server)
CREATE TABLE IF NOT EXISTS builtin_prompts (
    id          TEXT PRIMARY KEY,
    server_name TEXT NOT NULL,
    name        TEXT NOT NULL,
    description TEXT,
    arguments   TEXT,  -- JSON
    enabled     INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Builtin resources (per server)
CREATE TABLE IF NOT EXISTS builtin_resources (
    id          TEXT PRIMARY KEY,
    server_name TEXT NOT NULL,
    uri         TEXT NOT NULL,
    name        TEXT NOT NULL,
    description TEXT,
    mime_type   TEXT,
    enabled     INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
