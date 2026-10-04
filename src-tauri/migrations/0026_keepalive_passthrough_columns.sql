-- INERT FILE: the runtime path uses add_column_if_missing (idempotent) in
-- db/migration.rs; this file exists only for sqlx::migrate! compatibility.
-- Replaying it against an already-migrated DB would fail (no IF NOT EXISTS).
-- v26: sse/streamable-http keep-alive + passthrough header columns
-- (frontend round-trip persistence).
ALTER TABLE servers ADD COLUMN enable_keep_alive INTEGER;
ALTER TABLE servers ADD COLUMN keep_alive_interval INTEGER;
ALTER TABLE servers ADD COLUMN passthrough_headers TEXT;
