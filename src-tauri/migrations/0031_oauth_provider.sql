-- v30 → v31: OAuth Provider (MCPHub as authorization server).
-- Historical snapshot: the runtime source of truth is src/db/migration.rs
-- (migrate_v31); this file exists for reference parity with migrations 0001-0030.

CREATE TABLE IF NOT EXISTS oauth_clients (
    client_id          TEXT PRIMARY KEY,
    client_secret_hash TEXT,
    name               TEXT NOT NULL DEFAULT '',
    redirect_uris      TEXT NOT NULL DEFAULT '[]',
    scopes             TEXT NOT NULL DEFAULT '',
    created_at         INTEGER NOT NULL DEFAULT 0,
    revoked            INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS oauth_tokens (
    token_hash          TEXT PRIMARY KEY,
    kind                TEXT NOT NULL DEFAULT 'access',
    client_id           TEXT NOT NULL,
    scope               TEXT NOT NULL DEFAULT '',
    resource            TEXT NOT NULL DEFAULT '',
    expires_at          INTEGER NOT NULL DEFAULT 0,
    refresh_expires_at  INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS oauth_codes (
    code_hash        TEXT PRIMARY KEY,
    client_id        TEXT NOT NULL,
    challenge        TEXT NOT NULL DEFAULT '',
    challenge_method TEXT NOT NULL DEFAULT 'S256',
    scope            TEXT NOT NULL DEFAULT '',
    resource         TEXT NOT NULL DEFAULT '',
    redirect_uri     TEXT NOT NULL DEFAULT '',
    expires_at       INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_oauth_tokens_client ON oauth_tokens(client_id);
