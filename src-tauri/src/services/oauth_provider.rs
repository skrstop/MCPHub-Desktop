//! OAuth Provider: MCPHub as an OAuth 2.1 authorization server for third-party
//! AI clients (Claude etc.) reaching `/mcp` and `/mcp/$smart`.
//!
//! Only active when the built-in HTTP server is exposed AND `oauthServer.enabled`
//! is set in system config (deep-merged by the frontend's /system-config PUT).
//! Endpoints live in http_server.rs; this module owns storage + token logic.
//!
//! Token/secret hygiene: only SHA-256 hashes are stored (unlike bearer keys,
//! a documented legacy debt).

use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::collections::HashSet;

use crate::db::pool;

// ────────────────────────────────────────────────────────────────────────────
// Config
// ────────────────────────────────────────────────────────────────────────────

/// Provider settings read from the system config JSON (`oauthServer` key —
/// same shape the frontend SettingsContext saves). `None` = provider disabled.
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub enabled: bool,
    pub access_token_lifetime: i64,
    pub refresh_token_lifetime: i64,
    pub authorization_code_lifetime: i64,
    pub dynamic_registration: bool,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            access_token_lifetime: 3600,
            refresh_token_lifetime: 30 * 24 * 3600,
            authorization_code_lifetime: 60,
            dynamic_registration: true,
        }
    }
}

pub async fn provider_config() -> ProviderConfig {
    let cfg = ProviderConfig::default();
    let Ok(json) = crate::services::config_service::get().await else {
        return cfg;
    };
    let Some(o) = json.get("oauthServer") else {
        return cfg;
    };
    let lifetime = |key: &str, fallback: i64| -> i64 {
        o.get(key).and_then(|v| v.as_i64()).filter(|v| *v > 0).unwrap_or(fallback)
    };
    ProviderConfig {
        enabled: o.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false),
        access_token_lifetime: lifetime("accessTokenLifetime", cfg.access_token_lifetime),
        refresh_token_lifetime: lifetime("refreshTokenLifetime", cfg.refresh_token_lifetime),
        authorization_code_lifetime: lifetime(
            "authorizationCodeLifetime",
            cfg.authorization_code_lifetime,
        ),
        dynamic_registration: o
            .get("dynamicRegistration")
            .and_then(|d| d.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Hashing / token generation
// ────────────────────────────────────────────────────────────────────────────

pub(crate) fn sha256_hex(input: &str) -> String {
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    hex::encode(h.finalize())
}

/// Two uuid v4s (getrandom-backed, repo precedent) = 256 bits of entropy.
pub(crate) fn random_token() -> String {
    uuid::Uuid::new_v4().simple().to_string() + &uuid::Uuid::new_v4().simple().to_string()
}

/// PKCE S256: BASE64URL(SHA256(verifier)).
pub(crate) fn pkce_s256(verifier: &str) -> String {
    let mut h = Sha256::new();
    h.update(verifier.as_bytes());
    base64url_no_pad(&h.finalize())
}

pub(crate) fn base64url_no_pad(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

// ────────────────────────────────────────────────────────────────────────────
// Clients (RFC 7591 dynamic registration + manual revoke)
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OAuthClient {
    pub client_id: String,
    pub client_secret_hash: Option<String>,
    pub name: String,
    pub redirect_uris: String, // JSON array
    pub scopes: String,        // space separated
    pub created_at: i64,
    pub revoked: bool,
}

pub async fn register_client(
    name: &str,
    redirect_uris: &[String],
    scopes: &str,
    with_secret: bool,
) -> Result<(OAuthClient, Option<String>), String> {
    if redirect_uris.is_empty() {
        return Err("redirect_uris is required".into());
    }
    for uri in redirect_uris {
        // Exact host check (never string prefix — "http://localhost.evil.com"
        // must not pass). Loopback hosts allow http; everything else needs https.
        let parsed = match url::Url::parse(uri) {
            Ok(u) => u,
            Err(_) => return Err(format!("redirect_uri is not a valid URL: {uri}")),
        };
        let host = parsed.host_str().unwrap_or_default();
        let loopback = matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]");
        match parsed.scheme() {
            "http" if loopback => {}
            "https" => {}
            _ => return Err(format!("redirect_uri scheme not allowed: {uri}")),
        }
        if parsed.path().contains('\\') || uri.contains('\r') || uri.contains('\n') {
            return Err(format!("redirect_uri contains illegal characters: {uri}"));
        }
    }
    let client_id = format!("mcp-{}", random_token());
    let secret = with_secret.then(random_token);
    let now = now_epoch();
    sqlx::query(
        "INSERT INTO oauth_clients (client_id, client_secret_hash, name, redirect_uris, scopes, created_at) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&client_id)
    .bind(secret.as_deref().map(sha256_hex))
    .bind(name)
    .bind(&serde_json::to_string(redirect_uris).unwrap_or_else(|_| "[]".into()))
    .bind(scopes)
    .bind(now)
    .execute(pool())
    .await
    .map_err(|e| e.to_string())?;
    let client = find_client(&client_id)
        .await?
        .ok_or_else(|| "client vanished after insert".to_string())?;
    Ok((client, secret))
}

pub async fn find_client(client_id: &str) -> Result<Option<OAuthClient>, String> {
    sqlx::query_as::<_, OAuthClient>(
        "SELECT client_id, client_secret_hash, name, redirect_uris, scopes, created_at, revoked \
         FROM oauth_clients WHERE client_id = ?",
    )
    .bind(client_id)
    .fetch_optional(pool())
    .await
    .map_err(|e| e.to_string())
}

pub async fn list_clients() -> Result<Vec<OAuthClient>, String> {
    sqlx::query_as::<_, OAuthClient>(
        "SELECT client_id, client_secret_hash, name, redirect_uris, scopes, created_at, revoked \
         FROM oauth_clients ORDER BY created_at DESC",
    )
    .fetch_all(pool())
    .await
    .map_err(|e| e.to_string())
}

/// Revoke: mark the client revoked and drop all its tokens/codes.
pub async fn revoke_client(client_id: &str) -> Result<bool, String> {
    let res = sqlx::query("UPDATE oauth_clients SET revoked = 1 WHERE client_id = ?")
        .bind(client_id)
        .execute(pool())
        .await
        .map_err(|e| e.to_string())?;
    if res.rows_affected() == 0 {
        return Ok(false);
    }
    let _ = sqlx::query("DELETE FROM oauth_tokens WHERE client_id = ?")
        .bind(client_id)
        .execute(pool())
        .await;
    let _ = sqlx::query("DELETE FROM oauth_codes WHERE client_id = ?")
        .bind(client_id)
        .execute(pool())
        .await;
    Ok(true)
}

/// Validate a client for the authorize/token endpoints. `secret` is checked
/// only when the stored hash exists (public clients skip it).
pub async fn validate_client(client_id: &str, secret: Option<&str>) -> Result<OAuthClient, String> {
    let client = find_client(client_id)
        .await?
        .ok_or_else(|| "unknown client_id".to_string())?;
    if client.revoked {
        return Err("client is revoked".into());
    }
    if let Some(hash) = client.client_secret_hash.as_deref() {
        let provided = secret.unwrap_or("");
        // Constant-time comparison on the hash of the provided secret.
        let provided_hash = sha256_hex(provided);
        if !constant_time_eq(provided_hash.as_bytes(), hash.as_bytes()) {
            return Err("client_secret mismatch".into());
        }
    }
    Ok(client)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub fn client_redirect_uris(client: &OAuthClient) -> Vec<String> {
    serde_json::from_str(&client.redirect_uris).unwrap_or_default()
}

// ────────────────────────────────────────────────────────────────────────────
// Authorization codes (single use, PKCE-bound, short TTL)
// ────────────────────────────────────────────────────────────────────────────

pub async fn store_code(
    client_id: &str,
    challenge: &str,
    scope: &str,
    resource: &str,
    redirect_uri: &str,
    ttl: i64,
) -> Result<String, String> {
    let code = random_token();
    sqlx::query(
        "INSERT INTO oauth_codes (code_hash, client_id, challenge, challenge_method, scope, resource, redirect_uri, expires_at) \
         VALUES (?, ?, ?, 'S256', ?, ?, ?, ?)",
    )
    .bind(sha256_hex(&code))
    .bind(client_id)
    .bind(challenge)
    .bind(scope)
    .bind(resource)
    .bind(redirect_uri)
    .bind(now_epoch() + ttl)
    .execute(pool())
    .await
    .map_err(|e| e.to_string())?;
    Ok(code)
}

pub struct ConsumedCode {
    pub client_id: String,
    pub challenge: String,
    pub scope: String,
    pub resource: String,
    pub redirect_uri: String,
}

/// Single-use consume: the row is deleted before validation results are used
/// (replay of even a failed exchange is impossible).
pub async fn consume_code(code: &str) -> Result<Option<ConsumedCode>, String> {
    let hash = sha256_hex(code);
    let row = sqlx::query_as::<_, (String, String, String, String, String, i64)>(
        "SELECT client_id, challenge, scope, resource, redirect_uri, expires_at \
         FROM oauth_codes WHERE code_hash = ?",
    )
    .bind(&hash)
    .fetch_optional(pool())
    .await
    .map_err(|e| e.to_string())?;
    let _ = sqlx::query("DELETE FROM oauth_codes WHERE code_hash = ?")
        .bind(&hash)
        .execute(pool())
        .await;
    let Some((client_id, challenge, scope, resource, redirect_uri, expires_at)) = row else {
        return Ok(None);
    };
    if now_epoch() > expires_at {
        return Err("authorization code expired".into());
    }
    Ok(Some(ConsumedCode { client_id, challenge, scope, resource, redirect_uri }))
}

// ────────────────────────────────────────────────────────────────────────────
// Tokens
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct IssuedTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
}

/// Issue a fresh access+refresh pair for a client (authorization_code grant).
pub async fn issue_tokens(
    client_id: &str,
    scope: &str,
    resource: &str,
    cfg: &ProviderConfig,
) -> Result<IssuedTokens, String> {
    let access = random_token();
    let refresh = random_token();
    let now = now_epoch();
    let mut tx = pool().begin().await.map_err(|e| e.to_string())?;
    sqlx::query(
        "INSERT INTO oauth_tokens (token_hash, kind, client_id, scope, resource, expires_at, refresh_expires_at) \
         VALUES (?, 'access', ?, ?, ?, ?, 0)",
    )
    .bind(sha256_hex(&access))
    .bind(client_id)
    .bind(scope)
    .bind(resource)
    .bind(now + cfg.access_token_lifetime)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    sqlx::query(
        "INSERT INTO oauth_tokens (token_hash, kind, client_id, scope, resource, expires_at, refresh_expires_at) \
         VALUES (?, 'refresh', ?, ?, ?, ?, ?)",
    )
    .bind(sha256_hex(&refresh))
    .bind(client_id)
    .bind(scope)
    .bind(resource)
    .bind(now + cfg.refresh_token_lifetime)
    .bind(now + cfg.refresh_token_lifetime)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(IssuedTokens { access_token: access, refresh_token: refresh, expires_in: cfg.access_token_lifetime })
}

/// Refresh grant: consume the refresh token (rotate it), return new pair.
/// Reuse detection (RFC 6749 §10.4 / oauth-security-topics §4.13.2): a replayed
/// (already-rotated) refresh token is a theft signal — the whole client's
/// token family is revoked before invalid_grant is returned.
pub async fn refresh_tokens(
    refresh_token: &str,
    cfg: &ProviderConfig,
) -> Result<Option<(String, String, IssuedTokens)>, String> {
    let hash = sha256_hex(refresh_token);
    // Replay check FIRST: a consumed (rotated) token must trigger family
    // revocation, not a silent unknown-token answer.
    let replay: Option<String> = sqlx::query_scalar(
        "SELECT client_id FROM oauth_tokens WHERE token_hash = ? AND kind = 'refresh_used'",
    )
    .bind(&hash)
    .fetch_optional(pool())
    .await
    .map_err(|e| e.to_string())?;
    if let Some(client_id) = replay {
        let _ = sqlx::query("DELETE FROM oauth_tokens WHERE client_id = ?")
            .bind(&client_id)
            .execute(pool())
            .await;
        return Err("refresh token reuse detected — all tokens for this client were revoked".into());
    }
    let row = sqlx::query_as::<_, (String, String, String, i64)>(
        "SELECT client_id, scope, resource, refresh_expires_at FROM oauth_tokens \
         WHERE token_hash = ? AND kind = 'refresh'",
    )
    .bind(&hash)
    .fetch_optional(pool())
    .await
    .map_err(|e| e.to_string())?;
    let Some((client_id, scope, resource, refresh_expires_at)) = row else {
        return Ok(None);
    };
    if now_epoch() > refresh_expires_at {
        // Expired refresh: revoke the whole family (client's tokens).
        let _ = sqlx::query("DELETE FROM oauth_tokens WHERE client_id = ?")
            .bind(&client_id)
            .execute(pool())
            .await;
        return Err("refresh token expired".into());
    }
    // Rotation: mark this refresh consumed (kept for reuse detection; dropped
    // later by cleanup_expired) + its sibling access tokens for the client.
    let _ = sqlx::query(
        "UPDATE oauth_tokens SET kind = 'refresh_used', expires_at = ? WHERE token_hash = ?",
    )
    .bind(now_epoch() + 3600)
    .bind(&hash)
    .execute(pool())
    .await;
    let _ = sqlx::query("DELETE FROM oauth_tokens WHERE client_id = ? AND kind = 'access'")
        .bind(&client_id)
        .execute(pool())
        .await;
    let client = find_client(&client_id).await?;
    if client.map(|c| c.revoked).unwrap_or(true) {
        return Err("client is revoked".into());
    }
    let issued = issue_tokens(&client_id, &scope, &resource, cfg).await?;
    Ok(Some((client_id, scope, issued)))
}

/// Validate an access token. Returns the (client_id, scope) on success.
pub async fn validate_access_token(access_token: &str) -> Result<Option<(String, String)>, String> {
    let hash = sha256_hex(access_token);
    let row = sqlx::query_as::<_, (String, String, i64)>(
        "SELECT client_id, scope, expires_at FROM oauth_tokens WHERE token_hash = ? AND kind = 'access'",
    )
    .bind(&hash)
    .fetch_optional(pool())
    .await
    .map_err(|e| e.to_string())?;
    let Some((client_id, scope, expires_at)) = row else {
        return Ok(None);
    };
    if now_epoch() > expires_at {
        return Ok(None);
    }
    let revoked = find_client(&client_id).await?.map(|c| c.revoked).unwrap_or(true);
    if revoked {
        return Ok(None);
    }
    Ok(Some((client_id, scope)))
}

/// Drop expired codes/tokens (called opportunistically from the sweeper).
pub async fn cleanup_expired() {
    let now = now_epoch();
    let _ = sqlx::query("DELETE FROM oauth_codes WHERE expires_at < ?")
        .bind(now)
        .execute(pool())
        .await;
    let _ = sqlx::query(
        "DELETE FROM oauth_tokens WHERE kind = 'access' AND expires_at < ?",
    )
    .bind(now)
    .execute(pool())
    .await;
    let _ = sqlx::query(
        "DELETE FROM oauth_tokens WHERE kind = 'refresh' AND refresh_expires_at < ?",
    )
    .bind(now)
    .execute(pool())
    .await;
    // Consumed (rotated) refresh markers live 1h past rotation for reuse
    // detection, then go.
    let _ = sqlx::query(
        "DELETE FROM oauth_tokens WHERE kind = 'refresh_used' AND expires_at < ?",
    )
    .bind(now)
    .execute(pool())
    .await;
}

// ────────────────────────────────────────────────────────────────────────────
// Scope → allowed servers mapping (reuses the bearer gating model)
// ────────────────────────────────────────────────────────────────────────────

/// Map an oauth scope to the bearer-model access restriction.
/// - `mcp:all` / empty scope → `None` (no restriction).
/// - otherwise: space-separated server/group names; group names are expanded
///   to member servers at request time so the existing allowed_servers gate
///   works unchanged downstream.
pub async fn scope_to_allowed_servers(scope: &str) -> Option<HashSet<String>> {
    let scope = scope.trim();
    if scope.is_empty() || scope.split_whitespace().any(|s| s == "mcp:all") {
        return None;
    }
    let mut allowed: HashSet<String> = HashSet::new();
    for item in scope.split_whitespace() {
        let members: Option<String> = match sqlx::query_scalar(
            "SELECT servers FROM `groups` WHERE name = ?",
        )
        .bind(item)
        .fetch_optional(pool())
        .await
        {
            Ok(m) => m,
            Err(_) => None,
        };
        if let Some(members) = members {
            if let Ok(arr) = serde_json::from_str::<serde_json::Value>(&members) {
                if let Some(list) = arr.as_array() {
                    for m in list {
                        if let Some(name) = m.as_str() {
                            allowed.insert(name.to_string());
                        } else if let Some(name) = m.get("name").and_then(|n| n.as_str()) {
                            allowed.insert(name.to_string());
                        }
                    }
                }
            }
            continue;
        }
        allowed.insert(item.to_string());
    }
    Some(allowed)
}

pub fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// Silence unused import when only some paths reference it.
#[allow(unused_imports)]
use crate::models::bearer_key::BearerKey;

#[allow(dead_code)]
fn _pool_type_witness(_p: &SqlitePool) {}

#[cfg(test)]
mod tests {
    use super::*;

    async fn init_pool() {
        static INIT: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
        INIT.get_or_init(|| async {
            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .connect("sqlite::memory:")
                .await
                .unwrap();
            crate::db::migration::run_pending(&pool).await.unwrap();
            crate::db::set_pool_for_test(pool);
        })
        .await;
    }

    #[tokio::test]
    async fn register_validate_revoke_roundtrip() {
        init_pool().await;
        let (client, secret) = register_client("Claude", &["https://claude.ai/api/mcp/callback".into()], "mcp:all", true)
            .await
            .expect("register");
        assert!(client.client_id.starts_with("mcp-"));
        assert!(secret.is_some());

        // Correct secret passes; wrong secret rejected.
        assert!(validate_client(&client.client_id, secret.as_deref()).await.is_ok());
        assert!(validate_client(&client.client_id, Some("wrong")).await.is_err());

        // Redirect scheme whitelist.
        assert!(register_client("bad", &["javascript:alert(1)".into()], "", false)
            .await
            .is_err());

        // Revoke → token validation fails.
        assert!(revoke_client(&client.client_id).await.unwrap());
        assert!(validate_client(&client.client_id, secret.as_deref()).await.is_err());
    }

    #[tokio::test]
    async fn code_and_token_lifecycle() {
        init_pool().await;
        let cfg = ProviderConfig { enabled: true, ..Default::default() };
        let (client, _) = register_client("T", &["http://127.0.0.1/cb".into()], "srv-a srv-b", false)
            .await
            .expect("register");

        // PKCE S256 round-trip (RFC 7636 appendix B vector).
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = pkce_s256(verifier);
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");

        let code = store_code(&client.client_id, &challenge, "srv-a srv-b", "", "http://127.0.0.1/cb", 60)
            .await
            .expect("store code");

        // First consume works; replay is rejected (single use).
        let consumed = consume_code(&code).await.expect("consume").expect("code exists");
        assert_eq!(consumed.challenge, challenge);
        assert!(consume_code(&code).await.expect("replay").is_none());

        // PKCE verification: correct verifier matches, wrong one does not.
        assert_eq!(pkce_s256(verifier), consumed.challenge);
        assert_ne!(pkce_s256("wrong-verifier"), consumed.challenge);

        // Issue + validate access token; refresh rotates.
        let tokens = issue_tokens(&client.client_id, "srv-a", "", &cfg).await.expect("issue");
        assert_eq!(
            validate_access_token(&tokens.access_token).await.expect("validate"),
            Some((client.client_id.clone(), "srv-a".into()))
        );
        let rotated = refresh_tokens(&tokens.refresh_token, &cfg)
            .await
            .expect("refresh")
            .expect("valid refresh");
        assert_ne!(rotated.2.access_token, tokens.access_token);
        // Old refresh is consumed (rotation) — second use is a REPLAY:
        // family revocation + hard error (not a silent unknown-token).
        assert!(refresh_tokens(&tokens.refresh_token, &cfg).await.is_err());
        // Replay revoked the whole family — successor tokens are gone too.
        assert!(validate_access_token(&rotated.2.access_token).await.expect("v").is_none());

        // Scope mapping: mcp:all → unrestricted; named servers → set.
        assert!(scope_to_allowed_servers("mcp:all").await.is_none());
        let set = scope_to_allowed_servers("srv-a").await.expect("scope set");
        assert!(set.contains("srv-a"));
    }

    #[tokio::test]
    async fn provider_config_defaults_and_overrides() {
        init_pool().await;
        let cfg = provider_config().await;
        assert!(!cfg.enabled, "disabled by default");
        assert_eq!(cfg.access_token_lifetime, 3600);
    }
}
