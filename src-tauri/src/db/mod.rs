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

    // Run embedded migrations
    sqlx::migrate!("./migrations").run(&pool).await?;

    // Ensure a default admin user exists (admin / 123456)
    let admin_hash = "$2b$10$68DpNRgEB4V88lMXDK46J.ahxYKObFIUnuff5x2oxkhtaWt2dMUO6";
    sqlx::query(
        "INSERT OR IGNORE INTO users (id, username, password_hash, role, created_at, updated_at) \
         SELECT 'admin-default', 'admin', ?, 'admin', datetime('now'), datetime('now') \
         WHERE NOT EXISTS (SELECT 1 FROM users WHERE username = 'admin')",
    )
    .bind(admin_hash)
    .execute(&pool)
    .await?;

    DB_POOL.set(pool).ok();
    log::info!("Database initialized at {}", db_path.display());
    Ok(())
}
