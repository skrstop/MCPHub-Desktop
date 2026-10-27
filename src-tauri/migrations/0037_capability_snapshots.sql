-- v37 (#1384 parity): last-known tool snapshots for on-demand servers.
-- One row per server name; tools_json is a serialized Vec<Tool> captured when
-- the server last woke. Sleeping placeholders load it at startup so the server
-- still advertises its previous tools until the first cold start.
CREATE TABLE IF NOT EXISTS capability_snapshots (
    server_name TEXT PRIMARY KEY,
    tools_json  TEXT NOT NULL,
    fingerprint TEXT NOT NULL DEFAULT '',
    updated_at  TEXT NOT NULL DEFAULT ''
);
