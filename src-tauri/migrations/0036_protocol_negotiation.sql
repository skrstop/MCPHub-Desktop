-- v36 (#1382 parity): per-server upstream protocol negotiation mode.
-- 'auto' = probe server/discover (2026 modern) before legacy initialize
-- (streamable-http only); 'legacy' = skip the modern probe. NULL = auto for
-- streamable-http, legacy for every other transport.
ALTER TABLE servers ADD COLUMN protocol_negotiation TEXT;
