use crate::{
    auth,
    db,
    models::user::{User, UserInfo, UserPayload, UserRole},
};
use anyhow::Result;
use chrono::Local;
use sqlx::Row;
use uuid::Uuid;

/// Usernames reserved for system use (case-insensitive).
const RESERVED_USERNAMES: &[&str] = &["system", "admin", "guest", "root"];

/// Check if a username is reserved for system use.
/// Returns the reason string if reserved, or null if allowed.
fn check_reserved_username(username: &str) -> Option<String> {
    let lower = username.to_lowercase();
    if RESERVED_USERNAMES.contains(&lower.as_str()) {
        Some(format!("Username '{}' is reserved and cannot be used", username))
    } else {
        None
    }
}

fn role_from_str(s: &str) -> UserRole {
    if s == "admin" { UserRole::Admin } else { UserRole::User }
}

pub async fn find_by_username(username: &str) -> Result<Option<User>> {
    let row = sqlx::query(
        "SELECT id, username, password_hash, role, created_at, updated_at FROM users WHERE username = ?",
    )
    .bind(username)
    .fetch_optional(db::pool())
    .await?;

    match row {
        None => Ok(None),
        Some(r) => Ok(Some(User {
            id: r.try_get("id")?,
            username: r.try_get("username")?,
            password_hash: r.try_get("password_hash")?,
            role: role_from_str(r.try_get("role")?),
            created_at: r.try_get("created_at")?,
            updated_at: r.try_get("updated_at")?,
        })),
    }
}

pub async fn list_all() -> Result<Vec<UserInfo>> {
    let rows = sqlx::query(
        "SELECT id, username, role, created_at FROM users ORDER BY username",
    )
    .fetch_all(db::pool())
    .await?;

    rows.into_iter()
        .map(|r| {
            Ok(UserInfo {
                id: r.try_get("id")?,
                username: r.try_get("username")?,
                role: role_from_str(r.try_get("role")?),
                created_at: r.try_get("created_at")?,
            })
        })
        .collect()
}

pub async fn create(payload: &UserPayload) -> Result<UserInfo> {
    // Check reserved usernames
    if let Some(reason) = check_reserved_username(&payload.username) {
        log::warn!("User creation blocked: {}", reason);
        return Err(anyhow::anyhow!(reason));
    }

    let id = Uuid::new_v4().to_string();
    let hash = auth::hash_password(&payload.password)?;
    let role = match payload.role {
        Some(UserRole::Admin) => "admin",
        _ => "user",
    };
    let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

    sqlx::query(
        "INSERT INTO users (id, username, password_hash, role, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&payload.username)
    .bind(&hash)
    .bind(role)
    .bind(&now)
    .bind(&now)
    .execute(db::pool())
    .await?;

    Ok(UserInfo {
        id,
        username: payload.username.clone(),
        role: payload.role.clone().unwrap_or_default(),
        created_at: now,
    })
}

pub async fn update_password(user_id: &str, new_password: &str) -> Result<()> {
    let hash = auth::hash_password(new_password)?;
    let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    sqlx::query(
        "UPDATE users SET password_hash=?, updated_at=? WHERE id=?",
    )
    .bind(&hash)
    .bind(&now)
    .bind(user_id)
    .execute(db::pool())
    .await?;
    Ok(())
}

pub async fn delete(user_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM users WHERE id=?")
        .bind(user_id)
        .execute(db::pool())
        .await?;
    Ok(())
}

/// Update a user by username: optionally change role and/or password.
pub async fn update_by_username(
    username: &str,
    is_admin: Option<bool>,
    new_password: Option<&str>,
) -> Result<UserInfo> {
    let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    if let Some(admin) = is_admin {
        let role = if admin { "admin" } else { "user" };
        sqlx::query(
            "UPDATE users SET role=?, updated_at=? WHERE username=?",
        )
        .bind(role)
        .bind(&now)
        .bind(username)
        .execute(db::pool())
        .await?;
    }
    if let Some(pw) = new_password {
        let hash = auth::hash_password(pw)?;
        sqlx::query(
            "UPDATE users SET password_hash=?, updated_at=? WHERE username=?",
        )
        .bind(&hash)
        .bind(&now)
        .bind(username)
        .execute(db::pool())
        .await?;
    }
    let user = find_by_username(username)
        .await?
        .ok_or_else(|| anyhow::anyhow!("User '{}' not found", username))?;
    Ok(UserInfo::from(user))
}

/// Delete a user by username.
pub async fn delete_by_username(username: &str) -> Result<()> {
    sqlx::query("DELETE FROM users WHERE username=?")
        .bind(username)
        .execute(db::pool())
        .await?;
    Ok(())
}

/// Seed a default admin account if no users exist
pub async fn ensure_default_admin() -> Result<()> {
    let row = sqlx::query("SELECT COUNT(*) as cnt FROM users")
        .fetch_one(db::pool())
        .await?;
    let count: i64 = row.try_get("cnt")?;

    if count == 0 {
        let payload = UserPayload {
            username: "admin".to_string(),
            password: "admin".to_string(),
            role: Some(UserRole::Admin),
        };
        create(&payload).await?;
        log::info!("Default admin account created (username: admin, password: admin)");
    }
    Ok(())
}

