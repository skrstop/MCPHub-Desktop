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
    // Reject empty usernames (parity with the password check below; review
    // round 10).
    if payload.username.trim().is_empty() {
        return Err(anyhow::anyhow!("Username must not be empty"));
    }
    // Check reserved usernames
    if let Some(reason) = check_reserved_username(&payload.username) {
        log::warn!("User creation blocked: {}", reason);
        return Err(anyhow::anyhow!(reason));
    }

    // Reject empty/whitespace-only passwords (settings_import feeds external
    // JSON here; an empty hash would create a lock-out-able account).
    if payload.password.trim().is_empty() {
        return Err(anyhow::anyhow!("Password must not be empty"));
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
    if new_password.trim().is_empty() {
        return Err(anyhow::anyhow!("Password must not be empty"));
    }
    let hash = auth::hash_password(new_password)?;
    let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let affected = sqlx::query(
        "UPDATE users SET password_hash=?, updated_at=? WHERE id=?",
    )
    .bind(&hash)
    .bind(&now)
    .bind(user_id)
    .execute(db::pool())
    .await?
    .rows_affected();
    // 0 rows = unknown user: report instead of silently succeeding (review
    // round 10).
    if affected == 0 {
        return Err(anyhow::anyhow!("User not found"));
    }
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
    // Validate BEFORE any write: previously the role UPDATE committed first
    // and an empty-password rejection afterwards left a half-applied change
    // (review round 10).
    if let Some(pw) = new_password {
        if pw.trim().is_empty() {
            return Err(anyhow::anyhow!("password must not be empty"));
        }
    }
    let hash = match new_password {
        Some(pw) => Some(auth::hash_password(pw)?),
        None => None,
    };
    // BEGIN IMMEDIATE: the last-admin guard read and the writes must share
    // the write lock, or two concurrent demotes can both pass the guard and
    // leave zero admins (review round 10 TOCTOU fix).
    let mut tx = db::pool().begin().await?;
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *tx).await?;
    if let Some(admin) = is_admin {
        // Last-admin guard (parity with delete_by_username): demoting the
        // final admin leaves user management unreachable until a restart
        // reseeds the default credential (review round 8, 2026-10-04).
        if !admin {
            let target_role: Option<String> = sqlx::query_scalar(
                "SELECT role FROM users WHERE username = ?",
            )
            .bind(username)
            .fetch_optional(&mut *tx)
            .await?;
            if target_role.as_deref() == Some("admin") {
                let admins: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM users WHERE role = 'admin'",
                )
                .fetch_one(&mut *tx)
                .await?;
                if admins <= 1 {
                    return Err(anyhow::anyhow!("cannot demote the last admin account"));
                }
            }
        }
        let role = if admin { "admin" } else { "user" };
        sqlx::query(
            "UPDATE users SET role=?, updated_at=? WHERE username=?",
        )
        .bind(role)
        .bind(&now)
        .bind(username)
        .execute(&mut *tx)
        .await?;
    }
    if let Some(hash) = &hash {
        sqlx::query(
            "UPDATE users SET password_hash=?, updated_at=? WHERE username=?",
        )
        .bind(hash)
        .bind(&now)
        .bind(username)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    let user = find_by_username(username)
        .await?
        .ok_or_else(|| anyhow::anyhow!("User '{}' not found", username))?;
    Ok(UserInfo::from(user))
}

/// Delete a user by username.
pub async fn delete_by_username(username: &str) -> Result<()> {
    // BEGIN IMMEDIATE: guard read + delete share the write lock (review
    // round 10 TOCTOU fix, same as update_by_username).
    let mut tx = db::pool().begin().await?;
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *tx).await?;
    // Last-admin guard: deleting the final admin leaves user management
    // unreachable until a restart reseeds the KNOWN default credential
    // (admin/admin) on a machine that may expose the HTTP server.
    let target_role: Option<String> = sqlx::query_scalar(
        "SELECT role FROM users WHERE username = ?",
    )
    .bind(username)
    .fetch_optional(&mut *tx)
    .await?;
    if target_role.as_deref() == Some("admin") {
        let admins: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role = 'admin'")
                .fetch_one(&mut *tx)
                .await?;
        if admins <= 1 {
            return Err(anyhow::anyhow!("cannot delete the last admin account"));
        }
    }
    sqlx::query("DELETE FROM users WHERE username=?")
        .bind(username)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Seed a default admin account if no users exist.
/// Inserts directly (not via `create()`): "admin" is a reserved username for
/// user-facing creation, and the reserved check would otherwise deadlock the
/// very first login on a fresh install (review round 9, 2026-10-04).
pub async fn ensure_default_admin() -> Result<()> {
    let row = sqlx::query("SELECT COUNT(*) as cnt FROM users")
        .fetch_one(db::pool())
        .await?;
    let count: i64 = row.try_get("cnt")?;

    if count == 0 {
        let id = Uuid::new_v4().to_string();
        let hash = auth::hash_password("admin")?;
        let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        sqlx::query(
            "INSERT INTO users (id, username, password_hash, role, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind("admin")
        .bind(&hash)
        .bind("admin")
        .bind(&now)
        .bind(&now)
        .execute(db::pool())
        .await?;
        log::info!("Default admin account created (username: admin, password: admin)");
    }
    Ok(())
}

