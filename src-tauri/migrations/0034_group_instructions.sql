-- v33 → v34: group-level instructions (#1340 parity).
-- Optional text sent to MCP clients on initialize when they connect through
-- the group's route (/mcp/{group} or $smart/{group}).
ALTER TABLE groups ADD COLUMN instructions TEXT;
