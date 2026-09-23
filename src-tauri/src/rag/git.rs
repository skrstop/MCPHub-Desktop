//! Git data source support for the RAG import pipeline (需求2).
//!
//! Pure-Rust git via `gix` (gitoxide) — compiled into the binary, no external
//! git needed. Shallow clone (depth, default 1) only; update strategy = full
//! re-clone + atomic swap (see doc/rag_data_source_tree_plan_20260909.md §3.4).
//!
//! Storage is two-staged (§3.2):
//! - **Scan stage (temp)**: clone into `std::env::temp_dir()/mcphub-rag-git/
//!   {repo_hash}` — picking/cancelling never touches the app-data dir; the OS
//!   reclaims temp dirs, so no per-repo cleanup is needed for abandoned scans.
//! - **Persist stage (on confirm)**: when the user actually imports files,
//!   `ensure_persisted` copies the repo clone to `<app_data>/rag/git/
//!   {repo_hash}` (sibling of `rag/files`, outside the `.meta` scan). All
//!   doc update paths (md5 re-hash of `original_path`) read the persistent
//!   copy.
//!
//! Credentials live in a local file inside the app data dir
//! (`rag/git-credentials.json`, 0600 on unix, atomic replace) — never in doc
//! metas or logs. A local file (not the OS keyring) is deliberate: keyring
//! access can pop an authorization prompt on every auto-update tick. First
//! authenticated clone persists the credential so later batch/auto updates
//! pull without re-entry. No source registry is kept: re-importing a repo
//! always re-enters its URL (and credentials if changed).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{anyhow, Result};
use tauri::{Emitter, Manager};
use tokio::sync::Mutex;


/// Timeout for clone/fetch operations (a shallow clone should be way below
/// this; oversized repos are capped by the scan cap anyway).
const CLONE_TIMEOUT_SECS: u64 = 120;

// ── Directory layout ──────────────────────────────────────────────────────

/// Temp root for scan-stage clones: `temp_dir()/mcphub-rag-git/`.
pub fn temp_root() -> PathBuf {
    std::env::temp_dir().join("mcphub-rag-git")
}

/// Temp clone dir for one repo: `temp_root()/{repo_hash}`.
fn temp_repo_dir(repo_hash: &str) -> PathBuf {
    temp_root().join(repo_hash)
}

/// Public accessor for the temp dir of a repo hash (the upload command uses
/// it to locate the source of the copy into app-data).
pub fn temp_dir_for(repo_hash: &str) -> PathBuf {
    temp_repo_dir(repo_hash)
}

/// Persistent root: `<app_data>/rag/git/` (sibling of `rag/files`).
fn persistent_root(app: &tauri::AppHandle) -> Result<PathBuf> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| anyhow!("app data dir: {e}"))?
        .join("rag")
        .join("git");
    Ok(dir)
}

/// Persistent clone dir for one repo: `persistent_root()/{repo_hash}`.
fn persistent_repo_dir(app: &tauri::AppHandle, repo_hash: &str) -> Result<PathBuf> {
    Ok(persistent_root(app)?.join(repo_hash))
}

/// Public accessor for the persistent clone dir (the source-sync scan uses
/// it to detect files added/removed in a repo). `None` on app-data errors.
/// Persistent clone dir for a RAW url: canonicalizes first (http->https
/// redirect probe etc.), then hashes — the same identity `refresh_persistent`
/// / `clone_to_temp` use. Callers MUST go through this instead of hashing the
/// raw url themselves, or they will miss the clone dir whenever the canonical
/// form differs from what the frontend stored (e.g. http repos behind a
/// redirect) and silently skip add/remove sync for that source.
pub async fn persistent_repo_dir_for_url(app: &tauri::AppHandle, url: &str) -> Option<PathBuf> {
    let c = canonicalize_url(url).await;
    let hash = repo_hash(&c);
    persistent_repo_dir_public(app, &hash)
}

pub fn persistent_repo_dir_public(app: &tauri::AppHandle, repo_hash: &str) -> Option<PathBuf> {
    persistent_repo_dir(app, repo_hash).ok()
}

/// Stable, filesystem-safe dir name for a repo URL: lowercase hex md5 of the
/// trimmed URL. Non-security (local dir naming only); md-5 is already a dep.
/// Follow redirects once for http(s) URLs and return the FINAL url. Why:
/// some servers permanently redirect http -> https (308), and gix's reqwest
/// backend STRIPS the Authorization header when a redirect changes the
/// scheme — so creds sent over http are dropped on the https follow-up and
/// every retry repeats the dance until gix gives up ("InvalidCredentials" /
/// "smart protocol header" errors). Canonicalizing to the final scheme
/// before cloning sidesteps the whole class: the first (and only) request
/// goes out over https with auth attached.
/// Non-http URLs (ssh/scp) pass through unchanged. Network errors during the
/// probe are NON-FATAL (fall back to the original URL — the clone itself
/// will surface a proper error). Cached per origin for 10 min.
pub(crate) async fn canonicalize_url(url: &str) -> String {
    let u = url.trim();
    if !(u.starts_with("http://") || u.starts_with("https://")) {
        return u.to_string();
    }
    // cache hit?
    {
        let cache = REDIRECT_CACHE.get_or_init(|| async_utils_mutex_default());
        let cache = cache.lock().await;
        if let Some((at, final_url)) = cache.get(u) {
            if at.elapsed() < std::time::Duration::from_secs(600) {
                return final_url.clone();
            }
        }
    }
    let probe_url = format!(
        "{}/info/refs?service=git-upload-pack",
        u.trim_end_matches('/')
    );
    let owned = probe_url.clone();
    let res = tauri::async_runtime::spawn_blocking(move || {
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            // Bypass the system proxy (macOS/Windows): proxy apps (Clash etc.)
            // often mangle the git smart-protocol request even when the host
            // is in the bypass list — reqwest's system-proxy matcher mishandles
            // wildcard exceptions, and the proxy answers 404 for /info/refs.
            // The git URL is user-configured and meant to be direct.
            .no_proxy()
            .build()
            .ok()
            .and_then(|c| {
                // No credentials here — a 401 response is fine; we only care
                // about where the redirect LANDS (scheme canonicalization).
                c.get(&owned).send().ok().map(|r| r.url().to_string())
            })
    })
    .await
    .ok()
    .flatten();
    let final_url = match res {
        Some(final_url) => final_url,
        None => return u.to_string(),
    };
    // Derive the canonical repo base from the final /info/refs URL.
    let canonical_base = final_url
        .split("/info/refs")
        .next()
        .map(|s| s.to_string())
        .unwrap_or_else(|| u.to_string());
    if canonical_base != u.trim_end_matches('/') {
        let cache = REDIRECT_CACHE.get_or_init(|| async_utils_mutex_default());
        cache.lock().await.insert(
            u.to_string(),
            (std::time::Instant::now(), canonical_base.clone()),
        );
        canonical_base
    } else {
        u.to_string()
    }
}

static REDIRECT_CACHE: OnceLock<tokio::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, String)>>> = OnceLock::new();

fn async_utils_mutex_default() -> tokio::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, String)>> {
    tokio::sync::Mutex::new(std::collections::HashMap::new())
}

pub fn repo_hash(url: &str) -> String {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(url.trim().as_bytes());
    let d = h.finalize();
    d.iter().map(|b| format!("{b:02x}")).collect()
}

// ── URL handling ──────────────────────────────────────────────────────────

/// Validate the remote URL. Supports `http://`, `https://`, `ssh://` and
/// SCP-style `user@host:path.git` (gix parses both ssh forms). Reject URLs
/// with embedded passwords: for http(s) ANY userinfo is refused (the
/// credential form is the only path — avoids secret leakage into logs); for
/// ssh, `user@host` is the STANDARD login form (not a credential), only
/// `user:pass@` is refused (ssh URLs cannot carry passwords — auth goes
/// through the system ssh key/agent).
pub fn validate_url(url: &str) -> Result<()> {
    let u = url.trim();
    if u.is_empty() {
        return Err(anyhow!("repository URL is empty"));
    }
    if let Some(after_scheme) = u.strip_prefix("ssh://") {
        let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
        if authority.is_empty() {
            return Err(anyhow!("ssh URL is missing a host (got: {})", u));
        }
        if let Some(at) = authority.rfind('@') {
            let userinfo = &authority[..at];
            if userinfo.contains(':') {
                return Err(anyhow!(
                    "SSH URL with an embedded password is not allowed; configure an SSH key instead"
                ));
            }
        }
        return Ok(());
    }
    let after_scheme = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"));
    if let Some(after_scheme) = after_scheme {
        // userinfo in URL = embedded credentials -> reject (any `user[:pass]@`
        // form — injecting our own creds over an existing userinfo would produce
        // a malformed URL, and the credential form is the only supported path).
        let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
        if authority.contains('@') {
            return Err(anyhow!(
                "URL with embedded credentials is not allowed; use the username/password fields"
            ));
        }
        return Ok(());
    }
    // SCP-style: `[user@]host:path`. Require an explicit `user@` so local /
    // Windows paths (`C:\...`, `foo:bar`) can never be mistaken for remotes.
    if is_scp_style(u) {
        let authority = u.split(':').next().unwrap_or("");
        let userinfo = authority.rsplit('@').next().unwrap_or("");
        if userinfo.contains(':') {
            return Err(anyhow!(
                "SSH URL with an embedded password is not allowed; configure an SSH key instead"
            ));
        }
        return Ok(());
    }
    Err(anyhow!(
        "only http://, https://, ssh:// or user@host:path URLs are supported (got: {})",
        u
    ))
}

/// SCP-style remote detection: `[user@]host:path` — a `:` before any `/` AND
/// an `@` before that `:` (the explicit user is required, see `validate_url`).
fn is_scp_style(u: &str) -> bool {
    let colon = match u.find(':') {
        Some(c) => c,
        None => return false,
    };
    if u[..colon].contains('/') {
        return false;
    }
    u[..colon].contains('@')
}

/// Minimal percent-encoding for username/password embedded in the clone URL
/// (RFC 3986 unreserved set kept as-is, everything else escaped). Avoids
/// pulling in a URL crate for a handful of bytes.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Build the clone URL: inject credentials into userinfo when provided
/// (gix extracts userinfo for HTTP basic auth). Token-as-password style:
/// callers pass username + password(token) from the credential form.
/// SSH URLs (ssh:// or SCP-style) are returned unchanged — gix's ssh
/// transport spawns the system `ssh` program, whose key/agent handles auth;
/// a URL `user@` is the ssh login name, never a basic-auth credential.
fn build_clone_url(url: &str, username: Option<&str>, password: Option<&str>) -> String {
    let u = url.trim();
    if u.starts_with("ssh://") || is_scp_style(u) {
        // SSH: no password injection (auth goes through the system ssh
        // program — keys, or the askpass bridge for password-auth servers).
        // A URL `user@` is the ssh LOGIN NAME — and on password-auth servers
        // (self-hosted Gitea/GitLab etc.) the real login account is the
        // USER'S account, not the `git` convention. The form username is
        // authoritative: inject when absent, OVERRIDE when the URL carries
        // a different one (`git@host` is just a convention, often wrong for
        // password auth). Key-auth servers (GitHub) ignore it only when the
        // form leaves username empty.
        if let Some(user) = username.filter(|u| !u.trim().is_empty()) {
            let user = user.trim();
            if let Some(rest) = u.strip_prefix("ssh://") {
                let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
                let (authority, tail) = rest.split_at(authority_end);
                let host_part = match authority.rfind('@') {
                    Some(at) => &authority[at + 1..],
                    None => authority,
                };
                if !host_part.is_empty() {
                    return format!("ssh://{}@{}{}", user, host_part, tail);
                }
            }
            // SCP-style `user@host:path`: swap the userinfo too.
            if is_scp_style(u) {
                let colon = u.find(':').unwrap_or(u.len());
                let (authority, path) = u.split_at(colon);
                let host_part = authority.rsplit('@').next().unwrap_or(authority);
                if !host_part.is_empty() {
                    return format!("{}@{}{}", user, host_part, path);
                }
            }
        }
        return u.to_string();
    }
    match (username.filter(|u| !u.trim().is_empty()), password.filter(|p| !p.is_empty())) {
        (Some(user), pass) => {
            let creds = match pass {
                Some(p) => format!("{}:{}", percent_encode(user.trim()), percent_encode(p)),
                None => percent_encode(user.trim()),
            };
            // <scheme>://<creds>@host/... — userinfo carries basic auth for
            // gix HTTP. Scheme (http/https) is preserved from the input URL.
            let after_scheme = u
                .strip_prefix("https://")
                .or_else(|| u.strip_prefix("http://"))
                .unwrap_or(u);
            let scheme = if u.starts_with("http://") { "http" } else { "https" };
            format!("{scheme}://{}@{}", creds, after_scheme)
        }
        _ => u.to_string(),
    }
}

// ── Clone / refresh (gix, blocking — call via spawn_blocking) ─────────────

/// Event payload for `rag://git-clone-progress` (clone download progress +
/// speed; frontend shows a bar under the 拉取按钮 during git pick).
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCloneProgress {
    /// Bytes received so far.
    received: u64,
    /// Expected total bytes (0 = unknown → indeterminate bar).
    total: u64,
    /// Smoothed transfer speed in bytes/s (EMA over samples).
    speed: u64,
}

const GIT_CLONE_PROGRESS_EVENT: &str = "rag://git-clone-progress";

/// Shared byte counters written by the gix progress impl (blocking clone
/// thread) and sampled by the async emitter task. `total == 0` = unknown.
#[derive(Default)]
pub(crate) struct CloneByteCounters {
    /// Backing store for `Progress::counter()` — gix fetch paths may bump
    /// it directly (pack reads); byte-only writes are additionally gated by
    /// the per-task `enabled` flag in `CloneProgress`.
    received: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    total: std::sync::atomic::AtomicU64,
}

impl CloneByteCounters {
    fn snapshot(&self) -> (u64, u64) {
        (
            self.received
                .load(std::sync::atomic::Ordering::Relaxed) as u64,
            self.total.load(std::sync::atomic::Ordering::Relaxed),
        )
    }
}

/// gix `NestedProgress` impl accumulating **byte** counts only: gix inits a
/// child progress per fetch task with its own unit (objects, refs, …) — only
/// tasks inited with the `Bytes` unit contribute to the shared counters.
/// Bytes detection is by unit hash: the unit is an opaque struct (private
/// kind), but `progress::bytes()` (the same helper gix's pack reader uses)
/// hashes identically to any bytes unit seen at `init`.
struct CloneProgress {
    counters: std::sync::Arc<CloneByteCounters>,
    /// Set by `init` when this task counts bytes.
    enabled: std::sync::atomic::AtomicBool,
}

impl CloneProgress {
    fn new(counters: std::sync::Arc<CloneByteCounters>) -> Self {
        Self {
            counters,
            enabled: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn unit_is_bytes(unit: &Option<gix::progress::Unit>) -> bool {
        use std::hash::{Hash, Hasher};
        let Some(u) = unit else { return false };
        let reference = gix::progress::bytes();
        let Some(reference) = reference else {
            return false;
        };
        let mut a = std::collections::hash_map::DefaultHasher::new();
        u.hash(&mut a);
        let mut b = std::collections::hash_map::DefaultHasher::new();
        reference.hash(&mut b);
        a.finish() == b.finish()
    }
}

impl gix::Progress for CloneProgress {
    fn init(&mut self, max: Option<gix::progress::Step>, unit: Option<gix::progress::Unit>) {
        let is_bytes = Self::unit_is_bytes(&unit);
        self.enabled
            .store(is_bytes, std::sync::atomic::Ordering::Relaxed);
        if is_bytes {
            self.counters
                .received
                .store(0, std::sync::atomic::Ordering::Relaxed);
            self.counters
                .total
                .store(max.unwrap_or(0) as u64, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn unit(&self) -> Option<gix::progress::Unit> {
        None
    }

    fn set_name(&mut self, _name: String) {}
    fn name(&self) -> Option<String> {
        None
    }
    fn id(&self) -> gix::progress::Id {
        [0; 4]
    }

    fn message(&self, _level: gix::progress::MessageLevel, _message: String) {}
}

impl gix::progress::Count for CloneProgress {
    fn set(&self, step: gix::progress::Step) {
        if self
            .enabled
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            self.counters
                .received
                .store(step, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn step(&self) -> gix::progress::Step {
        self.counters
            .received
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    fn inc_by(&self, step: gix::progress::Step) {
        if self
            .enabled
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            self.counters
                .received
                .fetch_add(step, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn inc(&self) {
        self.inc_by(1);
    }

    fn counter(&self) -> std::sync::Arc<std::sync::atomic::AtomicUsize> {
        self.counters.received.clone()
    }
}

impl gix::NestedProgress for CloneProgress {
    type SubProgress = CloneProgress;

    fn add_child(&mut self, _name: impl Into<String>) -> Self::SubProgress {
        CloneProgress::new(self.counters.clone())
    }

    fn add_child_with_id(
        &mut self,
        name: impl Into<String>,
        _id: gix::progress::Id,
    ) -> Self::SubProgress {
        self.add_child(name)
    }
}

/// Sample the clone counters every 250ms and emit progress events (with an
/// EMA-smoothed speed) until `done` flips. Runs on the async runtime; the
/// blocking clone thread never does I/O for progress.
async fn emit_clone_progress(
    app: tauri::AppHandle,
    counters: std::sync::Arc<CloneByteCounters>,
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    const SAMPLE_MS: u64 = 120;
    let mut last_received: u64 = 0;
    let mut speed_ema: f64 = 0.0;
    let mut last_ts = std::time::Instant::now();
    let mut iv = tokio::time::interval(std::time::Duration::from_millis(SAMPLE_MS));
    iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // First tick of `interval` fires immediately — emit the initial (0-byte)
    // sample so the UI switches into progress mode without waiting.
    loop {
        tokio::select! {
            _ = iv.tick() => {}
        }
        let (received, total) = counters.snapshot();
        let dt = last_ts.elapsed().as_secs_f64();
        if dt > 0.05 && received >= last_received {
            let inst = (received - last_received) as f64 / dt;
            speed_ema = if speed_ema == 0.0 { inst } else { speed_ema * 0.7 + inst * 0.3 };
        }
        last_received = received;
        last_ts = std::time::Instant::now();
        if let Err(e) = app.emit(
            GIT_CLONE_PROGRESS_EVENT,
            &GitCloneProgress {
                received,
                total,
                speed: speed_ema as u64,
            },
        ) {
            log::warn!("[rag-git] emit clone-progress failed: {e}");
        }
        if done.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
    }
}



/// Shallow-clone `url` into `dest` (depth, branch optional) and return the
/// checked-out HEAD commit sha. Blocking — must run inside spawn_blocking.
pub(crate) fn clone_blocking(
    url: &str,
    dest: &Path,
    branch: Option<&str>,
    depth: u32,
    ssh_password: Option<&str>,
    counters: std::sync::Arc<CloneByteCounters>,
) -> Result<String> {
    let depth = depth.clamp(1, 100);
    // The dest must not exist for gix clone (it creates it).
    let _ = std::fs::remove_dir_all(dest);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let url = gix::url::parse(url.as_bytes().into())?;
    // SSH remotes: install a ssh wrapper (stderr filter + optional askpass
    // for password-auth servers) and point the clone at it via
    // `core.sshCommand`. The wrapper dir is unique per clone (concurrent
    // clones may carry different passwords) and removed when this call ends.
    struct WrapperCleanup(std::path::PathBuf);
    impl Drop for WrapperCleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let mut _wrapper_cleanup: Option<WrapperCleanup> = None;
    let open_opts = gix::open::Options::default();
    // Disable gix's own interactive credential prompt (gix-prompt opens
    // /dev/tty when the credential cascade comes up empty — e.g. http 401
    // with no/incorrect stored creds). With prompts disabled the handshake
    // fails fast as EmptyCredentials/PermissionDenied, which we map to
    // GIT_AUTH_REQUIRED so the FRONTEND asks for credentials instead of the
    // console. (For SSH our wrapper already keeps ssh off the TTY; this is
    // the http-path equivalent and a belt-and-braces for both.)
    let mut overrides: Vec<String> = vec!["gitoxide.credentials.terminalPrompt=false".into()];
    if url.scheme == gix::url::Scheme::Ssh {
        if let Some(w) = ensure_ssh_wrapper(ssh_password)? {
            overrides.push(format!("core.sshCommand={}", w.command.display()));
            _wrapper_cleanup = Some(WrapperCleanup(w.dir));
        }
    }
    let open_opts = open_opts.config_overrides(overrides);
    let prep = gix::clone::PrepareFetch::new(
        url,
        dest,
        gix::create::Kind::WithWorktree,
        gix::create::Options::default(),
        open_opts,
    )
    .map_err(|e| anyhow!("clone init: {e}"))?;
    let prep = if let Some(b) = branch.filter(|b| !b.trim().is_empty()) {
        // ref_name selects which branch to fetch & check out (NOT the remote name).
        prep.with_ref_name(Some(b.trim()))
            .map_err(|e| anyhow!("branch: {e}"))?
    } else {
        prep
    };
    // Depth-at-remote shallow negotiation; 1 = only the branch tip commit.
    let mut prep = prep.with_shallow(gix::remote::fetch::Shallow::DepthAtRemote(
        std::num::NonZeroU32::new(depth).expect("depth clamped >= 1"),
    ));
    // Fetch + checkout the main worktree (fetch_only alone leaves the tree empty).
    let (mut prep_checkout, _outcome) = prep
        .fetch_then_checkout(
            CloneProgress::new(counters),
            &std::sync::atomic::AtomicBool::new(false),
        )
        .map_err(|e| anyhow!("fetch: {e:#}"))?;
    let (repo, _checkout) = prep_checkout
        .main_worktree(gix::progress::Discard, &std::sync::atomic::AtomicBool::new(false))
        .map_err(|e| anyhow!("checkout: {e}"))?;
    let head = repo
        .head_commit()
        .map_err(|e| anyhow!("head: {e}"))?
        .id
        .to_string();
    Ok(head)
}

/// Paths of the per-clone ssh wrapper bundle.
pub(crate) struct SshWrapper {
    /// Directory holding the wrapper + askpass (removed after the clone).
    pub(crate) dir: std::path::PathBuf,
    /// Wrapper path to hand to `core.sshCommand` (file name MUST be `ssh`:
    /// gix derives its argument/error style from the basename, and a
    /// non-`ssh` name downgrades to the "Simple" kind which cannot pass
    /// ports and loses error classification).
    pub(crate) command: std::path::PathBuf,
}

/// Write the ssh wrapper used for SSH remotes and return its paths. The
/// wrapper (POSIX sh):
/// 1. filters benign stderr lines gix misreads as connection errors —
///    `nc -v` (common ProxyCommand in ~/.ssh/config, e.g. github-over-443
///    proxy setups) prints `Connection to ... succeeded!` on SUCCESS, which
///    gix's per-line heuristic would treat as a failed handshake;
/// 2. auto-accepts new host keys (`StrictHostKeyChecking=accept-new`) — the
///    spawned ssh has no TTY, so an interactive confirm would hang/fail;
/// 3. DETACHES ssh from the controlling terminal (`setsid` when available) —
///    without this, ssh with a live SSH_AUTH_SOCK/agent env still opens
///    /dev/tty directly for password prompts, printing
///    "user@host's password:" into the app console and hanging forever
///    (no TTY input can arrive). With /dev/tty unreachable, ssh falls back
///    to the SSH_ASKPASS helper, which surfaces failures as a normal error
///    (and the frontend then shows the credential form);
/// 4. when `password` is set (server does password auth, no local key), an
///    SSH_ASKPASS helper is installed that answers EVERY prompt (server
///    kbdint prompts may be localized) with the password. OpenSSH 8.4+
///    honors `SSH_ASKPASS_REQUIRE=force`; `DISPLAY` is set for older
///    versions that only consult askpass without a TTY when DISPLAY exists.
///
/// The bundle lives in a UNIQUE temp dir (concurrent clones may carry
/// different passwords) with 0700 perms; the askpass file is 0600 and the
/// caller removes the whole dir after the clone (WrapperCleanup).
/// Windows has no POSIX shell — return None (plain ssh directly; no
/// stderr filter, no password askpass; key auth still works).
fn ensure_ssh_wrapper(password: Option<&str>) -> Result<Option<SshWrapper>> {
    if cfg!(windows) {
        return Ok(None);
    }
    let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
    let dir = std::env::temp_dir().join(format!("mcphub-ssh-{}-{}", std::process::id(), nanos));
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }

    // Optional askpass helper (password auth). The password is embedded with
    // classic single-quote escaping so any byte sequence round-trips.
    let askpass_line = match password.filter(|p| !p.is_empty()) {
        Some(p) => {
let escaped = p.replace('\'', "'\''");
            let path = dir.join("askpass.sh");
            std::fs::write(
                &path,
                format!(
                    "#!/bin/sh
# MCPHub Desktop ssh askpass (auto-generated; do not edit).
# Answer EVERY prompt with the password: in this controlled invocation
# the only interactive prompts are password/keyboard-interactive (host
# keys are auto-accepted, the username comes from the URL) and server
# kbdint prompts may be localized, which a *assword* guard would miss.
# The PROMPT TEXT (never the answer) is logged for diagnostics.
printf '%s' \"$1\" >> \"$MCPHUB_SSH_DEBUG_LOG\" 2>/dev/null || true
printf '%s\\n' '{escaped}'
"
                ),
            )?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                // MUST be executable: ssh exec()s the askpass program. A non-executable
                // file makes ssh print "ssh_askpass: exec(...): Permission denied",
                // which gix's stderr classifier reads as an auth rejection.
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
            }
            format!("export SSH_ASKPASS={}
export SSH_ASKPASS_REQUIRE=force
export DISPLAY=dummy:0
", path.display())
        }
        None => String::new(),
    };

    let command = dir.join("ssh");
    // Without a password there is nothing to answer prompts WITH: add
    // BatchMode so ssh fails immediately ("Permission denied") instead of
    // opening /dev/tty and hanging forever waiting for input that can never
    // arrive — the frontend then shows the credential form. With a password,
    // askpass answers prompts; NumberOfPasswordPrompts=1 fails fast on a
    // wrong password instead of re-prompting.
    let batch_line = if password.filter(|p| !p.is_empty()).is_some() {
        ""
    } else {
        "OPTS=\"$OPTS -o BatchMode=yes\"\n"
    };
    let script = format!(
        r#"#!/bin/sh
# MCPHub Desktop ssh wrapper (auto-generated; do not edit).
# 1) gix misreads `Connection to ... succeeded` lines (nc -v ProxyCommand
#    output) as connection errors - filter those benign lines, forward the rest.
# 2) No TTY: auto-accept new host keys instead of hanging on the confirm prompt.
OPTS='-o StrictHostKeyChecking=accept-new -o NumberOfPasswordPrompts=1'
{batch_line}{askpass_line}export MCPHUB_SSH_DEBUG_LOG="${{TEMPDIR:-/tmp}}/mcphub-ssh-debug.log"
: >> "$MCPHUB_SSH_DEBUG_LOG" 2>/dev/null || MCPHUB_SSH_DEBUG_LOG=/dev/null
case "$(uname -s)" in
  Darwin) UNBUF='-l' ;;
  *) UNBUF='-u' ;;
esac
{{ (command -v setsid >/dev/null 2>&1 && setsid ssh $OPTS "$@" || ssh $OPTS "$@") 2>&1 1>&3 | tee "$MCPHUB_SSH_DEBUG_LOG" | sed $UNBUF -e '/Connection to .* succeeded/d' >&2; }} 3>&1
echo "SSH EXIT: $?" >> "$MCPHUB_SSH_DEBUG_LOG" 2>/dev/null
"#
    );
    std::fs::write(&command, script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(Some(SshWrapper { dir, command }))
}

/// Async wrapper with timeout: shallow clone into `dest`.
async fn clone_async(
    url: &str,
    dest: PathBuf,
    branch: Option<&str>,
    depth: u32,
    ssh_password: Option<&str>,
    counters: std::sync::Arc<CloneByteCounters>,
) -> Result<String> {
    let url = url.to_string();
    let branch = branch.map(|s| s.to_string());
    let ssh_password = ssh_password.map(|s| s.to_string());
    let fut = tauri::async_runtime::spawn_blocking(move || {
        clone_blocking(&url, &dest, branch.as_deref(), depth, ssh_password.as_deref(), counters)
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(CLONE_TIMEOUT_SECS),
        async move {
            fut.await
                .map_err(|e| anyhow!("clone task: {e}"))?
        },
    )
    .await
    .map_err(|_| anyhow!("clone timed out after {CLONE_TIMEOUT_SECS}s"))?
}

/// Classify a clone/fetch error as authentication-required (so the frontend
/// can surface the credential form instead of a generic failure). gix's
/// reqwest backend maps 401 to an io error "Received HTTP status 401", and
/// GitHub/GitLab answer anonymous requests to private repos with 404
/// "repository not found" (deliberately — don't leak existence). Match
/// specific phrases rather than bare "401"/"403" substrings: an error text
/// that merely CONTAINS the remote URL (e.g. repo named `issue-1401`) would
/// otherwise be misread as an auth failure.
pub fn is_auth_error(err: &anyhow::Error) -> bool {
        let msg = format!("{err:#}").to_lowercase();
        [
            "http status 401",
            "http status 403",
            "unauthorized",
            "forbidden",
            "authentication",
            "access denied",
            "could not read username",
            "terminal prompts disabled",
            "repository not found",
            "repo not found",
            // gix's handshake maps a 401 to a credential-helper roundtrip; with no
            // helper configured (our case — creds come from the form) it fails
            // with these texts BEFORE any "401" string survives in the chain:
            "no credentials were returned",
            "credential helper isn't functioning",
            "credentials provided",
            "were not accepted by the remote",
            "failed to obtain credentials",
            // SSH (gix spawns the system ssh program; these come from its
            // stderr): key rejected / agent absent, and the generic
            // "no access rights" suffix ssh prints after auth failures.
            "permission denied (publickey",
            "permission denied (password",
            "permission denied (keyboard-interactive",
            "host key verification failed",
            "could not read from remote repository",
        ]
        .iter()
        .any(|needle| msg.contains(needle))
}

/// Structured sentinel the frontend matches on: `GIT_AUTH_REQUIRED:<detail>`.
/// Generic failures stay plain `anyhow` errors (prefix-free).
pub fn auth_error(detail: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("GIT_AUTH_REQUIRED:{detail}")
}

// ── Public async API ──────────────────────────────────────────────────────

/// Global pick/clone lock. `clone_to_temp` wipes the WHOLE temp root before
/// cloning (crashed-session cleanup), so two concurrent picks (double Enter
/// before the spinner state lands, or a pick racing a slow previous one)
/// would destroy each other's clone. Serializing picks is fine — they're
/// rare, user-initiated operations. Refreshes are NOT under this lock (they
/// have their own per-repo locks in service.rs and never wipe the temp root).
static PICK_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

// ── Pick cancellation ─────────────────────────────────────────────────────
// "Cancel fetch" from the UI: per-URL notify tokens. The blocking gix clone
// thread can't be interrupted mid-syscall, so cancellation = abandon the
// wait (select!), let the thread finish into the void (its result is
// dropped), and clean up the temp clone it leaves behind. A re-pick of the
// same URL serializes behind PICK_LOCK and wipes the temp root anyway.

/// Aborted URLs (canonical form) -> notify token. Also used to short-circuit
/// a pick that is queued behind PICK_LOCK when the user already cancelled.
#[derive(Default)]
pub(crate) struct AbortToken {
    notify: tokio::sync::Notify,
    /// Set at signal time alongside `notify_waiters` — closes the race where
    /// the signal fires between token registration and the clone's `select!`
    /// (Notify::notify_waiters only wakes already-registered waiters).
    cancelled: std::sync::atomic::AtomicBool,
}

static PICK_ABORTS: OnceLock<tokio::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<AbortToken>>>> = OnceLock::new();

fn aborts() -> &'static tokio::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<AbortToken>>> {
    PICK_ABORTS.get_or_init(|| tokio::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Register a cancellation token for `url` (creates if absent). Returns the
/// token to watch.
pub(crate) async fn register_abort(url: &str) -> std::sync::Arc<AbortToken> {
    let mut m = aborts().lock().await;
    m.entry(url.trim().to_string())
        .or_insert_with(Default::default)
        .clone()
}

/// Signal cancellation for `url`; returns true if a token existed (i.e. a
/// pick may be in flight).
pub(crate) async fn signal_abort(url: &str) -> bool {
    let m = aborts().lock().await;
    if let Some(t) = m.get(url.trim()) {
        t.cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
        t.notify.notify_waiters();
        true
    } else {
        false
    }
}

/// Cancel entry used by the command layer: the pick registers its token
/// under the **canonicalized** URL (http→https upgrade, trailing-slash trim),
/// so the raw URL the frontend passes may not match. Canonicalize first (a
/// 10-min redirect cache makes this instant for known repos), then fall back
/// to the raw key.
pub(crate) async fn signal_abort_canonical(url: &str) -> bool {
    let canonical = canonicalize_url(url).await;
    if signal_abort(&canonical).await {
        return true;
    }
    if canonical != url.trim() {
        return signal_abort(url).await;
    }
    false
}

/// Consume the token after the pick ends (success, error, or cancel) so a
/// stale signal can't hit the next pick.
async fn unregister_abort(url: &str) {
    aborts().lock().await.remove(url.trim());
}

pub struct GitSyncResult {
    /// Clone dir (temp or persistent, per the variant).
    pub dir: PathBuf,
    /// HEAD commit sha after the fetch (display only).
    pub commit: String,
}

/// Scan-stage clone: into the **temp** dir. Wipes the whole temp root first
/// (one root, OS-reclaimed — abandoned scans from crashed sessions don't
/// linger past the next clone). Returns (temp_dir, commit).
pub async fn clone_to_temp(
    app: Option<&tauri::AppHandle>,
    url: &str,
    branch: Option<&str>,
    username: Option<&str>,
    password: Option<&str>,
    depth: u32,
) -> Result<GitSyncResult> {
    // Serialize picks: the temp-root wipe below is destructive to any
    // concurrent pick's clone.
    let _pick_guard = PICK_LOCK.get_or_init(|| Mutex::new(())).lock().await;
    validate_url(url)?;
    // Canonicalize http->https redirects BEFORE hashing: one canonical repo
    // identity (and no auth-strip dance across the scheme change).
    let url_owned;
    let url = {
        let c = canonicalize_url(url).await;
        url_owned = c;
        url_owned.as_str()
    };
    // Register the cancellation token AFTER canonicalization (the canonical
    // url is the cancel key the command layer passes back).
    let abort = register_abort(url).await;
    use std::sync::atomic::Ordering;
    // Queued behind PICK_LOCK: if the user already cancelled while waiting,
    // bail before touching the temp root. The flag check (not just
    // notified()) closes the lost-signal race: a cancel fired between
    // register and here is still visible via `cancelled`.
    if abort.cancelled.load(Ordering::SeqCst) {
        unregister_abort(url).await;
        return Err(anyhow!("PICK_CANCELLED"));
    }

    let hash = repo_hash(url);
    let root = temp_root();
    // Wipe the temp root (not just this repo's dir): one repo scans at a
    // time in practice, and stale roots from crashed sessions die here too.
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root)?;
    let dest = temp_repo_dir(&hash);
    let clone_url = build_clone_url(url, username, password);
    // Progress sampling: emit `rag://git-clone-progress` while the clone
    // runs (pick flow only — app is None on update-stage refreshes).
    let counters = std::sync::Arc::new(CloneByteCounters::default());
    let done_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let progress_task = match app {
        Some(a) => Some(tauri::async_runtime::spawn(emit_clone_progress(
            a.clone(),
            counters.clone(),
            done_flag.clone(),
        ))),
        None => None,
    };
    let result = tokio::select! {
        r = clone_async(&clone_url, dest.clone(), branch, depth, password, counters.clone()) => Some(r),
        _ = async {
            // Wake on notify OR on the flag (covers signals that arrived
            // before this select started registering its waker).
            loop {
                if abort.cancelled.load(Ordering::SeqCst) {
                    return;
                }
                let n = abort.notify.notified();
                tokio::pin!(n);
                // enable racing: register waker, then re-check the flag so a
                // signal landing between load and waker-registration isn't lost.
                n.as_mut().enable();
                if abort.cancelled.load(Ordering::SeqCst) {
                    return;
                }
                n.await;
            }
        } => None, // cancelled: abandon the clone wait
    };
    unregister_abort(url).await;
    done_flag.store(true, std::sync::atomic::Ordering::Relaxed);
    // Terminal sample: emit the exact final byte count (100% when the total
    // is known) so the UI doesn't freeze at the last 120ms snapshot.
    if let Some(a) = app {
        let (received, total) = counters.snapshot();
        let _ = a.emit(
            GIT_CLONE_PROGRESS_EVENT,
            &GitCloneProgress {
                received,
                total,
                speed: 0,
            },
        );
    }
    if let Some(t) = progress_task {
        let _ = t.await;
    }
    match result {
        Some(Ok(commit)) => Ok(GitSyncResult { dir: dest, commit }),
        Some(Err(e)) if is_auth_error(&e) => Err(auth_error(e)),
        Some(Err(e)) => Err(e),
        // Cancelled: clean the half-written temp clone so nothing lingers;
        // the next pick wipes the whole root anyway.
        None => {
            let _ = std::fs::remove_dir_all(&dest);
            Err(anyhow!("PICK_CANCELLED"))
        }
    }
}

/// Copy a directory tree recursively (create dest, walk entries). Used by
/// `ensure_persisted`; `walkdir`-free to keep deps flat.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let target = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Persist-stage: make sure the repo's clone exists under the app-data dir,
/// copying from the temp clone on first import. Idempotent — returns the
/// persistent dir either way. The `src` (temp dir) must still exist for the
/// first call; later calls short-circuit on the persistent dir.
pub fn ensure_persisted(app: &tauri::AppHandle, repo_hash: &str, temp_dir: &Path) -> Result<PathBuf> {
    let dst = persistent_repo_dir(app, repo_hash)?;
    if dst.exists() {
        return Ok(dst);
    }
    if !temp_dir.exists() {
        return Err(anyhow!("temp clone for repo {} is gone; re-fetch the repo", repo_hash));
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    copy_dir_recursive(temp_dir, &dst)?;
    Ok(dst)
}

/// Rewrite a path inside the temp clone root to its persistent counterpart
/// (prefix swap by repo hash dir). Returns None when `path` isn't under the
/// temp root. Used by the upload command so `original_path` recorded in doc
/// metas points at the persistent copy (survives temp cleanup).
pub fn map_temp_to_persistent(app: &tauri::AppHandle, path: &str) -> Result<Option<(String, String)>> {
    let temp_root = temp_root();
    let p = PathBuf::from(path);
    let Ok(rel) = p.strip_prefix(&temp_root) else {
        return Ok(None);
    };
    // rel = {repo_hash}/... — first segment is the repo hash dir.
    let Some(first) = rel.iter().next() else {
        return Ok(None);
    };
    let hash = first.to_string_lossy().to_string();
    let rest: PathBuf = rel.iter().skip(1).collect();
    let persisted = persistent_repo_dir(app, &hash)?;
    let mapped = if rest.as_os_str().is_empty() {
        persisted
    } else {
        persisted.join(rest)
    };
    Ok(Some((hash, mapped.to_string_lossy().into_owned())))
}

/// Update-stage fetch into the **persistent** dir: re-clone to
/// `{repo_hash}.new` and atomically swap (`.new` → main, main → `.old` →
/// delete). Cheap at depth 1 and immune to dirty-tree / shallow-fetch edge
/// cases. Returns (persistent_dir, commit).
pub async fn refresh_persistent(
    app: &tauri::AppHandle,
    url: &str,
    branch: Option<&str>,
    username: Option<&str>,
    password: Option<&str>,
    depth: u32,
) -> Result<GitSyncResult> {
    validate_url(url)?;
    let url_owned;
    let url = {
        let c = canonicalize_url(url).await;
        url_owned = c;
        url_owned.as_str()
    };
    let hash = repo_hash(url);
    let main_dir = persistent_repo_dir(app, &hash)?;
    let new_dir = persistent_root(app)?.join(format!("{hash}.new"));
    let old_dir = persistent_root(app)?.join(format!("{hash}.old"));
    let clone_url = build_clone_url(url, username, password);
    let commit = match clone_async(
        &clone_url,
        new_dir.clone(),
        branch,
        depth,
        password,
        std::sync::Arc::new(CloneByteCounters::default()),
    )
    .await
    {
        Ok(c) => c,
        Err(e) if is_auth_error(&e) => return Err(auth_error(e)),
        Err(e) => return Err(e),
    };
    // Atomic-ish swap: main -> .old, .new -> main, delete .old. If the final
    // rename fails, roll `.old` back to main so the previous clone keeps
    // serving (docs' original_path stays valid) instead of leaving main
    // missing until the next successful refresh.
    if main_dir.exists() {
        let _ = std::fs::remove_dir_all(&old_dir);
        std::fs::rename(&main_dir, &old_dir)?;
    }
    if let Err(e) = std::fs::rename(&new_dir, &main_dir) {
        if old_dir.exists() {
            let _ = std::fs::rename(&old_dir, &main_dir);
        }
        let _ = std::fs::remove_dir_all(&new_dir);
        return Err(anyhow!("swap new clone into place failed: {e}"));
    }
    let _ = std::fs::remove_dir_all(&old_dir);
    Ok(GitSyncResult { dir: main_dir, commit })
}

/// Update-stage refresh for a repo: resolve its credential from the local
/// credential file (if previously stored) and re-clone + swap the persistent dir.
/// Auth failures map to `GIT_AUTH_REQUIRED:` so callers can distinguish
/// "needs credentials" (skip + log for auto; manual flow may prompt) from
/// other network errors. No branch stored (registry removed) — re-clone
/// defaults to the remote HEAD, which matches the single-branch import flow.
pub async fn refresh_registered(app: &tauri::AppHandle, url: &str) -> Result<GitSyncResult> {
    let url_owned;
    let url = {
        let c = canonicalize_url(url).await;
        url_owned = c;
        url_owned.as_str()
    };
    let hash = repo_hash(url);
    let creds = load_credential(app, &hash);
    let (username, password) = match creds {
        Some((u, p)) => (Some(u), Some(p)),
        None => (None, None),
    };
    refresh_persistent(app, url, None, username.as_deref(), password.as_deref(), 1).await
}

/// Startup sweep: remove `.old` / `.new` leftovers from a crash mid-swap.
/// Cheap (one dir read of the persistent root); call from lib.rs setup.
pub fn sweep_stale_dirs(app: &tauri::AppHandle) {
    if let Ok(root) = persistent_root(app) {
        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".old") || name.ends_with(".new") {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
    }
}

/// Remove one repo's persistent clone (引用计数归零时由 delete 路径调用)。
pub fn remove_persistent(app: &tauri::AppHandle, repo_hash: &str) -> Result<()> {
    let dir = persistent_repo_dir(app, repo_hash)?;
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

// ── Keyring credentials (per-repo, local to this machine) ────────────────

// ── Local credential file storage ─────────────────────────────────────────

fn creds_lock() -> &'static std::sync::RwLock<()> {
    static LOCK: std::sync::OnceLock<std::sync::RwLock<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::RwLock::new(()))
}

/// `<app_data>/rag/git-credentials.json` — map of repo hash → credential.
/// Local file instead of the OS keyring: keyring reads pop authorization
/// prompts on macOS/Windows, which breaks unattended auto-update.
fn credentials_path(app: &tauri::AppHandle) -> Result<PathBuf> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| anyhow!("app data dir: {e}"))?
        .join("rag")
        .join("git-credentials.json"))
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
struct StoredCredential {
    username: String,
    password: String,
}

fn read_credentials(app: &tauri::AppHandle) -> std::collections::HashMap<String, StoredCredential> {
    let _guard = creds_lock().read().unwrap();
    let Ok(path) = credentials_path(app) else {
        return Default::default();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return Default::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

/// Store the credential for a repo hash (insert-or-replace, atomic write).
pub fn store_credential(app: &tauri::AppHandle, repo_hash: &str, username: &str, password: &str) -> Result<()> {
    let _guard = creds_lock().write().unwrap();
    let path = credentials_path(app)?;
    let mut creds: std::collections::HashMap<String, StoredCredential> = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    creds.insert(
        repo_hash.to_string(),
        StoredCredential {
            username: username.to_string(),
            password: password.to_string(),
        },
    );
    write_credentials_nolock(&creds, &path)
}

fn write_credentials_nolock(
    creds: &std::collections::HashMap<String, StoredCredential>,
    path: &PathBuf,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| anyhow!("create dir: {e}"))?;
    }
    let json = serde_json::to_vec_pretty(creds)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json).map_err(|e| anyhow!("write creds tmp: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path).map_err(|e| anyhow!("rename creds: {e}"))
}

/// Load the persisted credential for a repo (None = not stored / unreadable).
pub fn load_credential(app: &tauri::AppHandle, repo_hash: &str) -> Option<(String, String)> {
    let c = read_credentials(app).get(repo_hash)?.clone();
    Some((c.username, c.password))
}

/// Remove the stored credential for a repo hash (no-op when absent).
pub fn delete_credential(app: &tauri::AppHandle, repo_hash: &str) -> Result<()> {
    let _guard = creds_lock().write().unwrap();
    let path = credentials_path(app)?;
    let mut creds: std::collections::HashMap<String, StoredCredential> = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    if creds.remove(repo_hash).is_none() {
        return Ok(());
    }
    write_credentials_nolock(&creds, &path)
}

/// Persist credentials for a repo URL (canonicalized first so refresh and
/// delete resolve the same hash). Called after a successful authenticated
/// clone; replaces any previously stored value for the same repo.
pub async fn store_credential_for(app: &tauri::AppHandle, url: &str, username: &str, password: &str) -> Result<()> {
    let c = canonicalize_url(url).await;
    store_credential(app, &repo_hash(&c), username, password)
}

#[cfg(test)]
mod url_tests {
    use super::*;

    #[test]
    fn ssh_wrapper_renders_filter_and_askpass() {
        // No password: filter wrapper only, no askpass, no SSH_ASKPASS export.
        let w = ensure_ssh_wrapper(None).unwrap().expect("wrapper on unix");
        let script = std::fs::read_to_string(&w.command).unwrap();
        assert!(script.contains("StrictHostKeyChecking=accept-new"));
        assert!(script.contains("Connection to .* succeeded"));
        // No password -> BatchMode: fail fast instead of prompting on /dev/tty
        // (the hang + console-prompt bug).
        assert!(script.contains("BatchMode=yes"));
        assert!(!script.contains("SSH_ASKPASS"));
        assert!(!w.dir.join("askpass.sh").exists());

        // With password: askpass installed (0600), password single-quote
        // escaped so `p@ss'w` round-trips; prompt guard keeps the password
        // away from non-password prompts.
        let w = ensure_ssh_wrapper(Some("p@ss'w")).unwrap().expect("wrapper on unix");
        let script = std::fs::read_to_string(&w.command).unwrap();
        assert!(script.contains("SSH_ASKPASS_REQUIRE=force"));
        assert!(script.contains("MCPHUB_SSH_DEBUG_LOG"));
        // Password provided -> askpass answers; BatchMode would disable askpass
        // queries, so it must NOT be set here.
        assert!(!script.contains("BatchMode=yes"));
        let ask = std::fs::read_to_string(w.dir.join("askpass.sh")).unwrap();
        // password single-quote escaped so it round-trips through sh
        assert!(ask.contains("p@ss'\''w"));
        // answers EVERY prompt (server kbdint prompts may be localized) and
        // logs the prompt text (never the answer) for diagnostics
        assert!(ask.contains("$MCPHUB_SSH_DEBUG_LOG"));
        assert!(!ask.contains("*assword*)"));
        std::fs::remove_dir_all(&w.dir).ok();
        std::fs::remove_dir_all(&dir_of(&w)).ok();
    }

    // helper for the test above: the bundle dir is command.parent()
    fn dir_of(w: &SshWrapper) -> std::path::PathBuf { w.dir.clone() }

    #[tokio::test]
    async fn canonicalize_upgrades_http_redirect() {
        // Real network: the user's server 308s http -> https. The canonical
        // form must come back with the https scheme; ssh/SCP URLs and
        // unreachable hosts must pass through unchanged.
        let direct = canonicalize_url("https://git.haidaifu.net/hdf-book/hdf-book.git").await;
        assert_eq!(direct, "https://git.haidaifu.net/hdf-book/hdf-book.git");
        let upgraded = canonicalize_url("http://git.haidaifu.net/hdf-book/hdf-book.git").await;
        assert_eq!(upgraded, "https://git.haidaifu.net/hdf-book/hdf-book.git");
        // ssh untouched
        let ssh = canonicalize_url("git@github.com:u/r.git").await;
        assert_eq!(ssh, "git@github.com:u/r.git");
    }

    #[test]
    fn validate_accepts_http_and_https() {
        assert!(validate_url("https://github.com/u/r.git").is_ok());
        assert!(validate_url("http://git.haidaifu.net/hdf-book/hdf-book.git").is_ok());
        assert!(validate_url("https://host").is_ok());
        assert!(validate_url("ftp://host/r.git").is_err());
        assert!(validate_url("").is_err());
    }

    #[test]
    fn validate_accepts_ssh_forms() {
        assert!(validate_url("ssh://git@github.com/u/r.git").is_ok());
        assert!(validate_url("ssh://host/r.git").is_ok());
        // ssh user@ is a login name, not a credential.
        assert!(validate_url("ssh://git@host:2222/u/r.git").is_ok());
        // SCP-style (explicit user@ required).
        assert!(validate_url("git@github.com:u/r.git").is_ok());
        assert!(is_scp_style("git@github.com:u/r.git"));
        // No user@ -> ambiguous with local/Windows paths -> reject.
        assert!(!is_scp_style("host:path/r.git"));
        assert!(validate_url("host:path/r.git").is_err());
        assert!(validate_url("C:\\Users\\x\\repo").is_err());
        assert!(validate_url("./local/dir").is_err());
    }

    #[test]
    fn validate_rejects_ssh_embedded_password() {
        assert!(validate_url("ssh://git:pass@host/r.git").is_err());
        assert!(validate_url("git:pass@host:u/r.git").is_err());
    }

    #[test]
    fn validate_rejects_embedded_credentials_both_schemes() {
        assert!(validate_url("https://user:pass@host/r.git").is_err());
        assert!(validate_url("https://user@host/r.git").is_err());
        assert!(validate_url("http://user:pass@host/r.git").is_err());
        assert!(validate_url("http://user@host/r.git").is_err());
        // path segments containing @ are fine (not authority).
        assert!(validate_url("https://host/u@r/r.git").is_ok());
    }

    #[test]
    fn is_auth_error_matches_gix_credential_flow() {
        // gix handshake: 401 -> credential helper roundtrip -> with no helper
        // configured it fails with EmptyCredentials (no "401" text survives).
        let e = anyhow::anyhow!("No credentials were returned at all as if the credential helper isn't functioning unknowingly");
        assert!(is_auth_error(&e));
        let e = anyhow::anyhow!("Credentials provided for \"http://host/r.git\" were not accepted by the remote");
        assert!(is_auth_error(&e));
        let e = anyhow::anyhow!("Failed to obtain credentials");
        assert!(is_auth_error(&e));
        // ssh failures (system ssh stderr): key rejected / host key.
        let e = anyhow::anyhow!("git@github.com: Permission denied (publickey).");
        assert!(is_auth_error(&e));
        let e = anyhow::anyhow!("Host key verification failed.");
        assert!(is_auth_error(&e));
        let e = anyhow::anyhow!("fatal: Could not read from remote repository.");
        assert!(is_auth_error(&e));
        // Repo names containing digits must NOT trip the phrase matcher.
        let e = anyhow::anyhow!("Received HTTP status 500 for repo issue-1401");
        assert!(!is_auth_error(&e));
    }

    #[test]
    fn build_clone_url_preserves_scheme() {        assert_eq!(
            build_clone_url("https://host/r.git", Some("u"), Some("p")),
            "https://u:p@host/r.git"
        );
        assert_eq!(
            build_clone_url("http://host/r.git", Some("u"), Some("p")),
            "http://u:p@host/r.git"
        );
        // anonymous: url unchanged
        assert_eq!(build_clone_url("http://host/r.git", None, None), "http://host/r.git");
        // username-only credential (no password)
        assert_eq!(
            build_clone_url("http://host/r.git", Some("u"), None),
            "http://u@host/r.git"
        );
        // password percent-encoding
        assert_eq!(
            build_clone_url("http://host/r.git", Some("u"), Some("p@ss:w")),
            "http://u:p%40ss%3Aw@host/r.git"
        );
        // SSH: form username is the ssh LOGIN NAME and overrides the URL's
        // conventional `git@` (password-auth servers use the user's account,
        // not `git`); the password never enters the URL (askpass bridge).
        assert_eq!(
            build_clone_url("git@github.com:u/r.git", Some("alice"), Some("p")),
            "alice@github.com:u/r.git"
        );
        assert_eq!(
            build_clone_url("ssh://git@host/r.git", Some("alice"), Some("p")),
            "ssh://alice@host/r.git"
        );
        // No form username -> URL untouched (key auth, `git@` convention).
        assert_eq!(
            build_clone_url("ssh://git@host/r.git", None, None),
            "ssh://git@host/r.git"
        );
        // ssh:// with port: userinfo swap preserves the port.
        assert_eq!(
            build_clone_url("ssh://git@host:2222/r.git", Some("alice"), Some("p")),
            "ssh://alice@host:2222/r.git"
        );
    }
}
