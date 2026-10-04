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

/// Issue a JWT token for the given user
pub fn issue_token(user_id: &str, username: &str, role: &str) -> Result<AuthToken> {
    let now = Utc::now();
    let exp = now + Duration::hours(TOKEN_EXPIRY_HOURS);

    let claims = Claims {
        sub: user_id.to_string(),
        username: username.to_string(),
        role: role.to_string(),
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
    issue_token("guest", "guest", "guest")
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
