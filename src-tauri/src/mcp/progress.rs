//! Server install/update progress events.
//!
//! Mirrors the `runtime://progress` pattern used by the runtime version
//! manager, but for stdio MCP server package downloads (npx/uvx) and
//! package-update checks.
//!
//! The `AppHandle` is stored in a process-global `OnceLock` so that
//! `connect_server` and the transports can emit events without threading the
//! handle through every call site.
use serde::Serialize;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Emitter};

use crate::services::{app_logger, config_service};

/// Download/install progress for a single server.
///
/// `phase`:
/// - `downloading` - npx/uvx is fetching packages (progress may be `None`
///   for an indeterminate bar).
/// - `done` - connected successfully.
/// - `error` - connect failed / timed out.
#[derive(Debug, Clone, Serialize)]
pub struct ServerInstallProgress {
    pub server: String,
    pub phase: String,
    /// 0..100, or `None` for an indeterminate progress bar.
    pub progress: Option<u8>,
    pub message: Option<String>,
}

/// Result of a best-effort "update available" check run after a stdio server
/// connects. Emitted on every check (for npx/uvx servers) so the frontend can
/// both show and clear the update badge.
///
/// `current` is the last recorded *package* version (from the registry, not
/// the server's self-reported `serverInfo.version`, which often uses a
/// different scheme and is not comparable).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerUpdateInfo {
    pub server: String,
    /// Whether a newer package version is available.
    pub has_update: bool,
    /// Last recorded installed package version.
    pub current: Option<String>,
    /// Latest version published on the registry.
    pub latest: Option<String>,
}

const INSTALL_PROGRESS_EVENT: &str = "server://install-progress";
const UPDATE_AVAILABLE_EVENT: &str = "server://update-available";

static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

/// Stash the app handle once at startup so transports/pool can emit events.
pub fn set_app_handle(app: AppHandle) {
    let _ = APP_HANDLE.set(app);
}

fn app_handle() -> Option<&'static AppHandle> {
    APP_HANDLE.get()
}

pub fn emit_install_progress(payload: &ServerInstallProgress) {
    if let Some(app) = app_handle() {
        if let Err(e) = app.emit(INSTALL_PROGRESS_EVENT, payload) {
            log::warn!("[progress] emit install-progress failed: {e}");
        }
    }
}

pub fn emit_update_available(payload: &ServerUpdateInfo) {
    if let Some(app) = app_handle() {
        if let Err(e) = app.emit(UPDATE_AVAILABLE_EVENT, payload) {
            log::warn!("[progress] emit update-available failed: {e}");
        }
    }
}

// ── Persisted "installed package version" tracking ─────────────────────────
//
// The update check compares the registry's latest version against the version
// we last recorded as installed for this server - NOT against the server's
// self-reported `serverInfo.version`, which frequently uses a different
// versioning scheme (e.g. an internal/API version) and is not comparable to
// the package version. The recorded version is stored in the system config
// under `packageVersions` and advances when the user reinstalls/updates.

async fn get_recorded_version(server: &str) -> Option<String> {
    let cfg = config_service::get().await.ok()?;
    cfg.get("packageVersions")
        .and_then(|v| v.get(server))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Record `version` as the currently-installed package version for `server`.
/// Deep-merged into config so other servers' records are preserved.
async fn set_recorded_version(server: &str, version: &str) {
    let patch = serde_json::json!({
        "packageVersions": { server: version }
    });
    if let Err(e) = config_service::update(&patch).await {
        log::warn!("[progress] failed to persist package version for {}: {}", server, e);
    }
}

// ── "Just reinstalled" flag ────────────────────────────────────────────────
//
// When the user clicks update, `reinstall_server` marks the server here. The
// next update check then records the freshly-downloaded latest version as
// installed (instead of notifying), so the badge clears and won't reappear
// for that version - even after an app restart.

static RECENTLY_REINSTALLED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn recently_reinstalled() -> &'static Mutex<HashSet<String>> {
    RECENTLY_REINSTALLED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Mark that `server` was just reinstalled/updated by the user.
pub fn mark_reinstalled(server: &str) {
    if let Ok(mut set) = recently_reinstalled().lock() {
        set.insert(server.to_string());
    }
}

/// Consume the "just reinstalled" flag for `server` (returns true once).
fn take_reinstalled(server: &str) -> bool {
    if let Ok(mut set) = recently_reinstalled().lock() {
        return set.remove(server);
    }
    false
}

/// Whether a server command triggers a package download on spawn (npx/uvx).
pub fn is_package_manager(command: &Option<String>) -> bool {
    matches!(command.as_deref(), Some("npx") | Some("uvx"))
}

/// Extract the npm/PyPI package name from an npx/uvx argv.
///
/// npx: first non-flag arg (skipping `-y`/`--yes`/`--`), stripping a trailing
/// `@version` (keeps scoped `@scope/pkg`).
/// uvx: the `--from` value if present, else the first positional; strips
/// `[extras]`, `==`/`>=` version specifiers and `@version`.
pub fn extract_package_name(command: &str, args: &[String]) -> Option<String> {
    let pkg = match command {
        "npx" => {
            let mut iter = args.iter();
            let mut pkg: Option<&str> = None;
            while let Some(a) = iter.next() {
                if a == "--" {
                    // package is the next arg after the separator
                    pkg = iter.next().map(|s| s.as_str());
                    break;
                }
                if a.starts_with('-') {
                    // skip flags (npx short flags don't take values we care about)
                    continue;
                }
                pkg = Some(a.as_str());
                break;
            }
            pkg.map(|s| s.to_string())?
        }
        "uvx" => {
            // --from <pkg> takes precedence
            let mut iter = args.iter().peekable();
            let mut from_pkg: Option<String> = None;
            let mut positional: Option<String> = None;
            while let Some(a) = iter.next() {
                if a == "--from" {
                    if let Some(v) = iter.next() {
                        from_pkg = Some(v.clone());
                    }
                } else if a.starts_with("--from=") {
                    from_pkg = Some(a["--from=".len()..].to_string());
                } else if a.starts_with('-') {
                    // skip other flags and their values heuristically only for
                    // known value-taking flags; unknown flags may consume the
                    // package, so we only treat the first bare positional as pkg
                    continue;
                } else if positional.is_none() {
                    positional = Some(a.clone());
                }
            }
            from_pkg.or(positional)?
        }
        _ => return None,
    };

    Some(normalize_package_name(&pkg))
}

/// Strip `@version`, `[extras]` and version specifiers from a raw package arg.
fn normalize_package_name(raw: &str) -> String {
    // Strip [extras]
    let raw = raw.split('[').next().unwrap_or(raw);
    // Strip @version: keep a leading @ (scoped), remove a trailing @ver
    let raw = if raw.starts_with('@') {
        // @scope/pkg@1.2.3 -> @scope/pkg ; @scope/pkg -> @scope/pkg
        if let Some(idx) = raw[1..].find('@') {
            &raw[..1 + idx]
        } else {
            raw
        }
    } else if let Some(idx) = raw.find('@') {
        &raw[..idx]
    } else {
        raw
    };
    // Strip == / >= / <= / ~= / > / < / ; specifiers
    let raw = raw.split(['=', '>', '<', '~', ';']).next().unwrap_or(raw);
    raw.trim().to_string()
}

/// Fetch the latest published version of a package from npm (npx) or PyPI (uvx).
/// Returns `None` on any error / timeout so callers can simply skip the check.
pub async fn fetch_latest_version(command: &str, package: &str) -> Option<String> {
    if package.is_empty() {
        return None;
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .ok()?;
    let result = match command {
        "npx" => {
            // npm registry: encode '/' for scoped packages
            let encoded = package.replace('/', "%2F");
            let url = format!("https://registry.npmjs.org/{}/latest", encoded);
            let resp = client.get(&url).send().await.ok()?;
            if !resp.status().is_success() {
                return None;
            }
            let v: serde_json::Value = resp.json().await.ok()?;
            v.get("version").and_then(|v| v.as_str()).map(|s| s.to_string())
        }
        "uvx" => {
            let url = format!("https://pypi.org/pypi/{}/json", package);
            let resp = client.get(&url).send().await.ok()?;
            if !resp.status().is_success() {
                return None;
            }
            let v: serde_json::Value = resp.json().await.ok()?;
            v.get("info")
                .and_then(|i| i.get("version"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        }
        _ => None,
    };
    result
}

/// Best-effort semver "is latest newer than current" comparison.
/// Parses the first `major.minor.patch` numeric triple out of each string
/// (ignoring `v` prefix and pre-release suffixes). Returns `false` if either
/// version cannot be parsed, so we never false-report an update.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let (l_major, l_minor, l_patch) = match parse_semver_triple(latest) {
        Some(t) => t,
        None => return false,
    };
    let (c_major, c_minor, c_patch) = match parse_semver_triple(current) {
        Some(t) => t,
        None => return false,
    };
    (l_major, l_minor, l_patch) > (c_major, c_minor, c_patch)
}

/// Pull the first `N.N.N` triple out of a version-ish string.
fn parse_semver_triple(s: &str) -> Option<(u64, u64, u64)> {
    // Find the first run of digits, then expect .N.N after it.
    let bytes = s.as_bytes();
    let mut i = 0;
    // skip non-digits
    while i < bytes.len() && !bytes[i].is_ascii_digit() {
        i += 1;
    }
    let major = read_num(bytes, &mut i)?;
    if i >= bytes.len() || bytes[i] != b'.' {
        return Some((major, 0, 0));
    }
    i += 1;
    let minor = read_num(bytes, &mut i).unwrap_or(0);
    if i >= bytes.len() || bytes[i] != b'.' {
        return Some((major, minor, 0));
    }
    i += 1;
    let patch = read_num(bytes, &mut i).unwrap_or(0);
    Some((major, minor, patch))
}

fn read_num(bytes: &[u8], i: &mut usize) -> Option<u64> {
    let start = *i;
    while *i < bytes.len() && bytes[*i].is_ascii_digit() {
        *i += 1;
    }
    if *i == start {
        return None;
    }
    std::str::from_utf8(&bytes[start..*i]).ok()?.parse::<u64>().ok()
}

/// Spawn a non-blocking "update available" check for a stdio server.
///
/// Run after a successful connect: extract the package name, fetch the latest
/// published version, and compare it against the **last recorded installed
/// package version** (persisted in config) - NOT against the server's
/// self-reported `serverInfo.version`, which often uses a different versioning
/// scheme and is not comparable to the package version.
///
/// Notification rules:
/// - First time seeing this server (no recorded version): record `latest` as
///   installed, no notification.
/// - `latest` newer than recorded: emit `has_update = true`.
/// - Otherwise: emit `has_update = false` (clears any stale badge).
/// - If the user just clicked update (`mark_reinstalled`), record `latest` as
///   installed and emit `has_update = false` (the badge clears and won't
///   reappear for this version, even after restart).
pub fn spawn_update_check(server_name: String, command: String, args: Vec<String>, running_version: Option<String>) {
    let pkg = match extract_package_name(&command, &args) {
        Some(p) => p,
        None => {
            log::debug!("[{}] skip update check: could not extract package name from args {:?}", server_name, args);
            return;
        }
    };
    let self_reported = running_version.filter(|v| !v.is_empty());
    let just_reinstalled = take_reinstalled(&server_name);
    let start_msg = format!(
        "[{}] 开始检查包更新（{} {}{}）...",
        server_name,
        command,
        pkg,
        self_reported.as_ref().map(|v| format!("，服务自报版本 {}", v)).unwrap_or_default()
    );
    log::info!("{}", start_msg);
    app_logger::log_to_db("info", &start_msg);

    tauri::async_runtime::spawn(async move {
        let latest = match fetch_latest_version(&command, &pkg).await {
            Some(v) => v,
            None => {
                let msg = format!(
                    "[{}] 更新检查失败：无法获取最新版本（{} {}）",
                    server_name, command, pkg
                );
                log::warn!("{}", msg);
                app_logger::log_to_db("warn", &msg);
                return;
            }
        };

        let recorded = get_recorded_version(&server_name).await;

        if just_reinstalled {
            // User just clicked update: the package was re-downloaded, so the
            // running version is now `latest`. Record it and clear the badge.
            set_recorded_version(&server_name, &latest).await;
            let msg = format!(
                "[{}] 更新完成，已记录已安装版本 {}（{}）",
                server_name, latest, pkg
            );
            log::info!("{}", msg);
            app_logger::log_to_db("info", &msg);
            emit_update_available(&ServerUpdateInfo {
                server: server_name.clone(),
                has_update: false,
                current: Some(latest.clone()),
                latest: Some(latest),
            });
            return;
        }

        match &recorded {
            None => {
                // First check: record current latest as installed, no notify.
                set_recorded_version(&server_name, &latest).await;
                let msg = format!(
                    "[{}] 首次记录包版本：{}（{}）",
                    server_name, latest, pkg
                );
                log::info!("{}", msg);
                app_logger::log_to_db("info", &msg);
                emit_update_available(&ServerUpdateInfo {
                    server: server_name.clone(),
                    has_update: false,
                    current: Some(latest.clone()),
                    latest: Some(latest),
                });
            }
            Some(prev) if is_newer(&latest, prev) => {
                let msg = format!(
                    "[{}] 检测到新版本：已安装 {}，最新 {}（{}）",
                    server_name, prev, latest, pkg
                );
                log::info!("{}", msg);
                app_logger::log_to_db("info", &msg);
                emit_update_available(&ServerUpdateInfo {
                    server: server_name.clone(),
                    has_update: true,
                    current: Some(prev.clone()),
                    latest: Some(latest),
                });
            }
            Some(prev) => {
                let msg = format!(
                    "[{}] 已是最新版本：{}（{}）",
                    server_name, prev, pkg
                );
                log::info!("{}", msg);
                app_logger::log_to_db("info", &msg);
                emit_update_available(&ServerUpdateInfo {
                    server: server_name.clone(),
                    has_update: false,
                    current: Some(prev.clone()),
                    latest: Some(latest),
                });
            }
        }
    });
}
