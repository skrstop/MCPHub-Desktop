-- Add per_session_client column to servers table (idempotent)
-- Mirrors origin d74d1be (#985): per-session upstream client isolation flag.
-- INTEGER 0/1, default 0 (shared pool — original behavior). When 1, each
-- downstream HTTP MCP session gets its own dedicated upstream client.
ALTER TABLE servers ADD COLUMN per_session_client INTEGER NOT NULL DEFAULT 0;
