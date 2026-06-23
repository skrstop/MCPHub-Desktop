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
        .expect("Failed to resolve app data dir");

    std::fs::create_dir_all(&app_dir)?;
    let db_path = app_dir.join("mcphub.db");
    let db_url = format!("sqlite://{}?mode=rwc", db_path.display());

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await?;

    // Run version-aware migrations
    migration::run_pending(&pool).await?;

    DB_POOL.set(pool).ok();
    log::info!("Database initialized at {}", db_path.display());
    Ok(())
}
