-- v27: UNIQUE indexes on builtin prompt/resource NAMEs.
-- The service-layer re-name rejection is COUNT-then-INSERT with no UNIQUE
-- constraint — concurrent creates could insert duplicates. Dedup (keep
-- lowest rowid), then index. Deliberately NO uri index: v23 dropped it as an
-- explicit product decision (resource lookup is by name).
DELETE FROM builtin_prompts WHERE rowid NOT IN (
    SELECT MIN(rowid) FROM builtin_prompts GROUP BY name);
DELETE FROM builtin_resources WHERE rowid NOT IN (
    SELECT MIN(rowid) FROM builtin_resources GROUP BY name);
CREATE UNIQUE INDEX IF NOT EXISTS idx_builtin_prompts_name ON builtin_prompts(name);
CREATE UNIQUE INDEX IF NOT EXISTS idx_builtin_resources_name ON builtin_resources(name);
-- NOTE: builtin_prompts.name / builtin_resources.name are treated as
-- globally unique (existing invariant: FTS ref_id == name).
