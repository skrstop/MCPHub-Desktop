-- Migration 0002: Fix schema issues and add missing tables
-- This migration corrects structural mismatches from 0001_initial.sql

-- 1. Rebuild bearer_keys table with correct structure (matching original project)
DROP TABLE IF EXISTS bearer_keys;
CREATE TABLE IF NOT EXISTS bearer_keys (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    token           TEXT NOT NULL UNIQUE,
    enabled         INTEGER NOT NULL DEFAULT 1,
    access_type     TEXT NOT NULL DEFAULT 'all',   -- all | groups | servers | custom
    allowed_groups  TEXT NOT NULL DEFAULT '[]',    -- JSON array of group names
    allowed_servers TEXT NOT NULL DEFAULT '[]',    -- JSON array of server names
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

-- 2. Fix builtin_prompts: add missing columns
ALTER TABLE builtin_prompts ADD COLUMN title TEXT;
ALTER TABLE builtin_prompts ADD COLUMN template TEXT NOT NULL DEFAULT '';

-- 3. Fix builtin_resources: add missing content column
ALTER TABLE builtin_resources ADD COLUMN content TEXT NOT NULL DEFAULT '';

-- 4. Rebuild activity_log for tool call monitoring (matches original ActivityLoggingService)
DROP TABLE IF EXISTS activity_log;
CREATE TABLE IF NOT EXISTS activity_log (
    id            TEXT PRIMARY KEY,
    timestamp     TEXT NOT NULL DEFAULT (datetime('now')),
    server        TEXT NOT NULL,
    tool          TEXT NOT NULL,
    duration_ms   INTEGER,
    status        TEXT NOT NULL DEFAULT 'success',  -- success | error
    input         TEXT,          -- JSON string of tool arguments
    output        TEXT,          -- JSON string of tool result
    group_name    TEXT,
    key_id        TEXT,
    key_name      TEXT,
    error_message TEXT
);

-- 5. Extend system_config with MCPRouter cloud service fields
ALTER TABLE system_config ADD COLUMN mcprouter_api_key TEXT;
ALTER TABLE system_config ADD COLUMN mcprouter_base_url TEXT;

-- 6. Add templates table for config template import/export
CREATE TABLE IF NOT EXISTS templates (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    description TEXT,
    content     TEXT NOT NULL,   -- JSON-serialized ConfigTemplate
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- 7. Add server_tool_config table for per-server tool/prompt/resource toggles
CREATE TABLE IF NOT EXISTS server_tool_config (
    id          TEXT PRIMARY KEY,
    server_name TEXT NOT NULL,
    item_type   TEXT NOT NULL DEFAULT 'tool',  -- tool | prompt | resource
    item_name   TEXT NOT NULL,
    enabled     INTEGER NOT NULL DEFAULT 1,
    description TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(server_name, item_type, item_name)
);
