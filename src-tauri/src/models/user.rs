use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    Admin,
    User,
}

impl Default for UserRole {
    fn default() -> Self {
        Self::User
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub username: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub role: UserRole,
    pub created_at: String,
    pub updated_at: String,
}

/// Safe user info for API responses (no password hash)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInfo {
    pub id: String,
    pub username: String,
    pub role: UserRole,
    pub created_at: String,
}

impl From<User> for UserInfo {
    fn from(u: User) -> Self {
        Self {
            id: u.id,
            username: u.username,
            role: u.role,
            created_at: u.created_at,
        }
    }
}

/// Payload for creating / updating a user
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserPayload {
    pub username: String,
    pub password: String,
    pub role: Option<UserRole>,
}
