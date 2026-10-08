-- v29: Repair bearer_keys schema for fresh installs (token-based model).
-- Applied procedurally by db/migration.rs (migrate_v29) with an
-- idempotency guard; this file documents the shape for reference.
DROP TABLE IF EXISTS bearer_keys;
CREATE TABLE bearer_keys (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    token           TEXT NOT NULL,
    enabled         INTEGER NOT NULL DEFAULT 1,
    access_type     TEXT NOT NULL DEFAULT '',
    allowed_groups  TEXT NOT NULL DEFAULT '[]',
    allowed_servers TEXT NOT NULL DEFAULT '[]',
    created_at      TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
);
