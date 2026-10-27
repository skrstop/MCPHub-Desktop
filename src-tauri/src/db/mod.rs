pub mod migration;

use anyhow::Result;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    SqlitePool,
};
use std::{str::FromStr, sync::OnceLock, time::Duration};
use tauri::{AppHandle, Manager};

static DB_POOL: OnceLock<SqlitePool> = OnceLock::new();

pub fn pool() -> &'static SqlitePool {
    DB_POOL.get().expect("Database not initialized")
}

/// Test-only: install a pool into the global slot without an AppHandle.
/// Lets service-layer integration tests (oauth_upstream flow E2E, oauth
/// provider tests) exercise the real DB-backed stores. Idempotent: multiple
/// test modules each run their own Once-init; the first one wins and later
/// initializations are silently ignored (a test binary runs one global DB).
#[cfg(test)]
pub fn set_pool_for_test(pool: SqlitePool) {
    let _ = DB_POOL.set(pool);
}

/// Test-only shared init: ONE in-memory pool, migrated ONCE, installed once.
/// Each test module used to own its own init (fresh pool + global replace);
/// under parallel `cargo test` the second replace pointed the global slot at
/// a pool other tests were mid-migrating, producing "no such table" races.
/// All test modules must call this instead of rolling their own init.
#[cfg(test)]
pub async fn ensure_test_pool() {
    static INIT: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
    INIT.get_or_init(|| async {
        // A file-backed DB, NOT `sqlite::memory:`: each #[tokio::test] owns
        // its own runtime, and when that runtime drops the pooled connection
        // dies with it — the pool then opens a FRESH EMPTY :memory: database
        // for the next test ("no such table" races only in the full parallel
        // suite). A file survives connection recycling. Deleted first so
        // every test process starts from a clean migrated schema.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-pool.sqlite");
        for p in [path.clone(),
                  path.with_extension("sqlite-wal"),
                  path.with_extension("sqlite-shm")] {
            let _ = std::fs::remove_file(&p);
        }
        let url = format!("sqlite://{}?mode=rwc", path.display());
        // Mirror the app pool options (WAL + busy_timeout + bounded pool):
        // default DELETE-journal options under parallel tests produced
        // cross-connection statement-cache staleness (sqlx reused column
        // metadata captured before an ALTER, panicking in SqliteRow::current).
        let opts = SqliteConnectOptions::from_str(&url)
            .expect("parse test db url")
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_millis(5000));
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await
            .expect("connect test sqlite");
        migration::run_pending(&pool)
            .await
            .expect("run migrations on test pool");
        set_pool_for_test(pool);
    })
    .await;
}

pub async fn initialize(app: &AppHandle) -> Result<()> {
    let app_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| anyhow::anyhow!("Failed to resolve app data dir: {e}"))?;

    std::fs::create_dir_all(&app_dir)?;
    let db_path = app_dir.join("mcphub.db");
    let db_url = format!("sqlite://{}?mode=rwc", db_path.display());

    // Foreign keys are intentionally OFF. Cleanup of child rows (e.g.
    // skill_exports when a skill is deleted) is done explicitly in code via
    // transactions (more flexible than ON DELETE CASCADE — lets us pick what
    // to clean per operation).
    //
    // PRAGMA tuning on every pooled connection:
    //   - journal_mode=WAL: lets readers proceed concurrently with a writer
    //     (default "delete" rollback journal is single-writer and serializes
    //     all access, which caused SQLITE_BUSY under RAG + logging contention).
    //   - busy_timeout=5000ms: instead of immediately returning "database is
    //     locked" (code 5) when another writer holds the lock, wait up to 5s.
    //     This was the root cause of `upsert_doc_sql` failing when the user
    //     added RAG tags while app_log/activity_log writes were in flight,
    //     leaving the rag_tags mirror empty.
    let options = SqliteConnectOptions::from_str(&db_url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_millis(5000));
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    // Backup database before applying pending migrations (pre-migration copy
    // for manual rollback, requirement: mcphub.db.bak). Only runs when
    // migrations are pending; failure aborts startup — never migrate without
    // a backup. See db/migration.rs::backup_before_migration.
    migration::backup_before_migration(&pool, &db_path).await?;

    // Run version-wise migrations
    migration::run_pending(&pool).await?;
    // Unconditional repairs for DBs stamped current by an intermediate build
    // whose DDL was later amended (idempotent, cheap).
    migration::repair_schema(&pool).await?;

    DB_POOL
        .set(pool)
        .map_err(|_| anyhow::anyhow!("db pool already initialized"))?;
    log::info!("Database initialized at {}", db_path.display());
    Ok(())
}
