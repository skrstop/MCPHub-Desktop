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
pub const TARGET_VERSION: i64 = 19;

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
