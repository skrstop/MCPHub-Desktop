pub mod migration;

use anyhow::Result;
use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};
use std::sync::OnceLock;
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
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await?;

    // Run version-wise migrations
    migration::run_pending(&pool).await?;

    DB_POOL.set(pool).ok();
    log::info!("Database initialized at {}", db_path.display());
    Ok(())
}
