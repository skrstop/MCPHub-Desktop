-- v30: OAuth upstream support.
-- `servers.oauth` stores the ServerOauthConfig JSON (client credentials,
-- endpoints, scopes, static tokens); previously collected by the UI but
-- dropped by the Rust serde model (R10 debt).
-- `oauth_upstream_tokens` holds the runtime tokens obtained via the
-- authorization-code flow, one row per server, so token refreshes never
-- rewrite the server config row.
-- v30: OAuth upstream support.
-- `servers.oauth` stores the ServerOauthConfig JSON (client credentials,
-- endpoints, scopes, static tokens); previously collected by the UI but
-- dropped by the Rust serde model (R10 debt).
-- `oauth_upstream_tokens` holds the rmcp AuthorizationManager state, one row
-- per server. StoredCredentials / StoredAuthorizationState are JSON-serializable
-- SDK structs (rmcp 3.4.1 `auth` feature), so they persist as opaque blobs in
-- credentials_json / pending_json; refreshes never rewrite the server row.
ALTER TABLE servers ADD COLUMN oauth TEXT;
CREATE TABLE IF NOT EXISTS oauth_upstream_tokens (
    server_name        TEXT PRIMARY KEY,
    credentials_json   TEXT NOT NULL DEFAULT '',
    pending_json       TEXT NOT NULL DEFAULT '',
    status             TEXT NOT NULL DEFAULT 'disabled',
    error_message      TEXT NOT NULL DEFAULT '',
    updated_at         INTEGER NOT NULL DEFAULT 0
);
