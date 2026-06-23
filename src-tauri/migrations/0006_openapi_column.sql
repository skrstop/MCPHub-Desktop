-- Add openapi configuration column to servers table (idempotent)
-- SQLite does not support IF NOT EXISTS on ALTER TABLE, but sqlx::migrate
-- tracks applied migrations in _sqlx_migrations, so this only runs once.
-- The application code also handles the missing column gracefully at runtime.
ALTER TABLE servers ADD COLUMN openapi TEXT;
