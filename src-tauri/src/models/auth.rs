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
    pub exp: usize,
    pub iat: usize,
}
