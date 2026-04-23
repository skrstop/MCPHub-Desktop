/// Bundled runtime environment management.
///
/// Resolves commands like `node`, `npx`, `uv`, `uvx`, `python` to the
/// bundled binaries shipped with the app, so users don't need Node.js or
/// Python installed on their system.
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

static RUNTIMES_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Active Node.js version: "system" or a version string like "22.14.0"
static ACTIVE_NODE: OnceLock<RwLock<String>> = OnceLock::new();
/// Active Python version: "system" or a version string like "3.12"
static ACTIVE_PYTHON: OnceLock<RwLock<String>> = OnceLock::new();

/// Initialize with the resolved runtimes directory.
/// Call once on app startup before spawning any MCP servers.
pub fn init(runtimes: PathBuf) {
    if !runtimes.exists() {
        log::warn!("[runtime_env] Runtimes directory not found: {:?}", runtimes);
        return;
    }

    // Ensure node and uv binaries are executable (permissions may be lost on some systems)
    #[cfg(unix)]
    ensure_executable(&runtimes);

    let _ = RUNTIMES_DIR.set(runtimes.clone());
    log::info!("[runtime_env] Initialized: {:?}", runtimes);
}

#[cfg(unix)]
fn ensure_executable(runtimes: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let candidates = [
        node_bin_path(runtimes),
        uv_bin_path(runtimes),
        runtimes.join("uv").join("uvx"),
    ];
    for path in &candidates {
        if path.exists() {
            if let Ok(meta) = std::fs::metadata(path) {
                let mut perms = meta.permissions();
                perms.set_mode(0o755);
                let _ = std::fs::set_permissions(path, perms);
            }
        }
    }
}

fn runtimes_dir() -> Option<&'static PathBuf> {
    RUNTIMES_DIR.get()
}

// ---------------------------------------------------------------------------
// Active version management
// ---------------------------------------------------------------------------

/// Set the active Node.js version ("system" or a version string like "22.14.0").
pub fn set_active_node(version: String) {
    let rw = ACTIVE_NODE.get_or_init(|| RwLock::new("system".to_string()));
    if let Ok(mut v) = rw.write() {
        *v = version;
    }
}

/// Get the currently active Node.js version ("system" by default).
pub fn get_active_node() -> String {
    ACTIVE_NODE
        .get()
        .and_then(|rw| rw.read().ok().map(|v| v.clone()))
        .unwrap_or_else(|| "system".to_string())
}

/// Set the active Python version ("system" or a version string like "3.12").
pub fn set_active_python(version: String) {
    let rw = ACTIVE_PYTHON.get_or_init(|| RwLock::new("system".to_string()));
    if let Ok(mut v) = rw.write() {
        *v = version;
    }
}

/// Get the currently active Python version ("system" by default).
pub fn get_active_python() -> String {
    ACTIVE_PYTHON
        .get()
        .and_then(|rw| rw.read().ok().map(|v| v.clone()))
        .unwrap_or_else(|| "system".to_string())
}

// ---------------------------------------------------------------------------
// Public path helpers used by runtime commands
// ---------------------------------------------------------------------------

/// Returns the base directory where user-managed Node.js versions are installed.
/// e.g. ~/Library/Application Support/mcphub-desktop/node-versions/ (macOS)
pub fn node_versions_base() -> Option<PathBuf> {
    app_data_dir("node-versions")
}

/// Returns the path to the bundled `uv` binary, if available.
pub fn get_uv_path() -> Option<PathBuf> {
    let rt = runtimes_dir()?;
    let p = uv_bin_path(rt);
    if p.exists() { Some(p) } else { None }
}

/// Returns the directory where uv installs Python versions (isolated from system).
pub fn uv_python_install_dir() -> Option<PathBuf> {
    let rt = runtimes_dir()?;
    let dir = uv_dir(rt).join("python");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

// ---------------------------------------------------------------------------
// Command resolution
// ---------------------------------------------------------------------------

/// Resolve a command + args, remapping known runtimes to bundled binaries.
///
/// Returns `(resolved_command, resolved_args)`.
pub fn resolve_command(command: &str, args: &[String]) -> (String, Vec<String>) {
    match command {
        "node" | "npx" | "npm" => {
            let active = get_active_node();
            // "system" → let the OS find node on PATH, no override
            if active == "system" {
                return (command.to_string(), args.to_vec());
            }
            // Specific version → look in user-managed node-versions directory
            if active != "system" {
                if let Some(base) = node_versions_base() {
                    let ver_dir = base.join(&active);
                    if ver_dir.exists() {
                        let resolved = resolve_node_command_in_dir(command, args, &ver_dir);
                        if resolved.is_some() {
                            return resolved.unwrap();
                        }
                    }
                }
            }
            // Fall through to bundled runtime if version dir not found
        }
        "python" | "python3" => {
            let active = get_active_python();
            if active == "system" {
                return (command.to_string(), args.to_vec());
            }
            // For a specific python version, fall through to uv-managed resolution below
        }
        _ => {}
    }

    let Some(rt) = runtimes_dir() else {
        return (command.to_string(), args.to_vec());
    };

    match command {
        "node" => {
            let bin = node_bin_path(rt);
            if bin.exists() {
                return (bin.to_string_lossy().into_owned(), args.to_vec());
            }
        }
        "npx" => {
            // Call `node <npx-cli.js> [args]` directly — avoids shell script issues
            // when binaries are extracted from a bundle.
            let node = node_bin_path(rt);
            let cli = npx_cli_path(rt);
            if node.exists() && cli.exists() {
                let mut new_args = vec![cli.to_string_lossy().into_owned()];
                new_args.extend_from_slice(args);
                return (node.to_string_lossy().into_owned(), new_args);
            }
        }
        "npm" => {
            let node = node_bin_path(rt);
            let cli = npm_cli_path(rt);
            if node.exists() && cli.exists() {
                let mut new_args = vec![cli.to_string_lossy().into_owned()];
                new_args.extend_from_slice(args);
                return (node.to_string_lossy().into_owned(), new_args);
            }
        }
        "uvx" => {
            // Try dedicated uvx binary first; fall back to `uv tool run`
            let uvx = uv_dir(rt).join(uvx_exe());
            if uvx.exists() {
                return (uvx.to_string_lossy().into_owned(), args.to_vec());
            }
            let uv = uv_bin_path(rt);
            if uv.exists() {
                let mut new_args = vec!["tool".to_string(), "run".to_string()];
                new_args.extend_from_slice(args);
                return (uv.to_string_lossy().into_owned(), new_args);
            }
        }
        "uv" => {
            let bin = uv_bin_path(rt);
            if bin.exists() {
                return (bin.to_string_lossy().into_owned(), args.to_vec());
            }
        }
        "python" | "python3" => {
            // Prefer a managed Python inside the bundled uv python dir
            if let Some(py) = find_bundled_python(rt) {
                return (py.to_string_lossy().into_owned(), args.to_vec());
            }
            // Fallback: use `uv run python`
            let uv = uv_bin_path(rt);
            if uv.exists() {
                let mut new_args = vec!["run".to_string(), "python".to_string()];
                new_args.extend_from_slice(args);
                return (uv.to_string_lossy().into_owned(), new_args);
            }
        }
        _ => {}
    }

    (command.to_string(), args.to_vec())
}

/// Returns extra environment variables to inject when spawning `command`.
///
/// Prepends bundled binary directories to `PATH` and sets runtime-specific
/// cache/install dirs so everything stays self-contained within the app's
/// data directory.
pub fn env_overrides(original_command: &str) -> Vec<(String, String)> {
    let Some(rt) = runtimes_dir() else {
        return vec![];
    };

    let sep = if cfg!(target_os = "windows") { ";" } else { ":" };
    let existing_path = std::env::var("PATH").unwrap_or_default();

    let mut prepend_dirs: Vec<String> = vec![];

    match original_command {
        "node" | "npx" | "npm" => {
            let active = get_active_node();
            if active == "system" {
                return vec![]; // use system node, no injection
            }
            // User-managed version in node-versions directory
            if let Some(base) = node_versions_base() {
                let ver_dir = base.join(&active);
                if ver_dir.exists() {
                    let bin_dir = node_bin_dir_in(&ver_dir);
                    prepend_dirs.push(bin_dir.to_string_lossy().into_owned());
                }
            }
            // Fallback to bundled
            if prepend_dirs.is_empty() {
                prepend_dirs.push(node_bin_dir(rt).to_string_lossy().into_owned());
            }
        }
        "uv" | "uvx" | "python" | "python3" => {
            prepend_dirs.push(uv_dir(rt).to_string_lossy().into_owned());
        }
        _ => return vec![],
    }

    let new_path = format!("{}{}{}", prepend_dirs.join(sep), sep, existing_path);
    let mut env: Vec<(String, String)> = vec![("PATH".to_string(), new_path)];

    // Point npm cache to app-local directory to avoid permission issues
    if matches!(original_command, "node" | "npx" | "npm") {
        if let Some(cache) = app_local_dir("npm-cache") {
            env.push(("npm_config_cache".to_string(), cache));
        }
    }

    // Point uv to our bundled Python and use app-local cache/tool dirs
    if matches!(original_command, "uv" | "uvx" | "python" | "python3") {
        let python_install_dir = uv_dir(rt).join("python");
        if python_install_dir.exists() {
            env.push((
                "UV_PYTHON_INSTALL_DIR".to_string(),
                python_install_dir.to_string_lossy().into_owned(),
            ));
        }
        if let Some(cache) = app_local_dir("uv-cache") {
            env.push(("UV_CACHE_DIR".to_string(), cache));
        }
        if let Some(tools) = app_local_dir("uv-tools") {
            env.push(("UV_TOOL_DIR".to_string(), tools));
        }
        // Pin to a specific Python version if user has selected one
        let active_py = get_active_python();
        if active_py != "system" && !active_py.is_empty() {
            env.push(("UV_PYTHON".to_string(), format!("cpython-{active_py}")));
        }
    }

    env
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Resolve a node/npx/npm command within a specific Node.js installation directory.
fn resolve_node_command_in_dir(command: &str, args: &[String], dir: &Path) -> Option<(String, Vec<String>)> {
    match command {
        "node" => {
            let bin = node_bin_in(dir);
            if bin.exists() {
                return Some((bin.to_string_lossy().into_owned(), args.to_vec()));
            }
        }
        "npx" => {
            let node = node_bin_in(dir);
            let cli = npx_cli_in(dir);
            if node.exists() && cli.exists() {
                let mut new_args = vec![cli.to_string_lossy().into_owned()];
                new_args.extend_from_slice(args);
                return Some((node.to_string_lossy().into_owned(), new_args));
            }
        }
        "npm" => {
            let node = node_bin_in(dir);
            let cli = npm_cli_in(dir);
            if node.exists() && cli.exists() {
                let mut new_args = vec![cli.to_string_lossy().into_owned()];
                new_args.extend_from_slice(args);
                return Some((node.to_string_lossy().into_owned(), new_args));
            }
        }
        _ => {}
    }
    None
}

fn node_bin_in(dir: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    return dir.join("node.exe");
    #[cfg(not(target_os = "windows"))]
    dir.join("bin").join("node")
}

fn npx_cli_in(dir: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    return dir.join("node_modules").join("npm").join("bin").join("npx-cli.js");
    #[cfg(not(target_os = "windows"))]
    dir.join("lib").join("node_modules").join("npm").join("bin").join("npx-cli.js")
}

fn npm_cli_in(dir: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    return dir.join("node_modules").join("npm").join("bin").join("npm-cli.js");
    #[cfg(not(target_os = "windows"))]
    dir.join("lib").join("node_modules").join("npm").join("bin").join("npm-cli.js")
}

fn node_bin_dir_in(dir: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    return dir.to_path_buf();
    #[cfg(not(target_os = "windows"))]
    dir.join("bin")
}

fn node_bin_path(rt: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    return rt.join("node").join("node.exe");
    #[cfg(not(target_os = "windows"))]
    rt.join("node").join("bin").join("node")
}

fn node_bin_dir(rt: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    return rt.join("node");
    #[cfg(not(target_os = "windows"))]
    rt.join("node").join("bin")
}

fn npx_cli_path(rt: &Path) -> PathBuf {
    // Windows: node_modules/npm/bin/npx-cli.js (no lib/ prefix)
    // macOS/Linux: lib/node_modules/npm/bin/npx-cli.js
    #[cfg(target_os = "windows")]
    return rt
        .join("node")
        .join("node_modules")
        .join("npm")
        .join("bin")
        .join("npx-cli.js");
    #[cfg(not(target_os = "windows"))]
    rt.join("node")
        .join("lib")
        .join("node_modules")
        .join("npm")
        .join("bin")
        .join("npx-cli.js")
}

fn npm_cli_path(rt: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    return rt
        .join("node")
        .join("node_modules")
        .join("npm")
        .join("bin")
        .join("npm-cli.js");
    #[cfg(not(target_os = "windows"))]
    rt.join("node")
        .join("lib")
        .join("node_modules")
        .join("npm")
        .join("bin")
        .join("npm-cli.js")
}

fn uv_dir(rt: &Path) -> PathBuf {
    rt.join("uv")
}

fn uv_bin_path(rt: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    return uv_dir(rt).join("uv.exe");
    #[cfg(not(target_os = "windows"))]
    uv_dir(rt).join("uv")
}

fn uvx_exe() -> &'static str {
    #[cfg(target_os = "windows")]
    return "uvx.exe";
    #[cfg(not(target_os = "windows"))]
    "uvx"
}

/// Find a Python binary inside the bundled uv Python install dir.
fn find_bundled_python(rt: &Path) -> Option<PathBuf> {
    let python_dir = uv_dir(rt).join("python");
    if !python_dir.exists() {
        return None;
    }
    // uv installs Python as: python/cpython-3.12.x-{platform}/bin/python3
    let bin_name = if cfg!(target_os = "windows") {
        "python.exe"
    } else {
        "python3"
    };
    let entries = std::fs::read_dir(&python_dir).ok()?;
    for entry in entries.flatten() {
        let candidate = if cfg!(target_os = "windows") {
            entry.path().join(bin_name)
        } else {
            entry.path().join("bin").join(bin_name)
        };
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Returns an app-local cache subdirectory path (creates it if needed).
fn app_local_dir(name: &str) -> Option<String> {
    // Use XDG_CACHE_HOME on Linux, ~/Library/Caches on macOS
    #[cfg(target_os = "macos")]
    let base = std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join("Library").join("Caches"));
    #[cfg(target_os = "linux")]
    let base = std::env::var("XDG_CACHE_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".cache"))
        });
    #[cfg(target_os = "windows")]
    let base = std::env::var("LOCALAPPDATA").ok().map(PathBuf::from);

    let dir = base?.join("mcphub-desktop").join(name);
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.to_string_lossy().into_owned())
}

/// Returns an app data subdirectory (persistent, not cache) for storing managed runtimes.
/// macOS: ~/Library/Application Support/mcphub-desktop/{name}
/// Linux: ~/.local/share/mcphub-desktop/{name}
/// Windows: %APPDATA%/mcphub-desktop/{name}
pub fn app_data_dir(name: &str) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let base = std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join("Library").join("Application Support"));
    #[cfg(target_os = "linux")]
    let base = std::env::var("XDG_DATA_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".local").join("share"))
        });
    #[cfg(target_os = "windows")]
    let base = std::env::var("APPDATA").ok().map(PathBuf::from);

    let dir = base?.join("mcphub-desktop").join(name);
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}
