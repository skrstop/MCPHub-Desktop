-- v13: Skills (技能) library + export tracking.
-- NOTE: This .sql file is a placeholder for the legacy `sqlx::migrate!` system.
-- The authoritative migration is `migrate_v13` in src-tauri/src/db/migration.rs,
-- which is what actually runs at startup. Kept here per repo convention.

CREATE TABLE IF NOT EXISTS skills (
    id           TEXT PRIMARY KEY,
    dir_name     TEXT NOT NULL UNIQUE,
    name         TEXT,
    description  TEXT,
    source_agent TEXT,
    source_path  TEXT,
    status       TEXT NOT NULL DEFAULT 'pending',  -- 'pending' | 'ok'
    created_at   TEXT NOT NULL DEFAULT (datetime('now','localtime'))
);

CREATE TABLE IF NOT EXISTS skill_exports (
    id         TEXT PRIMARY KEY,
    skill_id   TEXT NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
    agent_id   TEXT NOT NULL,
    method     TEXT NOT NULL,                       -- 'symlink' | 'copy'
    status     TEXT NOT NULL DEFAULT 'pending',    -- 'pending' | 'ok'
    created_at TEXT NOT NULL DEFAULT (datetime('now','localtime')),
    UNIQUE(skill_id, agent_id)
);
