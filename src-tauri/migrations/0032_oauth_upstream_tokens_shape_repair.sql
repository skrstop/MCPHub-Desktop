-- v31 -> v32: repair legacy-shaped oauth_upstream_tokens tables (dev DBs that
-- predate v30 hold issuer/client_id columns; rmcp writes credentials_json).
-- Guarded by pragma checks in migration.rs; this file documents the shape.
CREATE TABLE IF NOT EXISTS oauth_upstream_tokens (
    server_name        TEXT PRIMARY KEY,
    credentials_json   TEXT NOT NULL DEFAULT '',
    pending_json       TEXT NOT NULL DEFAULT '',
    status             TEXT NOT NULL DEFAULT 'disabled',
    error_message      TEXT NOT NULL DEFAULT '',
    updated_at         INTEGER NOT NULL DEFAULT 0
);
