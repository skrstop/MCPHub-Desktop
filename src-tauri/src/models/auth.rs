use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthToken {
    pub token: String,
    pub expires_at: String,
    pub user_id: String,
    pub username: String,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Claims {
    pub sub: String,   // user id
    pub username: String,
    pub role: String,
    /// Credential version at issue time (sha256 of `username\0password_hash`).
    /// Re-checked against the live user row on every authorization decision —
    /// a token minted before a password change / user deletion is rejected
    /// (origin #4d1e84a parity). Guest tokens carry an empty version.
    #[serde(default)]
    pub session_version: String,
    pub exp: usize,
    pub iat: usize,
}
