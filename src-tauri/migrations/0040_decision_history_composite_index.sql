-- v39 → v40: composite index for the decision-history ordering key.
-- The v39 single-column index cannot satisfy `ORDER BY created_at DESC,
-- id DESC` (the id tiebreak forces a temp B-tree sort over the whole table
-- on every page fetch and row-cap prune). The composite index turns both
-- queries into index-range walks.
CREATE INDEX IF NOT EXISTS idx_decision_history_created_id
    ON decision_history(created_at DESC, id DESC);
