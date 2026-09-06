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

    DB_POOL.set(pool).ok();
    log::info!("Database initialized at {}", db_path.display());
    Ok(())
}
