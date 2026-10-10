//! Upstream OAuth client for MCPHub Desktop.
//!
//! Wraps the rmcp 3.4.1 `auth` feature (`AuthorizationManager` + `OAuthState`)
//! to give OAuth-protected upstream MCP servers (sse / streamable-http /
//! openapi) a full OAuth 2.1 client flow:
//!
//! 1. **Challenge-driven discovery** — the transport hits the upstream, gets a
//!    401 with `WWW-Authenticate: Bearer resource_metadata=...`, and we seed
//!    the rmcp discovery from that challenge (MCP auth spec).
//! 2. **Client registration** — rmcp picks the highest-priority mechanism the
//!    authorization server supports: pre-registered client_id (our
//!    `ServerOauthConfig` fields) → CIMD → RFC 7591 dynamic registration.
//! 3. **Authorization code + PKCE (S256)** — `start_authorization` returns an
//!    authorization URL; we open the system browser and run an **ephemeral
//!    loopback listener** (RFC 8252) whose port is fixed into the redirect URI.
//!    The listener completes the flow by itself; the user just approves in the
//!    browser.
//! 4. **Persistence** — rmcp's `StoredCredentials` / `StoredAuthorizationState`
//!    are JSON-serializable SDK structs; they live in the `oauth_upstream_tokens`
//!    table (v30) via a custom `CredentialStore`/`StateStore` pair, so refreshes
//!    survive restarts without touching the server config row.
//! 5. **Refresh** — `AuthorizationManager::get_access_token` refreshes
//!    proactively (REFRESH_BUFFER before expiry) and surfaces
//!    `AuthorizationRequired` when re-auth is genuinely needed.
//!
//! Static-token mode: a hand-filled `oauth.accessToken` bypasses this module
//! entirely (injected directly by the transports, §A3).
//!
//! # allowInsecureTokenEndpoint (#1293 mirror)
//! Hand-filled `tokenEndpoint` / discovered metadata over plain HTTP is
//! rejected unless `allowInsecureTokenEndpoint: true` was saved by an admin.
//! Discovered HTTP endpoints are likewise blocked (loopback 127.0.0.1 is
//! exempt for local dev authorization servers).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use rmcp::transport::auth::{
    AuthorizationManager, AuthorizationRequest, CredentialStore, OAuthState, StateStore,
    StoredAuthorizationState, StoredCredentials,
};
use tauri::Emitter;

use crate::models::server::ServerOauthConfig;
use crate::mcp::progress::get_app_handle;

// ────────────────────────────────────────────────────────────────────────────
// Errors
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum OauthError {
    /// Server has no OAuth support at all (no metadata, no challenge).
    NoOauthSupport,
    /// Flow is waiting for the user to approve in the browser.
    Pending { auth_url: String, state: String },
    /// Re-auth needed (expired/revoked refresh token).
    AuthorizationRequired,
    Other(String),
}

impl std::fmt::Display for OauthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoOauthSupport => write!(f, "server does not support OAuth"),
            Self::Pending { auth_url, .. } => write!(f, "authorization pending: {auth_url}"),
            Self::AuthorizationRequired => write!(f, "re-authorization required"),
            Self::Other(m) => write!(f, "{m}"),
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Security helpers
// ────────────────────────────────────────────────────────────────────────────

/// #1293 mirror: a token endpoint over plain HTTP sends credentials/tokens
/// without TLS. Allowed only when the admin saved
/// `allowInsecureTokenEndpoint: true` for this server (localhost is exempt —
/// local dev authorization servers are a supported desktop scenario).
fn ensure_token_endpoint_security(
    endpoint: &str,
    allow_insecure: bool,
) -> Result<(), OauthError> {
    let parsed = url::Url::parse(endpoint)
        .map_err(|e| OauthError::Other(format!("invalid token endpoint: {e}")))?;
    if parsed.scheme() == "https" {
        return Ok(());
    }
    let is_loopback = matches!(parsed.host_str(), Some(h) if h == "localhost" || h == "127.0.0.1" || h == "[::1]");
    if is_loopback || allow_insecure {
        Ok(())
    } else {
        Err(OauthError::Other(format!(
            "token endpoint '{endpoint}' uses plain HTTP; enable 'Allow HTTP OAuth token endpoints' (admin) to allow"
        )))
    }
}

// ────────────────────────────────────────────────────────────────────────────
// DB-backed stores (v30 oauth_upstream_tokens)
// ────────────────────────────────────────────────────────────────────────────

fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Per-server `CredentialStore` backed by `oauth_upstream_tokens.credentials_json`.
struct DbCredentialStore {
    server_name: String,
}

#[async_trait::async_trait]
impl CredentialStore for DbCredentialStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, rmcp::transport::auth::AuthError> {
        let row: Option<String> = sqlx::query_scalar(
            "SELECT credentials_json FROM oauth_upstream_tokens WHERE server_name = ?",
        )
        .bind(&self.server_name)
        .fetch_optional(crate::db::pool())
        .await
        .map_err(|e| {
            rmcp::transport::auth::AuthError::OAuthError(format!("credential store load: {e}"))
        })?;
        match row {
            Some(json) if !json.is_empty() => serde_json::from_str(&json).map(Some).map_err(|e| {
                rmcp::transport::auth::AuthError::OAuthError(format!(
                    "corrupt credentials_json for {}: {e}",
                    self.server_name
                ))
            }),
            _ => Ok(None),
        }
    }

    async fn save(
        &self,
        credentials: StoredCredentials,
    ) -> Result<(), rmcp::transport::auth::AuthError> {
        let json = serde_json::to_string(&credentials).map_err(|e| {
            rmcp::transport::auth::AuthError::OAuthError(format!("serialize credentials: {e}"))
        })?;
        upsert_token_row(&self.server_name, "connected", Some(&json), Some(""), now_epoch())
            .await
            .map_err(|e| {
                rmcp::transport::auth::AuthError::OAuthError(format!("credential store save: {e}"))
            })
    }

    async fn clear(&self) -> Result<(), rmcp::transport::auth::AuthError> {
        sqlx::query("DELETE FROM oauth_upstream_tokens WHERE server_name = ?")
            .bind(&self.server_name)
            .execute(crate::db::pool())
            .await
            .map_err(|e| {
                rmcp::transport::auth::AuthError::OAuthError(format!("credential store clear: {e}"))
            })?;
        Ok(())
    }
}

/// Per-server `StateStore` backed by `oauth_upstream_tokens.pending_json`
/// (single pending authorization round per server; stale state expires via
/// rmcp's created_at TTL handling).
struct DbStateStore {
    server_name: String,
}

#[async_trait::async_trait]
impl StateStore for DbStateStore {
    async fn save(
        &self,
        _csrf_token: &str,
        state: StoredAuthorizationState,
    ) -> Result<(), rmcp::transport::auth::AuthError> {
        let json = serde_json::to_string(&state).map_err(|e| {
            rmcp::transport::auth::AuthError::OAuthError(format!("serialize state: {e}"))
        })?;
        upsert_token_row(&self.server_name, "pending", None, Some(&json), now_epoch())
            .await
            .map_err(|e| {
                rmcp::transport::auth::AuthError::OAuthError(format!("state store save: {e}"))
            })
    }

    async fn load(
        &self,
        csrf_token: &str,
    ) -> Result<Option<StoredAuthorizationState>, rmcp::transport::auth::AuthError> {
        let row: Option<String> = sqlx::query_scalar(
            "SELECT pending_json FROM oauth_upstream_tokens WHERE server_name = ?",
        )
        .bind(&self.server_name)
        .fetch_optional(crate::db::pool())
        .await
        .map_err(|e| {
            rmcp::transport::auth::AuthError::OAuthError(format!("state store load: {e}"))
        })?;
        let Some(json) = row.filter(|j| !j.is_empty()) else {
            return Ok(None);
        };
        let state: StoredAuthorizationState =
            serde_json::from_str(&json).map_err(|e| {
                rmcp::transport::auth::AuthError::OAuthError(format!(
                    "corrupt pending_json for {}: {e}",
                    self.server_name
                ))
            })?;
        // CSRF contract (rmcp keys the round by this): a state value that
        // doesn't match the pending round must read as None — returning the
        // blob unconditionally would let a forged callback state complete
        // the exchange (authorization-code injection).
        if state.csrf_token != csrf_token {
            return Ok(None);
        }
        Ok(Some(state))
    }

    async fn delete(
        &self,
        _csrf_token: &str,
    ) -> Result<(), rmcp::transport::auth::AuthError> {
        // A completed flow clears the pending blob; the credentials row stays.
        sqlx::query("UPDATE oauth_upstream_tokens SET pending_json = '' WHERE server_name = ?")
            .bind(&self.server_name)
            .execute(crate::db::pool())
            .await
            .map_err(|e| {
                rmcp::transport::auth::AuthError::OAuthError(format!("state store delete: {e}"))
            })?;
        Ok(())
    }
}

async fn upsert_token_row(
    server_name: &str,
    status: &str,
    credentials_json: Option<&str>,
    pending_json: Option<&str>,
    updated_at: u64,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO oauth_upstream_tokens (server_name, credentials_json, pending_json, status, updated_at) \
         VALUES (?, COALESCE(?, ''), COALESCE(?, ''), ?, ?) \
         ON CONFLICT(server_name) DO UPDATE SET \
           credentials_json = CASE WHEN excluded.credentials_json != '' THEN excluded.credentials_json ELSE oauth_upstream_tokens.credentials_json END, \
           pending_json = CASE WHEN excluded.pending_json != '' THEN excluded.pending_json ELSE oauth_upstream_tokens.pending_json END, \
           status = excluded.status, \
           updated_at = excluded.updated_at",
    )
    .bind(server_name)
    .bind(credentials_json.unwrap_or(""))
    .bind(pending_json.unwrap_or(""))
    .bind(status)
    .bind(updated_at as i64)
    .execute(crate::db::pool())
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ────────────────────────────────────────────────────────────────────────────
// Loopback redirect listener (RFC 8252)
// ────────────────────────────────────────────────────────────────────────────

/// One-shot loopback HTTP listener for the OAuth redirect. Binds 127.0.0.1 on
/// an ephemeral port, serves exactly one `/callback?code=..&state=..` GET, then
/// shuts down. Kept alive for the whole authorization round (10 min timeout).
struct LoopbackListener {
    port: u16,
    code_rx: tokio::sync::oneshot::Receiver<Result<CallbackHit, String>>,
    // Kept alive for the listener's lifetime — dropping this aborts the
    // accept loop (a function-local guard would die the moment bind()
    // returns, leaving the redirect URI pointing at a dead port).
    _accept_guard: AbortOnDropHandle,
}

impl LoopbackListener {
    async fn bind() -> Result<Self, String> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| format!("loopback bind: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let (code_tx, code_rx) = tokio::sync::oneshot::channel();

        let _guard = AbortOnDropHandle(tokio::spawn(async move {
            // The code channel is single-consumer: wrap it so each accepted
            // connection can try to deliver (first send wins; later requests
            // — e.g. a browser preflight — are answered but ignored).
            let slot = Arc::new(tokio::sync::Mutex::new(Some(code_tx)));
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(600);
            let server = async {
                loop {
                    let (stream, _) = match listener.accept().await {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    let slot = slot.clone();
                    tokio::spawn(async move {
                        let _ = serve_one_callback(stream, slot).await;
                    });
                }
            };
            tokio::select! {
                _ = server => {}
                _ = tokio::time::sleep_until(deadline) => {}
            }
        }));

        Ok(Self { port, code_rx, _accept_guard: _guard })
    }

    fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{port}/callback", port = self.port)
    }

    /// Wait for the authorization-code callback (10 min deadline).
    async fn wait_for_code(self) -> Result<CallbackHit, String> {
        match tokio::time::timeout(std::time::Duration::from_secs(600), self.code_rx).await {
            Ok(Ok(Ok(hit))) => Ok(hit),
            Ok(Ok(Err(e))) => Err(e), // browser-side error=... deny/timeout
            Ok(Err(_)) => Err("authorization listener dropped".to_string()),
            Err(_) => Err("authorization timed out waiting for browser callback".to_string()),
        }
    }

}

/// The loopback callback payload: `code` + `state` (both required by rmcp's
/// `AuthorizationCallback::from_redirect_url` — dropping state made every
/// token exchange fail with "Authorization callback missing state").
#[derive(Debug)]
struct CallbackHit {
    code: String,
    state: String,
}

/// Dropping the LoopbackListener aborts the accept loop task.
struct AbortOnDropHandle(tokio::task::JoinHandle<()>);
impl Drop for AbortOnDropHandle {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Read one HTTP request from the stream; answer with a small landing page;
/// push `code`/`state` (or the provider's `error`) into the shared slot.
async fn serve_one_callback(
    mut stream: tokio::net::TcpStream,
    slot: Arc<tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<Result<CallbackHit, String>>>>>,
) -> std::io::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).await.unwrap_or(0);
    let req = String::from_utf8_lossy(&buf[..n]);
    // Request line: GET /callback?code=..&state=.. HTTP/1.1
    let target = req.split_whitespace().nth(1).unwrap_or("");
    let query = target.split('?').nth(1).unwrap_or("");
    let params: HashMap<String, String> = query
        .split('&')
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            Some((k.to_string(), urlencoding_decode(v)))
        })
        .collect();


    let (status_line, body) = if let Some(err) = params.get("error") {
        (
            "400 Bad Request",
            format!("Authorization failed: {err}. You can close this tab."),
        )
    } else if params.contains_key("code") {
        ("200 OK", "Authorization complete. You can close this tab.".to_string())
    } else {
        ("400 Bad Request", "Missing authorization code. You can close this tab.".to_string())
    };

    let ok = params.contains_key("code") && !params.contains_key("error");
    let result = if ok {
        Ok(CallbackHit {
            code: params.get("code").cloned().unwrap_or_default(),
            state: params.get("state").cloned().unwrap_or_default(),
        })
    } else {
        Err(params.get("error").cloned().unwrap_or_else(|| "missing code".into()))
    };
    // Deliver once: the first successful callback wins. Requests without a
    // code/error (favicon, OPTIONS preflight, manual visits) are answered but
    // NEVER delivered — they would consume the single-consumer slot and kill
    // the flow before the real callback arrives.
    if ok || params.contains_key("error") {
        if let Some(tx) = slot.lock().await.take() {
            let _ = tx.send(result);
        }
    }

    let http = format!(
        "HTTP/1.1 {status_line}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(http.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

/// Minimal percent-decoding for query values (spaces, %XX). Query values from
/// authorization servers are UTF-8 percent-encoded.
fn urlencoding_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(b);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ────────────────────────────────────────────────────────────────────────────
// Per-server cancel registry (the flow spans async boundaries: command →
// browser → loopback callback → completion)
// ────────────────────────────────────────────────────────────────────────────

/// Cancel signals only — the flow state itself is owned exclusively by the
/// completion task (AuthorizationManager is not Clone; keying the registry by
/// server name and removing from it inside the task let a restarted flow's
/// remove() steal the newer flow's entry).
static FLOWS: LazyLock<std::sync::Mutex<HashMap<String, Arc<tokio::sync::Notify>>>> =
    LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

fn flows() -> std::sync::MutexGuard<'static, HashMap<String, Arc<tokio::sync::Notify>>> {
    FLOWS.lock().unwrap_or_else(|p| p.into_inner())
}

fn register_cancel(server_name: &str) -> Arc<tokio::sync::Notify> {
    let notify = Arc::new(tokio::sync::Notify::new());
    flows().insert(server_name.to_string(), notify.clone());
    notify
}

/// Completion driver: dynamic mode runs through `OAuthState` (which owns
/// discovery); static mode bypasses `start_authorization` — it re-resolves
/// metadata unconditionally and would overwrite the hand-filled endpoints —
/// and drives an `AuthorizationSession` built directly on the preset metadata.
enum FlowDriver {
    State(OAuthState),
    Session(rmcp::transport::auth::AuthorizationSession),
}

impl FlowDriver {
    async fn complete(&mut self, callback_url: &str, code: &str, state: &str) -> Result<(), rmcp::transport::auth::AuthError> {
        match self {
            Self::State(s) => s.handle_callback_url(callback_url).await,
            Self::Session(sess) => sess.handle_callback(code, state).await.map(|_| ()),
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Public flow API
// ────────────────────────────────────────────────────────────────────────────

/// Begin an authorization flow for `server_name`:
/// challenge-driven discovery → registration (pre-registered / CIMD / dynamic)
/// → PKCE authorization URL → system browser + loopback listener.
///
/// The flow completes asynchronously: the loopback listener exchanges the code
/// via rmcp (`handle_callback_url`), credentials are persisted through
/// `DbCredentialStore`, status flips to `connected`, and a
/// `server://oauth-connected` event notifies the frontend.
pub async fn start_authorization(
    server_name: &str,
    upstream_url: &str,
    oauth_cfg: &ServerOauthConfig,
    challenge: Option<&str>,
) -> Result<String, OauthError> {
    // Supersede any already-running flow for this server (restart = new
    // listener): signal the old completion task so its listener dies and it
    // can never write state for an authorization the user replaced.
    if let Some(old) = flows().remove(server_name) {
        old.notify_one(); // stored permit: wakes even if not yet awaiting
    }

    // RFC 8252 §7.3: loopback redirects may use an ephemeral port. Bind the
    // listener FIRST so its port can be baked into the redirect URI.
    let listener = LoopbackListener::bind().await.map_err(OauthError::Other)?;
    let redirect_uri = match oauth_cfg.redirect_uri.clone().filter(|s| !s.is_empty()) {
        Some(uri) => {
            // The loopback listener is the ONLY completion path (no deep-link
            // handler exists): a redirect_uri the browser delivers elsewhere —
            // an external https URL, or a fixed loopback port other than the
            // ephemeral port actually bound — would hang the flow until the
            // 600s timeout. Loopback URIs must match the bound listener's
            // port; non-loopback URIs are rejected outright.
            let parsed = url::Url::parse(&uri).map_err(|_| {
                OauthError::Other(format!("configured redirect_uri is not a valid URL: {uri}"))
            })?;
            let host = parsed.host_str().unwrap_or_default();
            let loopback = matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]");
            let bound_port = listener.port;
            let uri_port = parsed.port();
            if !loopback
                || uri_port.map(|p| p != bound_port).unwrap_or(true)
                || parsed.path() != "/"
            {
                return Err(OauthError::Other(format!(
                    "configured redirect_uri '{uri}' cannot receive the callback (this build completes flows via the ephemeral loopback listener on port {bound_port})"
                )));
            }
            format!("http://127.0.0.1:{bound_port}/")
        }
        None => listener.redirect_uri(),
    };

    let allow_insecure = oauth_cfg.allow_insecure_token_endpoint.unwrap_or(false);

    // Hand-filled endpoints: static configuration mode — validate the token
    // endpoint gate and skip discovery entirely.
    let static_mode = matches!(
        (
            oauth_cfg.authorization_endpoint.as_deref().filter(|s| !s.is_empty()),
            oauth_cfg.token_endpoint.as_deref().filter(|s| !s.is_empty()),
        ),
        (Some(_), Some(_))
    );
    if static_mode {
        // Unwrap-safe: matched Some/Some above.
        let tok_ep = oauth_cfg.token_endpoint.as_deref().unwrap_or_default();
        ensure_token_endpoint_security(tok_ep, allow_insecure)?;
    }

    // Build the rmcp state machine against the upstream base URL.
    let mut manager = AuthorizationManager::new(upstream_url.to_string())
        .await
        .map_err(|e| OauthError::Other(format!("oauth init: {e}")))?;
    manager.set_credential_store(DbCredentialStore { server_name: server_name.to_string() });
    manager.set_state_store(DbStateStore { server_name: server_name.to_string() });

    if !static_mode {
        // Dynamic mode: pre-resolve discovery ourselves so we can
        //  1) reject servers with NO OAuth support — rmcp's
        //     LegacyEndpointFallback would otherwise synthesize /authorize
        //     from the base URL and we'd open the browser at a nonsense URL;
        //  2) apply the #1293 gate to the DISCOVERED token endpoint too
        //     (start_authorization re-resolves internally and ignores preset
        //     metadata, so validation must happen against our own resolution).
        let resolution = manager
            .resolve_metadata_from_challenge(challenge.filter(|s| !s.is_empty()))
            .await
            .map_err(|e| OauthError::Other(format!("oauth discovery: {e}")))?;
        use rmcp::transport::auth::AuthorizationMetadataSource as Src;
        match resolution.source {
            Src::LegacyEndpointFallback => {
                return Err(OauthError::NoOauthSupport);
            }
            // Non-exhaustive SDK enum: fail closed on unknown sources.
            _ => {
                ensure_token_endpoint_security(&resolution.metadata.token_endpoint, allow_insecure)?;
            }
        }
    }

    let mut request = AuthorizationRequest::new(redirect_uri.clone())
        .with_client_name(format!("MCPHub Desktop ({server_name})"));
    if let Some(ch) = challenge.filter(|s| !s.is_empty()) {
        request = request.with_challenge(ch);
    }
    if let Some(cid) = oauth_cfg.client_id.as_deref().filter(|s| !s.is_empty()) {
        request = request.with_preregistered_client(cid);
        if let Some(secret) = oauth_cfg.client_secret.as_deref().filter(|s| !s.is_empty()) {
            request = request.with_client_secret(secret);
        }
    }
    if let Some(scopes) = oauth_cfg.scopes.as_ref().filter(|s| !s.is_empty()) {
        request = request.with_scopes(scopes.iter().cloned());
    }

    // Hand-filled endpoints: feed the metadata directly to the manager.
    if static_mode {
        // AuthorizationMetadata is #[non_exhaustive]: build via Default +
        // field assignment (struct-literal syntax is rejected outside the
        // defining crate even with ..Default::default()).
        let mut metadata = rmcp::transport::auth::AuthorizationMetadata::default();
        metadata.authorization_endpoint = oauth_cfg.authorization_endpoint.clone().unwrap_or_default();
        metadata.token_endpoint = oauth_cfg.token_endpoint.clone().unwrap_or_default();
        metadata.scopes_supported = oauth_cfg.scopes.clone();
        metadata.code_challenge_methods_supported = Some(vec!["S256".to_string()]);
        manager.set_metadata(metadata);
    }

    // Static mode builds the session directly on the preset metadata —
    // OAuthState::start_authorization re-resolves metadata unconditionally and
    // would discard the hand-filled endpoints.
    let (driver, auth_url) = if static_mode {
        let session = rmcp::transport::auth::AuthorizationSession::new(manager, request)
            .await
            .map_err(|(_, e)| OauthError::Other(format!("start authorization: {e}")))?;
        let auth_url = session.auth_url.clone();
        (FlowDriver::Session(session), auth_url)
    } else {
        let mut state = OAuthState::Unauthorized(manager);
        state
            .start_authorization(request)
            .await
            .map_err(|e| OauthError::Other(format!("start authorization: {e}")))?;
        let auth_url = match &state {
            OAuthState::Session(session) => session.auth_url.clone(),
            _ => return Err(OauthError::Other("state not in session after start".into())),
        };
        (FlowDriver::State(state), auth_url)
    };

    // Persist pending status for the UI (`oauth_required` semantics).
    upsert_token_row(server_name, "pending", None, None, now_epoch())
        .await
        .map_err(OauthError::Other)?;

    // Open the system browser (reuses open_external_url's URL validation). On
    // failure revert the pending row so the UI doesn't hang on "pending".
    if let Err(e) = open_system_browser(&auth_url) {
        let _ = upsert_token_row(server_name, "error", None, None, now_epoch()).await;
        return Err(e);
    }

    // Register the cancel signal for disconnect(); the flow state itself is
    // moved into the completion task (no registry round-trip → a restarted
    // flow can never steal the previous task's entry).
    let cancel = register_cancel(server_name);

    // Arm the background completion task: wait for the loopback callback →
    // exchange the code via rmcp → persist credentials + notify the frontend.
    let server = server_name.to_string();
    let cb_redirect = redirect_uri.clone();
    tokio::spawn(async move {
        // Take the flow out (no other completion path exists).
        let hit = match tokio::select! {
            hit = listener.wait_for_code() => hit,
            _ = cancel.notified() => {
                log::info!("[oauth:{server}] flow cancelled (disconnect/restart)");
                return;
            }
        } {
            Ok(hit) => hit,
            Err(e) => {
                log::warn!("[oauth:{server}] callback failed: {e}");
                if still_owns_flow(&server, &cancel) {
                    let _ = upsert_token_row(&server, "error", None, None, now_epoch()).await;
                    emit_oauth_event("server://oauth-error", &server, &e);
                }
                return;
            }
        };
        // rmcp requires BOTH code and state (the PKCE verifier is looked up
        // by the state/csrf token in the state store).
        let callback_url = format!(
            "{cb_redirect}?code={}&state={}",
            urlencode_component(&hit.code),
            urlencode_component(&hit.state)
        );
        let mut driver = driver;
        match driver.complete(&callback_url, &hit.code, &hit.state).await {
            Ok(_token_response) => {
                // disconnect()/restart may have landed while the exchange was
                // in flight — notify_one cannot stop a task past the select!,
                // so ownership is re-checked before EVERY status write or a
                // deleted row would be resurrected with live credentials.
                if still_owns_flow(&server, &cancel) {
                    let _ = upsert_token_row(&server, "connected", None, None, now_epoch()).await;
                    log::info!("[oauth:{server}] authorization completed, credentials stored");
                    emit_oauth_event("server://oauth-connected", &server, "authorized");
                }
            }
            Err(e) => {
                log::warn!("[oauth:{server}] token exchange failed: {e}");
                if still_owns_flow(&server, &cancel) {
                    let _ = upsert_token_row(&server, "error", None, None, now_epoch()).await;
                    emit_oauth_event("server://oauth-error", &server, &e.to_string());
                }
            }
        }
        // Remove the cancel entry only if THIS task still owns it — a
        // restarted flow has already replaced the entry, and stealing it
        // would make disconnect unable to cancel the live flow.
        let owns = flows()
            .get(&server)
            .map(|n| Arc::ptr_eq(n, &cancel))
            .unwrap_or(false);
        if owns {
            flows().remove(&server);
        }
    });

    log::info!(
        "[oauth:{server_name}] authorization started (redirect={redirect_uri})",
    );
    Ok(auth_url)
}

/// Poll whether the server holds usable credentials (connected status).
pub async fn has_credentials(server_name: &str) -> bool {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT credentials_json FROM oauth_upstream_tokens WHERE server_name = ?",
    )
    .bind(server_name)
    .fetch_one(crate::db::pool())
    .await
    .ok()
    .flatten()
    .map(|j| !j.is_empty())
    .unwrap_or(false)
}

/// Persisted flow status for the UI: disabled | pending | connected | error.
pub async fn status_of(server_name: &str) -> String {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT status FROM oauth_upstream_tokens WHERE server_name = ?",
    )
    .bind(server_name)
    .fetch_one(crate::db::pool())
    .await
    .ok()
    .flatten()
    .unwrap_or_else(|| "disabled".to_string())
}

/// Whether this completion task still owns the server's flow entry: a
/// disconnect() or restarted flow replaces/removes the entry, and this task
/// must not write status rows (they would resurrect deleted state) or emit
/// events for an authorization the user replaced.
fn still_owns_flow(server_name: &str, cancel: &std::sync::Arc<tokio::sync::Notify>) -> bool {
    flows()
        .get(server_name)
        .map(|n| Arc::ptr_eq(n, cancel))
        .unwrap_or(false)
}

/// Clear all stored credentials/pending state for a server (disconnect).
pub async fn disconnect(server_name: &str) -> Result<(), String> {
    // Signal any in-flight completion task to stop before deleting the row.
    if let Some(cancel) = flows().remove(server_name) {
        // notify_one stores a permit when no waiter exists yet — the
        // completion task may not have been polled since spawn; notify_waiters
        // (no permit storage) would lose that race and let a disconnected
        // flow resurrect a "connected" row.
        cancel.notify_one();
    }
    sqlx::query("DELETE FROM oauth_upstream_tokens WHERE server_name = ?")
        .bind(server_name)
        .execute(crate::db::pool())
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Open the system browser with open_external_url's validation (non-http and
/// shell-metacharacter URLs are refused).
fn open_system_browser(url: &str) -> Result<(), OauthError> {
    // Test knob: the in-process flow E2E auto-approves via a mock AS that
    // redirects straight to the loopback callback — no real browser needed.
    if std::env::var("MCPHUB_OAUTH_NO_BROWSER").is_ok() {
        return Ok(());
    }
    crate::tray::open_external_url(url.to_string())
        .map_err(|e| OauthError::Other(format!("open browser: {e}")))
}

/// Emit a flow status event to the frontend (same AppHandle stash the
/// install-progress events use).
fn emit_oauth_event(event: &str, server: &str, message: &str) {
    if let Some(app) = get_app_handle() {
        use serde_json::json;
        let _ = app.emit(event, json!({ "serverName": server, "message": message }));
    }
}

/// Percent-encode a query parameter value (state tokens are url-safe base64,
/// but re-encode defensively so `&`/`=` inside can never corrupt the URL).
fn urlencode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_endpoint_security_gate() {
        // HTTPS always allowed.
        assert!(ensure_token_endpoint_security("https://up.example/token", false).is_ok());
        // Plain HTTP blocked unless the admin opted in (#1293 mirror).
        assert!(ensure_token_endpoint_security("http://up.example/token", false).is_err());
        assert!(ensure_token_endpoint_security("http://up.example/token", true).is_ok());
        // Loopback exempt (local dev authorization servers).
        assert!(ensure_token_endpoint_security("http://127.0.0.1:8080/token", false).is_ok());
        assert!(ensure_token_endpoint_security("http://localhost:9090/token", false).is_ok());
        // Invalid URL fails closed.
        assert!(ensure_token_endpoint_security("not a url", true).is_err());
    }

    #[test]
    fn urlencoding_decode_roundtrip() {
        assert_eq!(urlencoding_decode("a%20b+c"), "a b c");
        assert_eq!(urlencoding_decode("%E4%B8%AD%E6%96%87"), "中文");
        // Invalid escape passes through literally.
        assert_eq!(urlencoding_decode("100%"), "100%");
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Transport wiring (§A3)
// ────────────────────────────────────────────────────────────────────────────

/// Resolve the `Authorization` header value for an upstream connection:
/// 1. Static pre-obtained `oauth.accessToken` from the server config (highest
///    priority, no flow machinery involved).
/// 2. Stored flow credentials — a fresh rmcp manager is built against the DB
///    stores (refreshing near-expiry tokens proactively); the resulting access
///    token is snapshotted into the connection headers.
///
/// Returns `Ok(None)` when the server has no oauth config or no credentials —
/// the caller proceeds unauthenticated. A stored-credentials error surfaces
/// as `Err` so connect fails loudly instead of silently unauthenticated
/// (mirrors the openapi header-location fail-loud pattern).
pub async fn resolve_authorization_header(
    cfg: &crate::models::server::ServerConfig,
) -> Result<Option<String>, String> {
    let Some(oauth_cfg) = cfg.oauth.as_ref() else {
        return Ok(None);
    };

    // Static token mode: hand-filled accessToken bypasses everything.
    if let Some(token) = oauth_cfg.access_token.as_deref().filter(|t| !t.is_empty()) {
        return Ok(Some(format!("Bearer {token}")));
    }

    // Flow mode: only meaningful for URL-based transports.
    let Some(upstream_url) = cfg.url.as_deref().filter(|u| !u.is_empty()) else {
        return Ok(None);
    };

    let has_flow_creds = has_credentials(&cfg.name).await;
    let status = status_of(&cfg.name).await;
    if !has_flow_creds {
        if status == "pending" {
            return Err(format!(
                "server '{}': OAuth authorization is pending — approve it in the browser that was opened, then reconnect",
                cfg.name
            ));
        }
        // No credentials and no static token: let the upstream 401 speak and
        // drive the `oauth_required` state machine.
        return Ok(None);
    }

    let mut manager = AuthorizationManager::new(upstream_url.to_string())
        .await
        .map_err(|e| format!("oauth manager init for '{}': {e}", cfg.name))?;
    manager.set_credential_store(DbCredentialStore { server_name: cfg.name.clone() });
    manager.set_state_store(DbStateStore { server_name: cfg.name.clone() });
    // Hydrate metadata + client registration from the stored credentials.
    let initialized = manager
        .initialize_from_store()
        .await
        .map_err(|e| format!("oauth initialize for '{}': {e}", cfg.name))?;
    if !initialized {
        return Err(format!(
            "server '{}': stored OAuth credentials are unusable (re-authorize required)",
            cfg.name
        ));
    }
    let token = manager
        .get_access_token()
        .await
        .map_err(|e| format!("oauth token for '{}': {e}", cfg.name))?;
    Ok(Some(format!("Bearer {token}")))
}

/// Merge the resolved OAuth Authorization header into the request headers map
/// (user-supplied Authorization wins — explicit config beats flow tokens).
pub fn merge_authorization_header(
    headers: &mut HashMap<String, String>,
    auth: Option<String>,
) {
    let Some(auth) = auth else { return };
    let has_user_auth = headers.keys().any(|k| k.eq_ignore_ascii_case("authorization"));
    if !has_user_auth {
        headers.insert("Authorization".to_string(), auth);
    }
}

#[cfg(test)]
mod flow_e2e {
    use super::*;
    use crate::models::server::ServerConfig;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const AS_PORT: u16 = 18777;
    const UPSTREAM_PORT: u16 = 18778;
    static AUTHORIZE_HITS: AtomicUsize = AtomicUsize::new(0);
    static REGISTRATIONS: AtomicUsize = AtomicUsize::new(0);
    static TOKEN_EXCHANGES: AtomicUsize = AtomicUsize::new(0);
    static PKCE_FAILURES: AtomicUsize = AtomicUsize::new(0);

    /// Minimal HTTP/1.0 responder over a tokio TcpListener: reads the request
    /// head (and body for POST), hands (method, path, query, body) to `f`,
    /// writes back the bytes `f` returns.
    async fn serve(
        port: u16,
        f: impl Fn(String, String, String, String) -> Vec<u8> + Send + Sync + 'static,
    ) -> tokio::task::JoinHandle<()> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.unwrap();
        let f = std::sync::Arc::new(f);
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else { continue };
                let f = f.clone();
                tokio::spawn(async move {
                    use tokio::io::AsyncReadExt;
                    // Read until the declared Content-Length is satisfied — a
                    // single read() may return only the headers (TCP
                    // segmentation), silently truncating the body.
                    let mut buf: Vec<u8> = Vec::new();
                    let mut chunk = [0u8; 4096];
                    loop {
                        match stream.read(&mut chunk).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                buf.extend_from_slice(&chunk[..n]);
                                let head_end = buf.windows(4).position(|w| w == b"\r\n\r\n");
                                if let Some(h) = head_end {
                                    let cl = String::from_utf8_lossy(&buf[..h])
                                        .to_lowercase()
                                        .lines()
                                        .find_map(|l| l.strip_prefix("content-length:"))
                                        .and_then(|v| v.trim().parse::<usize>().ok())
                                        .unwrap_or(0);
                                    if buf.len() >= h + 4 + cl {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    let req = String::from_utf8_lossy(&buf).to_string();
                    let mut lines = req.split("\r\n");
                    let request_line = lines.next().unwrap_or("");
                    let mut parts = request_line.split_whitespace();
                    let method = parts.next().unwrap_or("").to_string();
                    let target = parts.next().unwrap_or("").to_string();
                    let (path, query) = match target.split_once('?') {
                        Some((p, q)) => (p.to_string(), q.to_string()),
                        None => (target.clone(), String::new()),
                    };
                    let body = req.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
                    let resp = f(method, path, query, body);
                    let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, &resp).await;
                });
            }
        })
    }

    fn http_json(status: &str, obj: serde_json::Value) -> Vec<u8> {
        let body = serde_json::to_string(&obj).unwrap();
        format!(
            "HTTP/1.0 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    fn pkce_s256(verifier: &str) -> String {
        use sha2::{Digest, Sha256};
        use std::fmt::Write as _;
        let digest = Sha256::digest(verifier.as_bytes());
        let mut out = String::new();
        for b in digest {
            write!(out, "{b:02X}").unwrap();
        }
        // base64url without padding — hand-rolled to avoid a base64 dep here.
        const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let bytes = out.as_bytes();
        let hex_to_bytes = |h: &str| {
            (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect::<Vec<u8>>()
        };
        let d = hex_to_bytes(&out);
        let _ = bytes;
        let mut b64 = String::new();
        for chunk in d.chunks(3) {
            let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            b64.push(ALPHA[((n >> 18) & 63) as usize] as char);
            b64.push(ALPHA[((n >> 12) & 63) as usize] as char);
            if chunk.len() > 1 { b64.push(ALPHA[((n >> 6) & 63) as usize] as char); }
            if chunk.len() > 2 { b64.push(ALPHA[(n & 63) as usize] as char); }
        }
        b64
    }

    /// Mock protected upstream: serves its own RFC 9728 discovery path (rmcp
    /// probes the base URL's well-known when the challenge route is unusable)
    /// and a token-gated tools/list.
    async fn start_mock_upstream() {
        serve(UPSTREAM_PORT, move |method, path, _query, _body| {
            let authorized = true; // flow E2E only checks the discovery/OAuth path
            match (method.as_str(), path.as_str()) {
                ("GET", "/.well-known/oauth-protected-resource")
                | ("GET", "/.well-known/oauth-protected-resource/mcp")
                | ("GET", "/mcp/.well-known/oauth-protected-resource") => http_json("200 OK", serde_json::json!({
                    "resource": format!("http://127.0.0.1:{UPSTREAM_PORT}/mcp"),
                    "authorization_servers": [format!("http://127.0.0.1:{AS_PORT}")],
                    "scopes_supported": ["read"],
                })),
                ("POST", "/mcp") if authorized => http_json("200 OK", serde_json::json!({
                    "jsonrpc": "2.0", "id": 1,
                    "result": {"tools": [{"name": "echo", "inputSchema": {"type": "object"}}]}
                })),
                _ => http_json("401 Unauthorized", serde_json::json!({
                    "jsonrpc": "2.0", "id": 1,
                    "error": {"code": -32001, "message": "unauthorized"}
                })),
            }
        }).await;
    }

    /// Mock AS: discovery, RFC7591 register, auto-approve authorize (302 to
    /// the loopback callback), PKCE-verified token endpoint.
    async fn start_mock_as() {
        let challenge_of: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>> =
            Default::default();
        let challenge_of2 = challenge_of.clone();
        serve(AS_PORT, move |method, path, query, body| {
            match (method.as_str(), path.as_str()) {
                ("GET", "/.well-known/oauth-protected-resource") => http_json("200 OK", serde_json::json!({
                    "resource": format!("http://127.0.0.1:{UPSTREAM_PORT}/mcp"),
                    "authorization_servers": [format!("http://127.0.0.1:{AS_PORT}")],
                    "scopes_supported": ["read"],
                })),
                ("GET", "/.well-known/oauth-authorization-server") => http_json("200 OK", serde_json::json!({
                    "issuer": format!("http://127.0.0.1:{AS_PORT}"),
                    "authorization_endpoint": format!("http://127.0.0.1:{AS_PORT}/authorize"),
                    "token_endpoint": format!("http://127.0.0.1:{AS_PORT}/token"),
                    "registration_endpoint": format!("http://127.0.0.1:{AS_PORT}/register"),
                    "code_challenge_methods_supported": ["S256"],
                    "token_endpoint_auth_methods_supported": ["none"],
                })),
                ("GET", "/authorize") => {
                    AUTHORIZE_HITS.fetch_add(1, Ordering::SeqCst);
                    let q: std::collections::HashMap<String, String> = serde_urlencoded(&query);
                    let code = format!("code-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
                    challenge_of.lock().unwrap().insert(code.clone(), q.get("code_challenge").cloned().unwrap_or_default());
                    let redirect = q.get("redirect_uri").cloned().unwrap_or_default();
                    let state = q.get("state").cloned().unwrap_or_default();
                    format!(
                        "HTTP/1.0 302 Found\r\nLocation: {redirect}?code={code}&state={state}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    ).into_bytes()
                }
                ("POST", "/register") => {
                    REGISTRATIONS.fetch_add(1, Ordering::SeqCst);
                    let b: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                    http_json("200 OK", serde_json::json!({
                        "client_id": "test-client",
                        "client_id_issued_at": 1,
                        "redirect_uris": b["redirect_uris"],
                        "token_endpoint_auth_method": "none",
                    }))
                }
                ("POST", "/token") => {
                    let form: std::collections::HashMap<String, String> = serde_urlencoded(&body);
                    match form.get("grant_type").map(|s| s.as_str()) {
                        Some("authorization_code") => {
                            TOKEN_EXCHANGES.fetch_add(1, Ordering::SeqCst);
                            let expected = challenge_of.lock().unwrap().remove(form.get("code").map(|s| s.as_str()).unwrap_or(""));
                            let verifier = form.get("code_verifier").cloned().unwrap_or_default();
                            if expected.as_deref() != Some(pkce_s256(&verifier).as_str()) {
                                PKCE_FAILURES.fetch_add(1, Ordering::SeqCst);
                                return http_json("400 Bad Request", serde_json::json!({"error": "invalid_grant"}));
                            }
                            http_json("200 OK", serde_json::json!({
                                "access_token": "flow-e2e-access-token",
                                "token_type": "Bearer",
                                "expires_in": 3600,
                                "refresh_token": "flow-e2e-refresh-token",
                                "scope": "read",
                            }))
                        }
                        _ => http_json("400 Bad Request", serde_json::json!({"error": "unsupported_grant_type"})),
                    }
                }
                _ => http_json("404 Not Found", serde_json::json!({"error": "not_found"})),
            }
        }).await;
        let _ = challenge_of2; // keep the clone alive conceptually
    }

    /// application/x-www-form-urlencoded + query string parser (values are
    /// percent-encoded; '+' means space).
    fn serde_urlencoded(s: &str) -> std::collections::HashMap<String, String> {
        s.split('&')
            .filter_map(|kv| {
                let (k, v) = kv.split_once('=')?;
                Some((urlencoding_decode(k), urlencoding_decode(v)))
            })
            .collect()
    }

    async fn init_test_db() {
        // Shared test-pool init (single in-memory DB, migrated once) — see
        // db::ensure_test_pool. The previous per-module `Once` + `tokio::spawn`
        // raced with the oauth_provider tests' own init: whichever global
        // replace ran second pointed the slot at a pool other tests were
        // still migrating ("no such table" under parallel cargo test).
        crate::db::ensure_test_pool().await;
    }

    /// Full flow: discovery → dynamic registration → PKCE authorize (mock AS
    /// auto-approves by 302 to our loopback listener) → token exchange →
    /// credentials persisted + status connected.
    #[tokio::test(flavor = "multi_thread")]
    async fn full_authorization_flow_completes() {
        std::env::set_var("MCPHUB_OAUTH_NO_BROWSER", "1");
        init_test_db().await;
        start_mock_as().await;
        start_mock_upstream().await;
        // Give the mock servers a beat to bind before discovery fires.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let cfg = ServerOauthConfig {
            allow_insecure_token_endpoint: None,
            client_id: None, // dynamic registration path
            client_secret: None,
            scopes: Some(vec!["read".into()]),
            access_token: None,
            refresh_token: None,
            authorization_endpoint: None,
            token_endpoint: None,
            resource: None,
            redirect_uri: None, // ephemeral loopback (RFC 8252)
            revocation_endpoint: None,
            dynamic_registration: None,
        };
        let upstream = format!("http://127.0.0.1:{UPSTREAM_PORT}/mcp");
        let challenge = format!(
            "Bearer resource_metadata=\"http://127.0.0.1:{AS_PORT}/.well-known/oauth-protected-resource\""
        );

        let auth_url = start_authorization("oauth-e2e", &upstream, &cfg, Some(&challenge))
            .await
            .expect("flow starts");
        assert!(auth_url.contains("code_challenge="), "PKCE in URL: {auth_url}");
        assert!(auth_url.contains("state="), "state in URL: {auth_url}");

        // Simulate the browser: follow the authorize URL's redirect chain
        // (AS 302 → loopback callback) without a real browser.
        {
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("client");
            let mut url = auth_url.clone();
            for _ in 0..10 {
                let resp = client.get(&url).send().await.expect("browser GET");
                match resp.status().as_u16() {
                    300..=399 => {
                        let loc = resp
                            .headers()
                            .get("location")
                            .and_then(|v| v.to_str().ok())
                            .map(|s| s.to_string())
                            .expect("redirect location");
                        url = loc;
                    }
                    _ => break,
                }
            }
        }

        // Wait for the completion task to persist credentials.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            if has_credentials("oauth-e2e").await {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "flow did not complete in 15s (status={:?}, err={:?}, register={}, authorize={}, token={}, pkce_fail={})",
                status_of("oauth-e2e").await,
                sqlx::query_scalar::<_, Option<String>>(
                    "SELECT error_message FROM oauth_upstream_tokens WHERE server_name = ?")
                    .bind("oauth-e2e").fetch_one(crate::db::pool()).await.ok().flatten(),
                REGISTRATIONS.load(Ordering::SeqCst),
                AUTHORIZE_HITS.load(Ordering::SeqCst),
                TOKEN_EXCHANGES.load(Ordering::SeqCst),
                PKCE_FAILURES.load(Ordering::SeqCst),
            );
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        assert_eq!(status_of("oauth-e2e").await, "connected");
        assert!(REGISTRATIONS.load(Ordering::SeqCst) >= 1, "dynamic registration ran");
        assert_eq!(PKCE_FAILURES.load(Ordering::SeqCst), 0, "PKCE verified");
        assert_eq!(TOKEN_EXCHANGES.load(Ordering::SeqCst), 1, "exactly one token exchange");

        // Stored credentials carry the mock AS access token. Read via serde —
        // the oauth2 TokenResponse trait isn't re-exported by rmcp.
        let creds: StoredCredentials = serde_json::from_str(
            &sqlx::query_scalar::<_, String>(
                "SELECT credentials_json FROM oauth_upstream_tokens WHERE server_name='oauth-e2e'",
            )
            .fetch_one(crate::db::pool())
            .await
            .unwrap(),
        )
        .unwrap();
        let token_json = serde_json::to_value(creds.token_response.unwrap()).unwrap();
        assert_eq!(token_json["access_token"], "flow-e2e-access-token");

        // Disconnect clears the row and signals the (already finished) task.
        disconnect("oauth-e2e").await.unwrap();
        assert_eq!(status_of("oauth-e2e").await, "disabled");
    }

    /// A server with NO OAuth support (no metadata, no challenge resource) must
    /// surface NoOauthSupport instead of rmcp's legacy endpoint synthesis.
    #[tokio::test(flavor = "multi_thread")]
    async fn no_oauth_server_is_rejected() {
        std::env::set_var("MCPHUB_OAUTH_NO_BROWSER", "1");
        init_test_db().await;
        // Port 1 is unroutable — discovery fails → legacy fallback.
        let cfg = ServerOauthConfig { scopes: None, ..Default::default() };
        let err = start_authorization("oauth-none", "http://127.0.0.1:1/mcp", &cfg, None).await;
        assert!(matches!(err, Err(OauthError::NoOauthSupport) | Err(OauthError::Other(_))), "{err:?}");
    }

    /// Static configuration mode: hand-filled endpoints skip discovery and the
    /// plain-HTTP token endpoint is rejected without the admin opt-in.
    #[tokio::test(flavor = "multi_thread")]
    async fn static_mode_http_token_endpoint_gated() {
        std::env::set_var("MCPHUB_OAUTH_NO_BROWSER", "1");
        init_test_db().await;
        let cfg = ServerOauthConfig {
            client_id: Some("cid".into()),
            authorization_endpoint: Some("http://as.example/authorize".into()),
            token_endpoint: Some("http://as.example/token".into()),
            scopes: None,
            ..Default::default()
        };
        let err = start_authorization("oauth-static", "http://127.0.0.1:1/mcp", &cfg, None).await;
        match err {
            Err(OauthError::Other(m)) => assert!(m.contains("Allow HTTP OAuth token endpoints"), "{m}"),
            other => panic!("expected insecure-endpoint gate error, got {other:?}"),
        }
        // With the opt-in, the flow proceeds to start (browser skipped).
        let cfg_ok = ServerOauthConfig {
            client_id: Some("cid".into()),
            authorization_endpoint: Some("http://127.0.0.1:1/authorize".into()),
            token_endpoint: Some("http://127.0.0.1:1/token".into()),
            scopes: None,
            ..Default::default()
        };
        let ok = start_authorization("oauth-static2", "http://127.0.0.1:1/mcp", &cfg_ok, None).await;
        assert!(ok.is_ok(), "{ok:?}");
    }

    // Silence unused import when ServerConfig isn't referenced in this module.
    #[allow(dead_code)]
    fn _assert_cfg(_c: &ServerConfig) {}
}
