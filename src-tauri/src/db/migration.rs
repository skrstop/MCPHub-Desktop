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
pub const TARGET_VERSION: i64 = 25;

/// Check whether any migration is pending (schema version below target).
/// Used by the pre-migration backup step in `db::initialize`.
pub async fn has_pending(pool: &SqlitePool) -> Result<bool> {
    let current = get_current_version(pool).await?;
    Ok(current < TARGET_VERSION)
}

/// Backup the database before applying pending migrations (requirement: keep a
/// pre-migration copy for manual rollback). Only backs up when migrations are
/// actually pending, so routine startups neither create nor overwrite the file.
///
/// Uses `VACUUM INTO` for a consistent snapshot (works under WAL without
/// checkpointing; takes a read transaction, never blocks writers beyond
/// busy_timeout). Note SQLite requires the target file NOT to exist — a stale
/// backup is removed first. The path is passed as a bound parameter so that
/// user names containing quotes (Windows %APPDATA%) cannot break the SQL.
pub async fn backup_before_migration(pool: &SqlitePool, db_path: &std::path::Path) -> Result<()> {
    if !has_pending(pool).await? {
        return Ok(());
    }
    let bak_path = db_path.with_extension("db.bak"); // mcphub.db -> mcphub.db.bak
    // VACUUM INTO fails if the target exists — remove stale backup first.
    let _ = std::fs::remove_file(&bak_path);
    let start = std::time::Instant::now();
    sqlx::query("VACUUM INTO ?1")
        .bind(bak_path.to_string_lossy().as_ref())
        .execute(pool)
        .await?;
    log::info!(
        "[db] pre-migration backup: {} ({}ms)",
        bak_path.display(),
        start.elapsed().as_millis()
    );
    Ok(())
}

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
        10 => migrate_v10(pool).await,
        11 => migrate_v11(pool).await,
        12 => migrate_v12(pool).await,
        13 => migrate_v13(pool).await,
        14 => migrate_v14(pool).await,
        15 => migrate_v15(pool).await,
        16 => migrate_v16(pool).await,
        17 => migrate_v17(pool).await,
        18 => migrate_v18(pool).await,
        19 => migrate_v19(pool).await,
        20 => migrate_v20(pool).await,
        21 => migrate_v21(pool).await,
        22 => migrate_v22(pool).await,
        23 => migrate_v23(pool).await,
        24 => migrate_v24(pool).await,
        25 => migrate_v25(pool).await,
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
            http_port   INTEGER DEFAULT 23333,
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
        sqlx::query(sqlx::AssertSqlSafe(&*sql)).execute(pool).await.ok();
    }
    for table in &["users", "servers", "templates", "server_tool_config"] {
        let sql = format!(
            "UPDATE {} SET updated_at = datetime(updated_at, 'localtime') WHERE updated_at IS NOT NULL",
            table
        );
        sqlx::query(sqlx::AssertSqlSafe(&*sql)).execute(pool).await.ok();
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

/// v9 → v10: Add per_session_client column to servers table
///
/// Mirrors origin `d74d1be` (#985): a server config flag that gives each
/// downstream HTTP MCP session its own dedicated upstream client/connection
/// instead of sharing the pool's single client (for stateful servers like
/// Playwright). Stored as INTEGER (0/1); default 0 (shared pool, original
/// behavior).
async fn migrate_v10(pool: &SqlitePool) -> Result<()> {
    // 用 add_column_if_missing 幂等加列(见该 helper 的注释:不能用 .ok() 吞错误)。
    add_column_if_missing(pool, "servers", "per_session_client", "INTEGER NOT NULL DEFAULT 0").await?;
    Ok(())
}

/// v10 → v11: Drop dead `server_name` NOT NULL column from builtin_prompts / builtin_resources.
///
/// v1 created both with `server_name TEXT NOT NULL` for a speculative per-server
/// design that was never wired up — the app treats prompts/resources as global
/// (matches the origin entity, which has no server_name). The INSERTs in
/// prompt_service / resource_service never bind server_name, so every create
/// failed with `NOT NULL constraint failed: builtin_prompts.server_name`.
/// Drop the column; ensure title/template (prompts) and content (resources)
/// exist for DBs that pre-date their ADD COLUMN.
async fn migrate_v11(pool: &SqlitePool) -> Result<()> {
    for table in &["builtin_prompts", "builtin_resources"] {
        let sql = format!("ALTER TABLE {} DROP COLUMN server_name", table);
        sqlx::query(sqlx::AssertSqlSafe(&*sql)).execute(pool).await.ok(); // ignore if column already absent
    }
    sqlx::query("ALTER TABLE builtin_prompts ADD COLUMN title TEXT")
        .execute(pool).await.ok();
    sqlx::query("ALTER TABLE builtin_prompts ADD COLUMN template TEXT NOT NULL DEFAULT ''")
        .execute(pool).await.ok();
    sqlx::query("ALTER TABLE builtin_resources ADD COLUMN content TEXT NOT NULL DEFAULT ''")
        .execute(pool).await.ok();
    Ok(())
}

/// v11 → v12: Add per-group builtin prompt/resource selection columns.
///
/// Built-in prompts/resources are global (no server_name). Until now they were
/// exposed in full to every group's `/mcp/{group}` route. v12 lets a group
/// record which builtin prompts/resources it exposes: NULL = expose all
/// (back-compat), `[]` = none, `["x","y"]` = only those. Stored as JSON text
/// arrays (prompt names / resource URIs). Columns are nullable with no default
/// so existing rows stay NULL = all.
async fn migrate_v12(pool: &SqlitePool) -> Result<()> {
    sqlx::query("ALTER TABLE groups ADD COLUMN builtin_prompts TEXT")
        .execute(pool).await.ok(); // ignore if column already exists
    sqlx::query("ALTER TABLE groups ADD COLUMN builtin_resources TEXT")
        .execute(pool).await.ok();
    Ok(())
}

/// v12 → v13: Skills tables.
///
/// `skills`: app-managed skill library (one row per imported skill dir).
/// `skill_exports`: per (skill, agent) install record with method + status.
/// Both carry a `status` column ('pending'|'ok') so a crash mid-import/export
/// leaves `pending` — only `ok` is treated as successful; `reconcile_pending`
/// (startup) cleans partial dirs + pending rows.
/// Also seeds `config_json.skills.agents` with known defaults if absent.
async fn migrate_v13(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS skills (
            id           TEXT PRIMARY KEY,
            dir_name     TEXT NOT NULL UNIQUE,
            name         TEXT,
            description  TEXT,
            source_agent TEXT,
            source_path  TEXT,
            status       TEXT NOT NULL DEFAULT 'pending',
            created_at   TEXT NOT NULL DEFAULT (datetime('now','localtime'))
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS skill_exports (
            id         TEXT PRIMARY KEY,
            skill_id   TEXT NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
            agent_id   TEXT NOT NULL,
            method     TEXT NOT NULL,
            status     TEXT NOT NULL DEFAULT 'pending',
            created_at TEXT NOT NULL DEFAULT (datetime('now','localtime')),
            UNIQUE(skill_id, agent_id)
        )",
    )
    .execute(pool)
    .await?;

    // Seed default known agents into config_json.skills.agents if the key is
    // absent. Paths use ~ (resolved at scan/export time by resolve_agent_path).
    let row = sqlx::query("SELECT config_json FROM system_config WHERE id=1")
        .fetch_optional(pool)
        .await?;
    let needs_seed = match row.as_ref().and_then(|r| {
        let s: Option<String> = r.try_get("config_json").ok()?;
        s.and_then(|v| serde_json::from_str::<serde_json::Value>(&v).ok())
    }) {
        Some(v) => !v.get("skills").and_then(|s| s.get("agents")).is_some(),
        None => true,
    };
    if needs_seed {
        let mut config: serde_json::Value = row
            .as_ref()
            .and_then(|r| {
                let s: Option<String> = r.try_get("config_json").ok()?;
                s.and_then(|v| serde_json::from_str(&v).ok())
            })
            .unwrap_or_else(|| serde_json::json!({}));
        if !config.is_object() {
            config = serde_json::json!({});
        }
        if config.get("skills").is_none() {
            config["skills"] = serde_json::json!({});
        }
        // Single source of truth: skill_service::default_agents() (12 known).
        config["skills"]["agents"] = serde_json::to_value(crate::services::skill_service::default_agents())?;
        let json_str = serde_json::to_string(&config)?;
        sqlx::query(
            "UPDATE system_config SET config_json=?, updated_at=datetime('now','localtime') WHERE id=1",
        )
        .bind(&json_str)
        .execute(pool)
        .await?;
    }

    log::info!("[db] migration v13: created skills/skill_exports tables, seeded default agents");
    Ok(())
}

/// v13 → v14: Switch the known-agents source to the bundled `install.json`
/// catalog (56 agents) — replaces the old hardcoded 4-agent v13 seed.
///
/// Behavior:
/// - No `skills.agents` at all → seed the full catalog.
/// - Current agent ids are EXACTLY the legacy v13 set {claude-code, cursor,
///   windsurf, cline} (untouched defaults) → REPLACE with the full catalog
///   (so the user moves from 4 → 56 cleanly).
/// - Otherwise (user has added/edited agents) → backfill missing catalog ids,
///   leaving user additions/edits intact.
async fn migrate_v14(pool: &SqlitePool) -> Result<()> {
    /// The ids v13 originally seeded — used to detect an untouched config.
    const LEGACY_V13_IDS: &[&str] = &["claude-code", "cursor", "windsurf", "cline"];
    let legacy: std::collections::HashSet<String> =
        LEGACY_V13_IDS.iter().map(|s| s.to_string()).collect();

    let row = sqlx::query("SELECT config_json FROM system_config WHERE id=1")
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else { return Ok(()); };
    let s: Option<String> = row.try_get("config_json")?;
    let mut config: serde_json::Value = match s.and_then(|v| serde_json::from_str::<serde_json::Value>(&v).ok()) {
        Some(v) if v.is_object() => v,
        _ => serde_json::json!({}),
    };

    if config.get("skills").is_none() {
        config["skills"] = serde_json::json!({});
    }

    let defaults = crate::services::skill_service::default_agents();
    let arr = config["skills"].get("agents").and_then(|a| a.as_array()).cloned();
    match arr {
        None => {
            config["skills"]["agents"] = serde_json::to_value(&defaults)?;
        }
        Some(agents) if agents.is_empty() => {
            config["skills"]["agents"] = serde_json::to_value(&defaults)?;
        }
        Some(agents) => {
            // Owned ids so `agents` can move into `merged` below.
            let current: std::collections::HashSet<String> = agents
                .iter()
                .filter_map(|a| a.get("id").and_then(|i| i.as_str()).map(|s| s.to_string()))
                .collect();
            if current == legacy {
                // Untouched v13 defaults → replace with the full catalog.
                config["skills"]["agents"] = serde_json::to_value(&defaults)?;
            } else {
                // User has customized → only backfill missing catalog ids.
                let mut merged = agents;
                for def in &defaults {
                    if !current.contains(def.id.as_str()) {
                        merged.push(serde_json::to_value(def)?);
                    }
                }
                config["skills"]["agents"] = serde_json::Value::Array(merged);
            }
        }
    }

    let json_str = serde_json::to_string(&config)?;
    sqlx::query("UPDATE system_config SET config_json=?, updated_at=datetime('now','localtime') WHERE id=1")
        .bind(&json_str)
        .execute(pool)
        .await?;
    log::info!("[db] migration v14: known-agents catalog applied ({} agents)", defaults.len());
    Ok(())
}

/// v14 → v15: Seed RAG defaults into `config_json.rag` if absent.
///
/// RAG config lives in the same `config_json` blob as skills (vector DB files
/// are separate, under app_data_dir/rag/lancedb). Defaults:
///   { enabled: false, vectorWeight: 0.9, keywordWeight: 0.1, maxResults: 20 }
/// These MUST match `RagSettings::default()` in `models/rag.rs` — `get_settings`
/// only falls back to the struct defaults for *missing* keys, so a value seeded
/// here shadows the struct default forever. Only seeds when the `rag` key is
/// missing — never overwrites user edits. (Pre-existing installs seeded with
/// the old 0.5/0.5 split are corrected by `migrate_v18`.)
async fn migrate_v15(pool: &SqlitePool) -> Result<()> {
    let row = sqlx::query("SELECT config_json FROM system_config WHERE id=1")
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else { return Ok(()); };
    let s: Option<String> = row.try_get("config_json")?;
    let mut config: serde_json::Value = match s.and_then(|v| serde_json::from_str::<serde_json::Value>(&v).ok()) {
        Some(v) if v.is_object() => v,
        _ => serde_json::json!({}),
    };

    if config.get("rag").is_none() {
        config["rag"] = serde_json::json!({
            "enabled": false,
            "vectorWeight": 0.9,
            "keywordWeight": 0.1,
            "maxResults": 20
        });
        let json_str = serde_json::to_string(&config)?;
        sqlx::query("UPDATE system_config SET config_json=?, updated_at=datetime('now','localtime') WHERE id=1")
            .bind(&json_str)
            .execute(pool)
            .await?;
        log::info!("[db] migration v15: seeded rag config defaults");
    } else {
        log::info!("[db] migration v15: rag config already present, skipped");
    }
    Ok(())
}

/// v15 → v16: RAG tag statistics table.
///
/// `rag_tag_stats` keeps one row per distinct tag with the count of documents
/// that carry it. Recomputed by the rag service on every tag-changing op
/// (upload / set_doc_tags / delete / batch). A tag whose count drops to 0 is
/// simply not (re)inserted — i.e. dropped — which is the desired "delete when
/// fileCount=0" behavior.
async fn migrate_v16(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS rag_tag_stats (
            tag         TEXT PRIMARY KEY,
            file_count INTEGER NOT NULL DEFAULT 0
        )",
    )
    .execute(pool)
    .await?;
    log::info!("[db] migration v16: created rag_tag_stats table");
    Ok(())
}

/// v16 → v17: Add `start_on_demand` / `idle_timeout_ms` columns to `servers`
/// for on-demand stdio server spawning (origin PR #1012).
async fn migrate_v17(pool: &SqlitePool) -> Result<()> {
    // ⚠️ 不能用 .ok() 吞错误:若 ALTER 失败而版本号仍被推进,会导致
    // schema_version 与实际 schema 不一致,后续启动不重跑该迁移,SELECT
    // 永久报 "no such column"。改用 add_column_if_missing 幂等加列。
    add_column_if_missing(pool, "servers", "start_on_demand", "INTEGER NOT NULL DEFAULT 0").await?;
    add_column_if_missing(pool, "servers", "idle_timeout_ms", "INTEGER NOT NULL DEFAULT 0").await?;
    log::info!("[db] migration v17: added start_on_demand / idle_timeout_ms columns to servers");
    Ok(())
}

/// v17 → v18: Correct stale RAG search-weight seed (0.5/0.5 → 0.9/0.1).
///
/// `migrate_v15` originally seeded `vectorWeight: 0.5, keywordWeight: 0.5`,
/// which shadowed the `RagSettings::default()` split (0.9/0.1) because
/// `get_settings` only falls back to the struct default for *missing* keys.
/// The struct default was always 0.9/0.1 (vector dominates; keyword is a
/// recall backstop), so the seeded 0.5/0.5 was simply a bug.
///
/// This migration rewrites the stored weights to 0.9/0.1, but ONLY when they
/// still equal the old seeded 0.5/0.5 — i.e. when the user never touched the
/// search settings dialog (any save through `save_settings` would have stored
/// a different pair). A user who deliberately set 0.5/0.5 is indistinguishable
/// from the stale seed, so we accept that edge case to fix the far more common
/// "untouched install shows the wrong default" path. Idempotent.
async fn migrate_v18(pool: &SqlitePool) -> Result<()> {
    let row = sqlx::query("SELECT config_json FROM system_config WHERE id=1")
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else { return Ok(()); };
    let s: Option<String> = row.try_get("config_json")?;
    let mut config: serde_json::Value = match s.and_then(|v| serde_json::from_str::<serde_json::Value>(&v).ok()) {
        Some(v) if v.is_object() => v,
        _ => return Ok(()),
    };

    let Some(rag) = config.get_mut("rag") else {
        // No rag config yet — v15 (now corrected) will seed 0.9/0.1 on a later
        // fresh path, or the user simply hasn't enabled RAG. Nothing to do.
        return Ok(());
    };
    let Some(obj) = rag.as_object_mut() else { return Ok(()); };

    // 0.5/0.5 are exactly representable in f64, so an exact compare is safe;
    // a tiny epsilon guards against any future rounding on save.
    let is_old_seed = |key: &str| -> bool {
        obj.get(key)
            .and_then(|v| v.as_f64())
            .map(|v| (v - 0.5).abs() < 1e-9)
            .unwrap_or(false)
    };
    if is_old_seed("vectorWeight") && is_old_seed("keywordWeight") {
        obj["vectorWeight"] = serde_json::json!(0.9);
        obj["keywordWeight"] = serde_json::json!(0.1);
        let json_str = serde_json::to_string(&config)?;
        sqlx::query("UPDATE system_config SET config_json=?, updated_at=datetime('now','localtime') WHERE id=1")
            .bind(&json_str)
            .execute(pool)
            .await?;
        log::info!("[db] migration v18: corrected stale RAG weight seed 0.5/0.5 → 0.9/0.1");
    } else {
        log::info!("[db] migration v18: RAG weights already customized, left untouched");
    }
    Ok(())
}

/// v18 → v19: Correct stale HTTP port seed (3000 → 23333).
///
/// The v1 schema seeded `system_config.http_port DEFAULT 3000`, and `migrate_v3`
/// copied that column into `config_json.httpPort`. That shadowed the runtime
/// default in `http_server::maybe_start` (`unwrap_or(23333)` only fires when the
/// key is absent), so every install that ran v3 ended up bound to 3000 — the
/// "changed default to 23333" only updated the dead fallback. The v1 column
/// default is now 23333, so fresh installs are correct; this migration fixes
/// existing installs.
///
/// Rewrites `config_json.httpPort` 3000 → 23333 ONLY when it still equals the
/// old seeded 3000 — i.e. the user never customized the port. A user who
/// deliberately set 3000 is indistinguishable from the stale seed, so we accept
/// that edge case to fix the far more common "untouched install on the wrong
/// port" path. Idempotent. Runs before `http_server::maybe_start`, so the very
/// first launch after upgrade binds 23333 without a restart.
async fn migrate_v19(pool: &SqlitePool) -> Result<()> {
    let row = sqlx::query("SELECT config_json FROM system_config WHERE id=1")
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else { return Ok(()); };
    let s: Option<String> = row.try_get("config_json")?;
    let mut config: serde_json::Value = match s.and_then(|v| serde_json::from_str::<serde_json::Value>(&v).ok()) {
        Some(v) if v.is_object() => v,
        _ => return Ok(()),
    };

    let is_old_seed = config
        .get("httpPort")
        .and_then(|v| v.as_f64())
        .map(|v| (v - 3000.0).abs() < 1e-9)
        .unwrap_or(false);
    if is_old_seed {
        if let Some(obj) = config.as_object_mut() {
            obj["httpPort"] = serde_json::json!(23333);
        }
        let json_str = serde_json::to_string(&config)?;
        sqlx::query("UPDATE system_config SET config_json=?, updated_at=datetime('now','localtime') WHERE id=1")
            .bind(&json_str)
            .execute(pool)
            .await?;
        log::info!("[db] migration v19: corrected stale HTTP port seed 3000 → 23333");
    } else {
        log::info!("[db] migration v19: httpPort already customized or absent, left untouched");
    }
    Ok(())
}

/// SQLite 不支持 `ALTER TABLE ... ADD COLUMN IF NOT EXISTS`,用
/// `pragma_table_info` 检查列是否存在,不存在才 ADD。
///
/// 关键:ADD 失败时返回 `Err`(而非 `.ok()` 吞掉),这样迁移失败会让
/// `set_version` 不执行,下次启动会重试该迁移。若用 `.ok()` 吞错误,版本号
/// 会被推进到 N+1 但列实际没加上,数据库进入"版本号=N+1、schema 实际=N"的
/// 不一致状态,后续启动看到 current>=target 不再重跑,导致依赖新列的
/// SELECT 永久失败(曾导致服务器列表读不出、用户以为数据丢失)。
async fn add_column_if_missing(
    pool: &SqlitePool,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    let exists: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(&*format!(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('{}') WHERE name = '{}')",
        table, column
    )))
    .fetch_one(pool)
    .await
    .map_err(|e| anyhow!("check column {}.{} existence failed: {}", table, column, e))?;
    if exists == 0 {
        sqlx::query(sqlx::AssertSqlSafe(&*format!(
            "ALTER TABLE {} ADD COLUMN {} {}",
            table, column, definition
        )))
        .execute(pool)
        .await
        .map_err(|e| anyhow!("add column {}.{} failed: {}", table, column, e))?;
        log::info!("[db] added column {}.{} ({})", table, column, definition);
    } else {
        log::debug!("[db] column {}.{} already exists, skip", table, column);
    }
    Ok(())
}

/// v19 → v20: servers 表添加 proxy 列（Proxychains4 配置 JSON）。
///
/// 上游 #1055 引入 proxy 配置的 round-trip（前端编辑服务器时原样带回，
/// 避免无表单编辑器的字段被静默丢弃并触发无谓重连）。桌面端跟随：模型
/// 加 `proxy` 字段、DB 持久化该 JSON。
async fn migrate_v20(pool: &SqlitePool) -> Result<()> {
    add_column_if_missing(pool, "servers", "proxy", "TEXT").await?;
    log::info!("[db] migration v20: added proxy column to servers");
    Ok(())
}

/// v20 -> v21: RAG 标签/文档 SQL 化 + 全库查询索引。
///
/// 三张新 RAG 表（`.meta` 文件仍是唯一事实源，这些表是随 CRUD 增量维护的
/// 查询镜像，启动时由 RAG 服务对账重建）：
///   - `rag_tags(tag PK, file_count)`：独立标签表，file_count 是该标签关联的
///     文档数（随关联表 CRUD 增减，减到 0 由服务层删除该行）。取代 v16 的
///     `rag_tag_stats`（数据平移后 DROP）。
///   - `rag_doc_tags(doc_id, tag)`：标签-文档关联表。
///   - `rag_docs`：文档元数据镜像（供文件名/标签过滤的 SQL 分页搜索）。
///
/// 现有表补显式索引（此前全库除 UNIQUE/PK 自动索引外没有任何索引）：
/// activity_log（ORDER BY created_at DESC + server/status 等值 + tool LIKE）、
/// app_log（created_at DESC）、builtin_prompts/builtin_resources（name/uri，
/// 供搜索的 ORDER BY/前缀）、skills（status+dir_name 列表查询）、
/// bearer_keys（created_at DESC 列表排序）。
///
/// 注意：`rag_doc_tags` / `rag_docs` 的数据回填不在这里做（迁移拿不到
/// AppHandle/files_dir），由 RAG 服务启动时的全量对账完成。
async fn migrate_v21(pool: &SqlitePool) -> Result<()> {
    // ---- RAG 标签表：rag_tag_stats -> rag_tags（数据平移） ----
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS rag_tags (
            tag         TEXT PRIMARY KEY,
            file_count  INTEGER NOT NULL DEFAULT 0
        )",
    )
    .execute(pool)
    .await?;
    // 幂等：只在旧表还有数据而新表为空时平移（防重跑重复插入）
    let migrated: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM rag_tags) + (SELECT COUNT(*) FROM rag_tag_stats)",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    let new_empty: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rag_tags")
        .fetch_one(pool)
        .await?;
    if migrated > 0 && new_empty == 0 {
        sqlx::query("INSERT INTO rag_tags (tag, file_count) SELECT tag, file_count FROM rag_tag_stats")
            .execute(pool)
            .await?;
    }
    sqlx::query("DROP TABLE IF EXISTS rag_tag_stats")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_rag_tags_file_count ON rag_tags(file_count DESC, tag)")
        .execute(pool)
        .await?;

    // ---- RAG 标签-文档关联表 ----
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS rag_doc_tags (
            doc_id  TEXT NOT NULL,
            tag     TEXT NOT NULL,
            PRIMARY KEY (doc_id, tag)
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_rag_doc_tags_tag ON rag_doc_tags(tag)")
        .execute(pool)
        .await?;

    // ---- RAG 文档元数据镜像表 ----
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS rag_docs (
            id            TEXT PRIMARY KEY,
            name          TEXT NOT NULL,
            size          INTEGER NOT NULL DEFAULT 0,
            uploaded_at   TEXT NOT NULL DEFAULT (datetime('now','localtime')),
            file_type     TEXT,
            method        TEXT,
            original_path TEXT,
            md5           TEXT,
            version       INTEGER NOT NULL DEFAULT 1,
            chunk_count   INTEGER NOT NULL DEFAULT 0
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_rag_docs_name ON rag_docs(name)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_rag_docs_uploaded_at ON rag_docs(uploaded_at DESC)")
        .execute(pool)
        .await?;

    // ---- 现有表查询索引（全部幂等） ----
    let index_statements: &[&str] = &[
        "CREATE INDEX IF NOT EXISTS idx_activity_log_created_at ON activity_log(created_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_activity_log_server ON activity_log(server)",
        "CREATE INDEX IF NOT EXISTS idx_activity_log_tool ON activity_log(tool)",
        "CREATE INDEX IF NOT EXISTS idx_activity_log_status ON activity_log(status)",
        "CREATE INDEX IF NOT EXISTS idx_app_log_created_at ON app_log(created_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_builtin_prompts_name ON builtin_prompts(name)",
        "CREATE INDEX IF NOT EXISTS idx_builtin_resources_name ON builtin_resources(name)",
        "CREATE INDEX IF NOT EXISTS idx_builtin_resources_uri ON builtin_resources(uri)",
        "CREATE INDEX IF NOT EXISTS idx_skills_status_dir_name ON skills(status, dir_name)",
        "CREATE INDEX IF NOT EXISTS idx_bearer_keys_created_at ON bearer_keys(created_at DESC)",
    ];
    for stmt in index_statements {
        sqlx::query(sqlx::AssertSqlSafe(*stmt))
            .execute(pool)
            .await
            .map_err(|e| anyhow!("create index failed: {} ({})", stmt, e))?;
    }

    log::info!("[db] migration v21: created rag_tags/rag_doc_tags/rag_docs + lookup indexes");
    Ok(())
}

/// v21 -> v22: RAG 标签表补 created_at + 排序索引 + 日志级别/业务表时间索引。
///
/// 1. `rag_tags` 增加 `created_at`（标签首次创建时间，毫秒时间戳 TEXT）——
///    标签下拉排序要求「关联文件数倒序 + 创建时间倒序」，需要记录标签的
///    创建时间。存量行回填为 0（排序时退化为 file_count DESC, tag ASC 的旧
///    语义，启动对账 `rebuild_rag_sql_index` 会用当前时间重写）。
/// 2. 排序索引：`rag_tags(file_count DESC, created_at DESC, tag)`。
/// 3. 补齐日志级别 + 各业务表 created_at 查询索引（v21 已给 activity_log /
///    app_log / bearer_keys 等加了部分索引；本迁移补 `app_log(level)` 和
///    servers/groups/builtin_prompts/builtin_resources/skills 等的
///    `created_at DESC`）。
///
/// 全部 `IF NOT EXISTS` 幂等；DDL 失败返回 Err（不吞错，版本号不脱节）。
async fn migrate_v22(pool: &SqlitePool) -> Result<()> {
    // ---- rag_tags 增加 created_at 列（SQLite 无 ADD COLUMN IF NOT EXISTS） ----
    add_column_if_missing(
        pool,
        "rag_tags",
        "created_at",
        "TEXT NOT NULL DEFAULT ''",
    )
    .await?;

    let index_statements: &[&str] = &[
        // 标签下拉排序：file_count DESC, created_at DESC（tag 兜底 tie-break）
        "CREATE INDEX IF NOT EXISTS idx_rag_tags_file_count ON rag_tags(file_count DESC, created_at DESC, tag)",
        // app_log：级别筛选（info/warn/error/debug）
        "CREATE INDEX IF NOT EXISTS idx_app_log_level ON app_log(level)",
        // 业务表 created_at（倒序列表排序）
        "CREATE INDEX IF NOT EXISTS idx_servers_created_at ON servers(created_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_groups_created_at ON groups(created_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_builtin_prompts_created_at ON builtin_prompts(created_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_builtin_resources_created_at ON builtin_resources(created_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_skills_created_at ON skills(created_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_skill_exports_created_at ON skill_exports(created_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_templates_created_at ON templates(created_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_server_tool_config_created_at ON server_tool_config(created_at DESC)",
    ];
    for stmt in index_statements {
        sqlx::query(sqlx::AssertSqlSafe(*stmt))
            .execute(pool)
            .await
            .map_err(|e| anyhow!("create index failed: {} ({})", stmt, e))?;
    }
    log::info!("[db] migration v22: rag_tags.created_at + level/created_at indexes");
    Ok(())
}

/// v22 -> v23: 索引补全/清理。
///
/// 1. 描述字段索引：servers/groups/builtin_prompts/builtin_resources/skills
///    的 `description`（LIKE 搜索/排序用）。此前这些表只有 name/created_at 等索引，
///    描述字段搜索走全表扫。
/// 2. 删除 `builtin_resources` 的 uri 索引（`idx_builtin_resources_uri`）——
///    resource 的定位用 name 即可，uri 不再单独索引（用户反馈不需要）。
/// 3. 遗漏点补全：
///    - `bearer_keys.user_id`（外键 ON DELETE CASCADE，删用户时要按 user_id 查 key）
///    - `activity_log.key_id` / `activity_log.source_ip`（按 key/IP 审计统计）
///
/// 注意 #3 中的索引用 `create_index_if_column_exists` 列存在性守卫，而非无条件
/// `CREATE INDEX`。历史路径里 `bearer_keys` 经历过两版 schema：v1 的 `CREATE
/// TABLE` 带 `user_id`，但旧 sqlx 迁移 `0002_schema_fix.sql` `DROP TABLE` 后
/// 重建为不带 `user_id` 的 `token`-中心表（见 `bearer_key_service`，所有 SELECT
/// /INSERT 都不碰 user_id）。`get_current_version` 把旧迁移数映射成版本号后，这些
/// 库的 bearer_keys 实际没有 user_id 列；无条件 `CREATE INDEX ON bearer_keys
/// (user_id)` 会以 `no such column: user_id` 失败，让整个迁移 Err → DB 初始化
/// fatal。列守卫让缺失列的单个索引跳过（并 log），其余索引照建，迁移正常完成。
async fn migrate_v23(pool: &SqlitePool) -> Result<()> {
    // 删除 uri 索引（幂等：不存在则 no-op）
    sqlx::query("DROP INDEX IF EXISTS idx_builtin_resources_uri")
        .execute(pool)
        .await?;

    // description 索引（各业务表描述字段搜索）——这些表自 v1 起就带 description
    // 列，无需守卫。
    let index_statements: &[&str] = &[
        "CREATE INDEX IF NOT EXISTS idx_servers_description ON servers(description)",
        "CREATE INDEX IF NOT EXISTS idx_groups_description ON groups(description)",
        "CREATE INDEX IF NOT EXISTS idx_builtin_prompts_description ON builtin_prompts(description)",
        "CREATE INDEX IF NOT EXISTS idx_builtin_resources_description ON builtin_resources(description)",
        "CREATE INDEX IF NOT EXISTS idx_skills_description ON skills(description)",
    ];
    for stmt in index_statements {
        sqlx::query(sqlx::AssertSqlSafe(*stmt))
            .execute(pool)
            .await
            .map_err(|e| anyhow!("create index failed: {} ({})", stmt, e))?;
    }

    // 遗漏点：外键 + 审计字段索引。这些列在不同历史 schema 路径下可能不存在
    // （bearer_keys.user_id 见上注释；activity_log 在 v9 之前是旧 user_id/action
    // 版本，没有 key_id/source_ip），用列守卫逐个建，缺失列的跳过。
    create_index_if_column_exists(pool, "idx_bearer_keys_user_id", "bearer_keys", "user_id").await?;
    create_index_if_column_exists(pool, "idx_activity_log_key_id", "activity_log", "key_id").await?;
    create_index_if_column_exists(pool, "idx_activity_log_source_ip", "activity_log", "source_ip").await?;

    log::info!("[db] migration v23: description indexes + bearer_keys.user_id + activity_log key/source_ip; dropped uri index");
    Ok(())
}

/// v23 → v24: FTS5 full-text index tables (Chinese/English/pinyin tokenization).
///
/// Creates one FTS5 table per searchable entity (7 tables: 5 entity mirrors +
/// rag_docs + app_log). Migration only creates the (empty) tables — data
/// backfill is done by `fts_service::rebuild_all()` (5 entity tables) and the
/// RAG service's `rebuild_rag_sql_index()` (fts_rag_docs) at startup, because
/// migration cannot run the charabia tokenizer. Mirrors the v21 rag_docs
/// pattern (agent.md §3.5.5 / doc/sqlite_fts_pinyin_plan_20260905.md §4.1).
///
/// Columns:
///   ref_id: source-table locator (UNINDEXED):
///           servers→name, groups→id, rag_docs→id, skills→dir_name,
///           prompts/resources→name, app_log→id
///   zh:  tokenized word sequence (charabia lemma, space separated)
///   py:  per-CJK-word full-pinyin token (数据库→shujuku; latin words kept)
///   ini: per-CJK-word initials token (数据库→sjk)
async fn migrate_v24(pool: &SqlitePool) -> Result<()> {
    // Smoke assertion: bundled SQLite must be built with FTS5 (fail-fast so we
    // never silently leave a half-migrated state).
    let fts5: i64 = sqlx::query_scalar("SELECT sqlite_compileoption_used('ENABLE_FTS5')")
        .fetch_one(pool)
        .await?;
    if fts5 == 0 {
        return Err(anyhow!(
            "FTS5 not available in bundled SQLite (SQLITE_ENABLE_FTS5 missing from compile options)"
        ));
    }

    let fts_tables: &[&str] = &[
        "fts_servers",
        "fts_groups",
        "fts_rag_docs",
        "fts_skills",
        "fts_prompts",
        "fts_resources",
        "fts_app_log",
    ];
    for table in fts_tables {
        sqlx::query(sqlx::AssertSqlSafe(
            format!(
                "CREATE VIRTUAL TABLE IF NOT EXISTS {table} USING fts5(ref_id UNINDEXED, zh, py, ini)"
            )
            .leak() as &'static str,
        ))
        .execute(pool)
        .await
        .map_err(|e| anyhow!("create FTS5 table {} failed: {}", table, e))?;
    }

    log::info!("[db] migration v24: FTS5 full-text tables created ({} tables)", fts_tables.len());
    Ok(())
}

/// v24 → v25: Backfill newly added known-agent catalog entries into the
/// existing `config_json.skills.agents` list.
///
/// The catalog (`runtimes/skill/install.json`) gained the generic agent
/// "通用 Agent" → `~/.agent/skills`. `list_agents` only falls back to the
/// bundled catalog when the user has never configured agents, so existing
/// installs would never see the new entry without this backfill. Behavior
/// mirrors the "user customized" branch of v14: append missing catalog ids,
/// leave user additions/edits (and their order) intact. Idempotent — missing
/// ids only.
async fn migrate_v25(pool: &SqlitePool) -> Result<()> {
    let row = sqlx::query("SELECT config_json FROM system_config WHERE id=1")
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else { return Ok(()); };
    let s: Option<String> = row.try_get("config_json")?;
    let mut config: serde_json::Value = match s.and_then(|v| serde_json::from_str::<serde_json::Value>(&v).ok()) {
        Some(v) if v.is_object() => v,
        _ => serde_json::json!({}),
    };

    if config.get("skills").is_none() {
        config["skills"] = serde_json::json!({});
    }

    let defaults = crate::services::skill_service::default_agents();
    let arr = config["skills"].get("agents").and_then(|a| a.as_array()).cloned();
    match arr {
        None => {
            config["skills"]["agents"] = serde_json::to_value(&defaults)?;
        }
        Some(agents) if agents.is_empty() => {
            config["skills"]["agents"] = serde_json::to_value(&defaults)?;
        }
        Some(agents) => {
            let current: std::collections::HashSet<String> = agents
                .iter()
                .filter_map(|a| a.get("id").and_then(|i| i.as_str()).map(|s| s.to_string()))
                .collect();
            let mut merged = agents;
            for def in &defaults {
                if !current.contains(def.id.as_str()) {
                    merged.push(serde_json::to_value(def)?);
                }
            }
            config["skills"]["agents"] = serde_json::Value::Array(merged);
        }
    }

    let json_str = serde_json::to_string(&config)?;
    sqlx::query("UPDATE system_config SET config_json=?, updated_at=datetime('now','localtime') WHERE id=1")
        .bind(&json_str)
        .execute(pool)
        .await?;
    log::info!("[db] migration v25: known-agents catalog backfilled ({} agents total)", defaults.len());
    Ok(())
}

/// 幂等建索引，且目标列不存在时跳过（不报错）。
///
/// 与无条件 `CREATE INDEX` 的区别：历史上有过多个 schema 路径（旧 sqlx 迁移
/// `0001/0002` + 现行 `migrate_v1..v23`），同一张表在不同库上可能落在不同
/// 版本的列集合上。若某索引依赖的列在该库不存在，无条件建索引会让整个迁移
/// `Err`、DB 初始化 fatal。这里先查 `pragma_table_info`，列缺失则跳过并 log。
async fn create_index_if_column_exists(
    pool: &SqlitePool,
    index_name: &str,
    table: &str,
    column: &str,
) -> Result<()> {
    let exists: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(&*format!(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('{}') WHERE name = '{}')",
        table, column
    )))
    .fetch_one(pool)
    .await
    .map_err(|e| anyhow!("check column {}.{} existence failed: {}", table, column, e))?;
    if exists == 0 {
        log::warn!(
            "[db] skip index {}: column {}.{} does not exist in this DB (legacy schema path)",
            index_name, table, column
        );
        return Ok(());
    }
    let stmt = format!(
        "CREATE INDEX IF NOT EXISTS {} ON {}({})",
        index_name, table, column
    );
    sqlx::query(sqlx::AssertSqlSafe(&*stmt))
        .execute(pool)
        .await
        .map_err(|e| anyhow!("create index failed: {} ({})", stmt, e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// v21 迁移在全新库上应建出三张 RAG 表 + 全部查询索引，且版本号推进到
    /// TARGET_VERSION；重跑（run_pending 二次调用）应为幂等 no-op。
    #[tokio::test]
    async fn v21_creates_rag_tables_and_indexes() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");

        run_pending(&pool).await.expect("run migrations");

        let version: i64 =
            sqlx::query_scalar("SELECT version FROM schema_version WHERE id = 1")
                .fetch_one(&pool)
                .await
                .expect("read version");
        assert_eq!(version, TARGET_VERSION);

        // 表存在性
        for table in ["rag_tags", "rag_doc_tags", "rag_docs"] {
            let n: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
            )
            .bind(table)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(n, 1, "table {} should exist", table);
        }
        // 旧表应已删除
        let old: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='rag_tag_stats'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(old, 0, "rag_tag_stats should be dropped");

        // 索引存在性（v21 新增 + 现有表补的查询索引）
        for idx in [
            "idx_rag_tags_file_count",
            "idx_rag_doc_tags_tag",
            "idx_rag_docs_name",
            "idx_rag_docs_uploaded_at",
            "idx_activity_log_created_at",
            "idx_activity_log_server",
            "idx_activity_log_tool",
            "idx_activity_log_status",
            "idx_app_log_created_at",
            "idx_builtin_prompts_name",
            "idx_builtin_resources_name",
            "idx_skills_status_dir_name",
            "idx_bearer_keys_created_at",
            // v22：app_log 级别 + 各业务表 created_at
            "idx_app_log_level",
            "idx_servers_created_at",
            "idx_groups_created_at",
            "idx_builtin_prompts_created_at",
            "idx_builtin_resources_created_at",
            "idx_skills_created_at",
            "idx_skill_exports_created_at",
            "idx_templates_created_at",
            "idx_server_tool_config_created_at",
            // v23：description 索引 + 遗漏点（uri 索引已删，见下方断言）。
            // 注意：在全新库上 bearer_keys.user_id / activity_log.key_id /
            // source_ip 列都存在（migrate_v1 建 bearer_keys 带 user_id，v9 重建
            // activity_log 带 key_id/source_ip），所以这三个索引应建出。
            "idx_servers_description",
            "idx_groups_description",
            "idx_builtin_prompts_description",
            "idx_builtin_resources_description",
            "idx_skills_description",
            "idx_bearer_keys_user_id",
            "idx_activity_log_key_id",
            "idx_activity_log_source_ip",
        ] {
            let n: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?",
            )
            .bind(idx)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(n, 1, "index {} should exist", idx);
        }

        // v23：uri 索引应已删除（v21 建过，v23 DROP）
        let uri_idx: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_builtin_resources_uri'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(uri_idx, 0, "idx_builtin_resources_uri should be dropped in v23");

        // 幂等：再跑一次不报错也不改版本
        run_pending(&pool).await.expect("re-run migrations (idempotent)");

        // v22：rag_tags.created_at 列存在且可写入
        sqlx::query("INSERT INTO rag_tags (tag, file_count, created_at) VALUES ('probe', 1, '123')")
            .execute(&pool)
            .await
            .expect("insert rag_tags with created_at");
        let created: String = sqlx::query_scalar("SELECT created_at FROM rag_tags WHERE tag='probe'")
            .fetch_one(&pool)
            .await
            .expect("read created_at");
        assert_eq!(created, "123");
    }

    /// v23 的列守卫回归：模拟旧 sqlx 迁移 0002 重建的 `bearer_keys`（带 token、
    /// 不带 user_id）+ 旧版 `activity_log`（无 key_id/source_ip），重跑 v23 应
    /// 跳过缺失列的索引、其余索引照建、迁移整体成功（不 fatal）。
    ///
    /// 复现原 bug：无条件 `CREATE INDEX ON bearer_keys(user_id)` 在没有 user_id
    /// 列时返回 `no such column: user_id`，让 `migrate_v23` Err → DB 初始化
    /// fatal。列守卫后应只 warn 跳过。
    #[tokio::test]
    async fn v23_skips_indexes_on_missing_columns() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        run_pending(&pool).await.expect("run migrations to v23");

        // 重建成旧 schema：bearer_keys 无 user_id（0002_schema_fix 的形状），
        // activity_log 无 key_id/source_ip（v9 之前的旧 user_id/action 形状）。
        sqlx::query("DROP TABLE bearer_keys").execute(&pool).await.unwrap();
        sqlx::query(
            "CREATE TABLE bearer_keys (
                id TEXT PRIMARY KEY, name TEXT NOT NULL, token TEXT NOT NULL UNIQUE,
                enabled INTEGER NOT NULL DEFAULT 1, access_type TEXT NOT NULL DEFAULT 'all',
                allowed_groups TEXT NOT NULL DEFAULT '[]', allowed_servers TEXT NOT NULL DEFAULT '[]',
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("DROP TABLE activity_log").execute(&pool).await.unwrap();
        sqlx::query(
            "CREATE TABLE activity_log (
                id TEXT PRIMARY KEY, user_id TEXT, action TEXT NOT NULL, resource TEXT NOT NULL,
                detail TEXT, created_at TEXT NOT NULL DEFAULT (datetime('now'))
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        // 删除 v23 可能已建的这三个索引，模拟"列刚被丢掉、索引待重跑"。
        for idx in [
            "idx_bearer_keys_user_id",
            "idx_activity_log_key_id",
            "idx_activity_log_source_ip",
        ] {
            sqlx::query(sqlx::AssertSqlSafe(&*format!("DROP INDEX IF EXISTS {}", idx)))
                .execute(&pool)
                .await
                .unwrap();
        }

        // 回退版本号到 v22，重跑 pending（=重跑 v23）。
        sqlx::query("UPDATE schema_version SET version = 22 WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        run_pending(&pool).await.expect("v23 must not fatal on missing columns");

        // 缺失列的索引应被跳过（不存在）。
        for idx in [
            "idx_bearer_keys_user_id",
            "idx_activity_log_key_id",
            "idx_activity_log_source_ip",
        ] {
            let n: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?",
            )
            .bind(idx)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(n, 0, "{} should be skipped (column absent)", idx);
        }
        // 其余 v23 索引（description）仍应正常建出。
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_servers_description'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(n, 1, "idx_servers_description should still be created");
        // 版本号应推进到 v23。
        let v: i64 = sqlx::query_scalar("SELECT version FROM schema_version WHERE id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(v, TARGET_VERSION);
    }

    /// 标签计数语义冒烟：直接驱动 rag_tags/rag_doc_tags 模拟 upsert/remove
    /// 的 SQL 形状 —— 计数随关联行增减，减到 0 的标签行删除。
    /// （upsert_doc_sql/remove_doc_sql 走全局 DB_POOL 无法单测，这里覆盖
    /// 它们执行的 SQL 语义本身。）
    #[tokio::test]
    async fn tag_count_semantics_decrement_to_zero_deletes() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        run_pending(&pool).await.expect("run migrations");

        // 模拟两个文档带上同一标签 a + 一个文档带标签 b
        for (doc, tags) in [("d1", vec!["a"]), ("d2", vec!["a", "b"])] {
            for t in tags {
                sqlx::query("INSERT INTO rag_doc_tags (doc_id, tag) VALUES (?, ?)")
                    .bind(doc)
                    .bind(t)
                    .execute(&pool)
                    .await
                    .unwrap();
                sqlx::query(
                    "INSERT INTO rag_tags (tag, file_count) VALUES (?, 1) \
                     ON CONFLICT(tag) DO UPDATE SET file_count = file_count + 1",
                )
                .bind(t)
                .execute(&pool)
                .await
                .unwrap();
            }
        }
        let count: i64 = sqlx::query_scalar("SELECT file_count FROM rag_tags WHERE tag='a'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 2);

        // 删除 d1：a 计数 -1（还剩 1，行保留）；再删 d2：a、b 都归零 -> 行删除
        for doc in ["d1", "d2"] {
            let old_tags: Vec<String> =
                sqlx::query_scalar("SELECT tag FROM rag_doc_tags WHERE doc_id = ?")
                    .bind(doc)
                    .fetch_all(&pool)
                    .await
                    .unwrap();
            for t in old_tags {
                sqlx::query("UPDATE rag_tags SET file_count = file_count - 1 WHERE tag = ?")
                    .bind(&t)
                    .execute(&pool)
                    .await
                    .unwrap();
                sqlx::query("DELETE FROM rag_tags WHERE tag = ? AND file_count <= 0")
                    .bind(&t)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            sqlx::query("DELETE FROM rag_doc_tags WHERE doc_id = ?")
                .bind(doc)
                .execute(&pool)
                .await
                .unwrap();
        }
        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rag_tags")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, 0, "zero-count tags must be deleted");
    }

    /// v24：FTS5 表应建出（7 张），重跑 run_pending 幂等；backup_before_migration
    /// 在有待迁移时产出 .bak、升级完成后不再生成（不覆盖既有备份）。
    /// VACUUM INTO 无法对 :memory: 库外的句柄语义做覆盖——这里用临时文件库。
    #[tokio::test]
    async fn v24_creates_fts_tables_and_backup_semantics() {
        let dir = std::env::temp_dir().join(format!("mcphub_v24_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("mcphub.db");
        let bak_path = db_path.with_extension("db.bak");
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(&bak_path);

        let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect(&db_url)
            .await
            .expect("connect file sqlite");

        // 有待迁移 → 备份生成（文件型快照）
        assert!(has_pending(&pool).await.unwrap(), "fresh db must have pending migrations");
        backup_before_migration(&pool, &db_path).await.expect("pre-migration backup");
        assert!(bak_path.exists(), "backup file should exist when migrations pending");

        run_pending(&pool).await.expect("run migrations to v24");

        // v24 FTS5 表存在性（7 张）
        for table in [
            "fts_servers",
            "fts_groups",
            "fts_rag_docs",
            "fts_skills",
            "fts_prompts",
            "fts_resources",
            "fts_app_log",
        ] {
            let n: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
            )
            .bind(table)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(n, 1, "FTS table {} should exist", table);
        }

        let version: i64 = sqlx::query_scalar("SELECT version FROM schema_version WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(version, TARGET_VERSION);

        // 无待迁移 → 备份 no-op（删除 bak 后不应重新生成）
        assert!(!has_pending(&pool).await.unwrap(), "no pending after v24");
        std::fs::remove_file(&bak_path).unwrap();
        backup_before_migration(&pool, &db_path).await.expect("no-op when up to date");
        assert!(!bak_path.exists(), "backup must NOT be recreated when up to date");

        // 幂等：重跑 run_pending 不报错、版本不变
        run_pending(&pool).await.expect("re-run migrations (idempotent)");
        let version2: i64 = sqlx::query_scalar("SELECT version FROM schema_version WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(version2, TARGET_VERSION);

        // MATCH 冒烟：FTS5 表可插入并查询（词元由 fts_service 生成，此处直插）
        sqlx::query("INSERT INTO fts_servers (ref_id, zh, py, ini) VALUES ('demo', '数据库', 'shujuku', 'sjk')")
            .execute(&pool)
            .await
            .unwrap();
        let hit: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM fts_servers WHERE fts_servers MATCH ?",
        )
        .bind("py:shu*")
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(hit, 1, "prefix query on py column should match");
        // 按行删除（FTS5 无 UPDATE，同步语义依赖 rowid 删除）
        let rowid: i64 = sqlx::query_scalar("SELECT rowid FROM fts_servers WHERE ref_id='demo'")
            .fetch_one(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM fts_servers WHERE rowid = ?")
            .bind(rowid)
            .execute(&pool)
            .await
            .unwrap();
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fts_servers")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 0, "row delete via rowid should work");

        pool.close().await;
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(&bak_path);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
