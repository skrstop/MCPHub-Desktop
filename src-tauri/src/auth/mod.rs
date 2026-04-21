use crate::models::auth::{AuthToken, Claims};
use anyhow::{anyhow, Result};
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use std::sync::OnceLock;

/// In-memory JWT secret (generated once at startup, stored in OS keychain on next launch)
static JWT_SECRET: OnceLock<String> = OnceLock::new();

const TOKEN_EXPIRY_HOURS: i64 = 24;

pub fn init_secret(secret: String) {
    JWT_SECRET.set(secret).ok();
}

fn secret() -> &'static str {
    JWT_SECRET.get().map(|s| s.as_str()).unwrap_or("mcphub-default-dev-secret-change-in-prod")
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
