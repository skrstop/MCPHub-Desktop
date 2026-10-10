-- v32 -> v33: mark dynamically registered clients (clientTtl reaper scoping).
ALTER TABLE oauth_clients ADD COLUMN dynamic INTEGER NOT NULL DEFAULT 0;
