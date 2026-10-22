use crate::models::auth::{AuthToken, Claims};
use anyhow::{anyhow, Result};
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use std::sync::OnceLock;

/// In-memory JWT secret (per-process random, generated at startup; session tokens only — no keychain persistence)
static JWT_SECRET: OnceLock<String> = OnceLock::new();

const TOKEN_EXPIRY_HOURS: i64 = 24;

pub fn init_secret(secret: String) {
    JWT_SECRET.set(secret).ok();
}

fn secret() -> &'static str {
    // Fallback when `init_secret` was never called (no persistence in
    // the current startup path — verified: zero call sites). A STATIC fallback
    // string would sign every token with a publicly-known secret (it ships in
    // the open-source repo), letting anyone with the source forge tokens.
    // Session tokens live only in the in-memory SessionState (they never
    // survive a restart anyway), so a fresh random secret per process is the
    // correct fallback — strict improvement over the static string.
    JWT_SECRET.get_or_init(|| {
        // 2× uuid v4 (getrandom-backed) = 244 bits of entropy — session-scoped
        // JWT signing key, sufficient for tokens that live ≤24h in memory.
        format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        )
    })
}

/// Credential version bound into tokens: sha256(`username\0password_hash`).
/// Salted password hashes change on password reset and account recreation,
/// so any token minted from earlier credentials stops verifying.
fn session_version_for(username: &str, password_hash: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(username.as_bytes());
    hasher.update([0u8]);
    hasher.update(password_hash.as_bytes());
    hex::encode(hasher.finalize())
}

/// Issue a JWT token for the given user
pub fn issue_token(user_id: &str, username: &str, role: &str, password_hash: &str) -> Result<AuthToken> {
    let now = Utc::now();
    let exp = now + Duration::hours(TOKEN_EXPIRY_HOURS);

    let claims = Claims {
        sub: user_id.to_string(),
        username: username.to_string(),
        role: role.to_string(),
        session_version: session_version_for(username, password_hash),
        exp: exp.timestamp() as usize,
        iat: now.timestamp() as usize,
    };

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret().as_bytes()),
    )
    .map_err(|e| anyhow!("Token encoding failed: {}", e))?;

    Ok(AuthToken {
        token,
        expires_at: exp.to_rfc3339(),
        user_id: user_id.to_string(),
        username: username.to_string(),
        role: role.to_string(),
    })
}

/// Issue a guest token with limited admin privileges
pub fn issue_guest_token() -> Result<AuthToken> {
    issue_token("guest", "guest", "guest", "")
}

/// Constant-time equality over byte strings (avoids short-circuit leaks).
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Validate a JWT token AND re-check it against the live user row: the
/// session_version claim must match the user's current credential version.
/// Tokens for deleted users, and tokens minted before a password change,
/// are rejected here (origin #4d1e84a parity). Guest tokens (no DB row by
/// design) pass signature validation only — role gates handle them.
pub async fn verify_token_fresh(token: &str) -> Result<Claims> {
    let claims = verify_token(token)?;
    if claims.role == "guest" {
        return Ok(claims);
    }
    let user = crate::services::user_service::find_by_username(&claims.username)
        .await
        .map_err(|e| anyhow!("user lookup failed: {e}"))?
        .ok_or_else(|| anyhow!("Token is not valid"))?;
    let expected = session_version_for(&user.username, &user.password_hash);
    if !constant_time_eq(&claims.session_version, &expected) {
        return Err(anyhow!("Token is not valid"));
    }
    Ok(claims)
}

/// Validate a JWT token and return the claims
pub fn verify_token(token: &str) -> Result<Claims> {
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret().as_bytes()),
        &Validation::default(),
    )
    .map_err(|e| anyhow!("Token validation failed: {}", e))?;

    Ok(token_data.claims)
}

/// Hash a password using bcrypt
pub fn hash_password(password: &str) -> Result<String> {
    bcrypt::hash(password, bcrypt::DEFAULT_COST)
        .map_err(|e| anyhow!("Password hashing failed: {}", e))
}

/// Verify a password against its bcrypt hash
pub fn verify_password(password: &str, hash: &str) -> Result<bool> {
    bcrypt::verify(password, hash).map_err(|e| anyhow!("Password verification failed: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The session_version claim must change when the credential changes, so
    /// a token minted before a password reset fails verify_token_fresh
    /// (origin #4d1e84a parity). Uses the pure version function — the DB
    /// round-trip is covered by the wired call sites.
    #[test]
    fn session_version_changes_with_credential() {
        let v1 = session_version_for("alice", "hash-one");
        let v2 = session_version_for("alice", "hash-two");
        let other = session_version_for("bob", "hash-one");
        assert_ne!(v1, v2, "password change must rotate the session version");
        assert_ne!(v1, other, "same password for a different user must differ");
        // Deterministic (same inputs → same version) and 64 hex chars.
        assert_eq!(v1, session_version_for("alice", "hash-one"));
        assert_eq!(v1.len(), 64);
    }

    #[tokio::test]
    async fn fresh_verify_rejects_stale_and_deleted_users() {
        crate::db::ensure_test_pool().await;
        let username = format!("sv_user_{}", uuid::Uuid::new_v4().simple());
        crate::services::user_service::create(&crate::models::user::UserPayload {
            username: username.clone(),
            password: "correct horse".to_string(),
            role: Some(crate::models::user::UserRole::Admin),
        })
        .await
        .unwrap();
        let user = crate::services::user_service::find_by_username(&username)
            .await
            .unwrap()
            .unwrap();
        let token = issue_token(&user.id, &user.username, "admin", &user.password_hash).unwrap();
        // Current credential → valid.
        assert!(verify_token_fresh(&token.token).await.is_ok());
        // Password changed after mint → the old token must stop verifying.
        crate::services::user_service::update_password(&user.id, "new pass")
            .await
            .unwrap();
        assert!(verify_token_fresh(&token.token).await.is_err());
        // Deleted user → rejected outright.
        crate::services::user_service::delete_by_username(&username)
            .await
            .unwrap();
        assert!(verify_token_fresh(&token.token).await.is_err());
    }

    #[test]
    fn constant_time_eq_cases() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(constant_time_eq("", ""));
        assert!(!constant_time_eq("abc", "abd"));
        // Different lengths must never panic / never compare equal.
        assert!(!constant_time_eq("abc", "abcd"));
        assert!(!constant_time_eq("", "a"));
    }

    #[tokio::test]
    async fn guest_token_verifies_fresh_without_db_row() {
        // Guest tokens have no user row by design — verify_token_fresh must
        // short-circuit on role instead of failing the DB lookup.
        let token = issue_guest_token().unwrap();
        let claims = verify_token_fresh(&token.token).await.unwrap();
        assert_eq!(claims.role, "guest");
    }

    #[test]
    fn verify_token_rejects_garbage() {
        assert!(verify_token("not-a-jwt").is_err());
        assert!(verify_token("").is_err());
        // Structurally valid JWT signed with a different secret.
        let other = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &serde_json::json!({"sub": "x", "username": "x", "role": "admin"}),
            &jsonwebtoken::EncodingKey::from_secret(b"wrong-secret"),
        )
        .unwrap();
        assert!(verify_token(&other).is_err());
    }

    #[test]
    fn password_hash_round_trip() {
        let hash = hash_password("s3cret pass").unwrap();
        assert_ne!(hash, "s3cret pass");
        assert!(verify_password("s3cret pass", &hash).unwrap());
        assert!(!verify_password("wrong", &hash).unwrap());
        assert!(verify_password("x", "not-a-hash").is_err());
    }

    #[test]
    fn init_secret_sets_process_secret() {
        // init_secret is the app-startup path (lib setup); tests otherwise
        // exercise the random per-process fallback. JWT_SECRET is a
        // process-wide OnceLock: whichever init wins (this call or a
        // parallel test's lazy fallback), mint + verify must stay consistent.
        init_secret(uuid::Uuid::new_v4().to_string());
        assert!(!secret().is_empty());
        let token = issue_token("uid-init", "init_user", "admin", "h").unwrap();
        assert!(verify_token(&token.token).is_ok());
    }
}
