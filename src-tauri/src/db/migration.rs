/// Database schema version management.
///
/// Tracks the current schema version in a dedicated `schema_version` table
/// and applies pending migrations sequentially at startup.
///
/// Each migration is an async function that takes a `&SqlitePool` and
/// performs the DDL/DML needed to upgrade from version N to N+1.
use anyhow::{anyhow, Result};
use sqlx::{Row, SqlitePool};

/// Current target schema version — bump this when adding new migrations.
pub const TARGET_VERSION: i64 = 9;

/// Initialize the schema_version table (create if not exists, read current version).
/// Handles migration from old `sqlx::migrate!` system (which used `_sqlx_migrations` table).
async fn get_current_version(pool: &SqlitePool) -> Result<i64> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS schema_version (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            version INTEGER NOT NULL DEFAULT 0,
            updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
        )",
    )
    .execute(pool)
    .await?;

    let version: i64 = sqlx::query_scalar("SELECT version FROM schema_version WHERE id = 1")
        .fetch_optional(pool)
        .await?
        .unwrap_or(0);

    if version > 0 {
        return Ok(version);
    }

    // Check if old sqlx::migrate! system was used — detect by _sqlx_migrations table
    let has_old_migrations: bool = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations'",
    )
    .fetch_one(pool)
    .await
    .map(|n| n > 0)
    .unwrap_or(false);

    if has_old_migrations {
        // Count how many old migrations were applied
        let old_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(pool)
            .await
            .unwrap_or(0);
        log::info!(
            "[db] migrating from sqlx::migrate! system ({} old migrations found)",
            old_count
        );
        // Map old migration count to new schema version
        // Old migrations: 0001_initial, 0002_schema_fix, 0003_config_json, 0004_default_admin, 0005_default_skip_auth
        // New system: v1=initial, v2=schema_fix, v3=config_json, v4=default_admin, v5=skip_auth
        let new_version = std::cmp::min(old_count, TARGET_VERSION);
        if new_version > 0 {
            set_version(pool, new_version).await?;
            log::info!("[db] initialized schema_version to v{} (from old system)", new_version);
            return Ok(new_version);
        }
    }

    Ok(0)
}

/// Update the schema version number.
async fn set_version(pool: &SqlitePool, version: i64) -> Result<()> {
    sqlx::query(
        "INSERT INTO schema_version (id, version, updated_at)
         VALUES (1, ?, datetime('now', 'localtime'))
         ON CONFLICT(id) DO UPDATE SET version = excluded.version, updated_at = excluded.updated_at",
    )
    .bind(version)
    .execute(pool)
    .await?;
    Ok(())
}

/// Run all pending migrations to bring the database to `TARGET_VERSION`.
pub async fn run_pending(pool: &SqlitePool) -> Result<()> {
    let current = get_current_version(pool).await?;
    log::info!("[db] schema version: current={}, target={}", current, TARGET_VERSION);

    if current >= TARGET_VERSION {
        log::info!("[db] schema is up to date");
        return Ok(());
    }

    // Apply each migration in order
    for version in (current + 1)..=TARGET_VERSION {
        log::info!("[db] applying migration v{} → v{}...", version - 1, version);
        apply_migration(pool, version).await?;
        set_version(pool, version).await?;
        log::info!("[db] migration v{} applied successfully", version);
    }

    log::info!("[db] all migrations applied, schema is now at v{}", TARGET_VERSION);
    Ok(())
}

/// Apply a single migration by version number.
async fn apply_migration(pool: &SqlitePool, version: i64) -> Result<()> {
    match version {
        1 => migrate_v1(pool).await,
        2 => migrate_v2(pool).await,
        3 => migrate_v3(pool).await,
        4 => migrate_v4(pool).await,
        5 => migrate_v5(pool).await,
        6 => migrate_v6(pool).await,
        7 => migrate_v7(pool).await,
        8 => migrate_v8(pool).await,
        9 => migrate_v9(pool).await,
        _ => Err(anyhow!("Unknown migration version: {}", version)),
    }
}

// ---------------------------------------------------------------------------
// Migration definitions
// ---------------------------------------------------------------------------

/// v0 → v1: Initial schema
async fn migrate_v1(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS users (
            id          TEXT PRIMARY KEY,
            username    TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            role        TEXT NOT NULL DEFAULT 'user',
            created_at  TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS servers (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL UNIQUE,
            server_type TEXT NOT NULL DEFAULT 'stdio',
            description TEXT,
            command     TEXT,
            args        TEXT,
            env         TEXT,
            url         TEXT,
            headers     TEXT,
            options     TEXT,
            enabled     INTEGER NOT NULL DEFAULT 1,
            created_at  TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS groups (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL UNIQUE,
            description TEXT,
            servers     TEXT NOT NULL DEFAULT '[]',
            created_at  TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS system_config (
            id          INTEGER PRIMARY KEY DEFAULT 1,
            proxy       TEXT,
            registry    TEXT,
            log_level   TEXT DEFAULT 'info',
            expose_http INTEGER DEFAULT 0,
            http_port   INTEGER DEFAULT 3000,
            updated_at  TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query("INSERT OR IGNORE INTO system_config (id) VALUES (1)")
        .execute(pool)
        .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS bearer_keys (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            key_hash    TEXT NOT NULL,
            user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            expires_at  TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS activity_log (
            id          TEXT PRIMARY KEY,
            user_id     TEXT,
            action      TEXT NOT NULL,
            resource    TEXT NOT NULL,
            detail      TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS app_log (
            id          TEXT PRIMARY KEY,
            level       TEXT NOT NULL,
            message     TEXT NOT NULL,
            server_name TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS builtin_prompts (
            id          TEXT PRIMARY KEY,
            server_name TEXT NOT NULL,
            name        TEXT NOT NULL,
            description TEXT,
            arguments   TEXT,
            enabled     INTEGER NOT NULL DEFAULT 1,
            created_at  TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS builtin_resources (
            id          TEXT PRIMARY KEY,
            server_name TEXT NOT NULL,
            uri         TEXT NOT NULL,
            name        TEXT NOT NULL,
            description TEXT,
            mime_type   TEXT,
            enabled     INTEGER NOT NULL DEFAULT 1,
            created_at  TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
        )",
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// v1 → v2: Schema fixes
async fn migrate_v2(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS activity_log (
            id          TEXT PRIMARY KEY,
            user_id     TEXT,
            action      TEXT NOT NULL,
            resource    TEXT NOT NULL,
            detail      TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query("ALTER TABLE system_config ADD COLUMN mcprouter_api_key TEXT")
        .execute(pool)
        .await
        .ok(); // ignore if column already exists

    sqlx::query("ALTER TABLE system_config ADD COLUMN mcprouter_base_url TEXT")
        .execute(pool)
        .await
        .ok();

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS templates (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL UNIQUE,
            description TEXT,
            content     TEXT NOT NULL,
            created_at  TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS server_tool_config (
            id          TEXT PRIMARY KEY,
            server_name TEXT NOT NULL,
            item_type   TEXT NOT NULL DEFAULT 'tool',
            item_name   TEXT NOT NULL,
            enabled     INTEGER NOT NULL DEFAULT 1,
            description TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
            UNIQUE(server_name, item_type, item_name)
        )",
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// v2 → v3: Config JSON consolidation
async fn migrate_v3(pool: &SqlitePool) -> Result<()> {
    // Add config_json column to system_config if not exists
    sqlx::query("ALTER TABLE system_config ADD COLUMN config_json TEXT")
        .execute(pool)
        .await
        .ok();

    // Migrate existing individual columns into config_json
    let row = sqlx::query("SELECT * FROM system_config WHERE id = 1")
        .fetch_optional(pool)
        .await?;

    if let Some(row) = row {
        let mut config = serde_json::Map::new();

        if let Ok(Some(v)) = row.try_get::<Option<String>, _>("proxy") {
            config.insert("proxy".to_string(), serde_json::Value::String(v));
        }
        if let Ok(Some(v)) = row.try_get::<Option<String>, _>("registry") {
            config.insert("registry".to_string(), serde_json::Value::String(v));
        }
        if let Ok(Some(v)) = row.try_get::<Option<String>, _>("log_level") {
            config.insert("logLevel".to_string(), serde_json::Value::String(v));
        }
        if let Ok(v) = row.try_get::<i64, _>("expose_http") {
            config.insert("exposeHttp".to_string(), serde_json::Value::Bool(v != 0));
        }
        if let Ok(v) = row.try_get::<i64, _>("http_port") {
            config.insert("httpPort".to_string(), serde_json::Value::Number(v.into()));
        }
        if let Ok(Some(v)) = row.try_get::<Option<String>, _>("mcprouter_api_key") {
            config.insert("mcprouterApiKey".to_string(), serde_json::Value::String(v));
        }
        if let Ok(Some(v)) = row.try_get::<Option<String>, _>("mcprouter_base_url") {
            config.insert("mcprouterBaseUrl".to_string(), serde_json::Value::String(v));
        }

        if !config.is_empty() {
            let json = serde_json::to_string(&config)?;
            sqlx::query("UPDATE system_config SET config_json = ? WHERE id = 1")
                .bind(&json)
                .execute(pool)
                .await?;
        }
    }

    Ok(())
}

/// v3 → v4: Default admin user
async fn migrate_v4(pool: &SqlitePool) -> Result<()> {
    let admin_hash = "$2b$10$68DpNRgEB4V88lMXDK46J.ahxYKObFIUnuff5x2oxkhtaWt2dMUO6";
    sqlx::query(
        "INSERT OR IGNORE INTO users (id, username, password_hash, role, created_at, updated_at)
         SELECT 'admin-default', 'admin', ?, 'admin', datetime('now', 'localtime'), datetime('now', 'localtime')
         WHERE NOT EXISTS (SELECT 1 FROM users WHERE username = 'admin')",
    )
    .bind(admin_hash)
    .execute(pool)
    .await?;
    Ok(())
}

/// v4 → v5: Default skip_auth setting
async fn migrate_v5(pool: &SqlitePool) -> Result<()> {
    sqlx::query("ALTER TABLE system_config ADD COLUMN skip_auth INTEGER DEFAULT 0")
        .execute(pool)
        .await
        .ok();
    Ok(())
}

/// v5 → v6: Add openapi column to servers table
async fn migrate_v6(pool: &SqlitePool) -> Result<()> {
    sqlx::query("ALTER TABLE servers ADD COLUMN openapi TEXT")
        .execute(pool)
        .await
        .ok(); // ignore if column already exists
    Ok(())
}

/// v6 → v7: Add source_ip column to activity_log
async fn migrate_v7(pool: &SqlitePool) -> Result<()> {
    sqlx::query("ALTER TABLE activity_log ADD COLUMN source_ip TEXT")
        .execute(pool)
        .await
        .ok(); // ignore if column already exists
    Ok(())
}

/// v7 → v8: Fix timezone — convert all UTC timestamps to local time
async fn migrate_v8(pool: &SqlitePool) -> Result<()> {
    // Update app_log: shift created_at from UTC to local time
    sqlx::query(
        "UPDATE app_log SET created_at = datetime(created_at, 'localtime') WHERE created_at IS NOT NULL"
    )
    .execute(pool)
    .await
    .ok();

    // Update other tables with created_at/updated_at columns
    for table in &["users", "servers", "groups", "bearer_keys", "templates", "server_tool_config", "builtin_prompts", "builtin_resources"] {
        let sql = format!(
            "UPDATE {} SET created_at = datetime(created_at, 'localtime') WHERE created_at IS NOT NULL",
            table
        );
        sqlx::query(&sql).execute(pool).await.ok();
    }
    for table in &["users", "servers", "templates", "server_tool_config"] {
        let sql = format!(
            "UPDATE {} SET updated_at = datetime(updated_at, 'localtime') WHERE updated_at IS NOT NULL",
            table
        );
        sqlx::query(&sql).execute(pool).await.ok();
    }

    log::info!("[db] migration v8: converted existing timestamps to local time");
    Ok(())
}

/// v8 → v9: Recreate activity_log with correct schema.
///
/// The old activity_log table (created in v1/v2) had columns:
///   id, user_id, action, resource, detail, created_at
///
/// The code expects columns:
///   id, created_at, server, tool, duration_ms, status,
///   input, output, error_message, group_name, key_id, key_name, source_ip
///
/// Since the schemas are incompatible, we drop and recreate the table.
async fn migrate_v9(pool: &SqlitePool) -> Result<()> {
    // Drop the old table with wrong schema
    sqlx::query("DROP TABLE IF EXISTS activity_log")
        .execute(pool)
        .await?;

    // Create with the correct schema matching log_service.rs
    sqlx::query(
        "CREATE TABLE activity_log (
            id            TEXT PRIMARY KEY,
            created_at    TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
            server        TEXT NOT NULL DEFAULT '',
            tool          TEXT NOT NULL DEFAULT '',
            duration_ms   INTEGER,
            status        TEXT NOT NULL DEFAULT '',
            input         TEXT,
            output        TEXT,
            error_message TEXT,
            group_name    TEXT,
            key_id        TEXT,
            key_name      TEXT,
            source_ip     TEXT
        )",
    )
    .execute(pool)
    .await?;

    log::info!("[db] migration v9: recreated activity_log with correct schema");
    Ok(())
}
