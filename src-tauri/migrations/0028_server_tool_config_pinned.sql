-- v28: per-item pin flag on server_tool_config.
-- Server-level pinned tools feed the $smart endpoints (root $smart lists them
-- across all servers; $smart/{group} unions them with per-member pins).
ALTER TABLE server_tool_config ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
