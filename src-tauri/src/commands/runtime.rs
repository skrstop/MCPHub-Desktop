/// Runtime version management commands.
///
/// Allows the frontend to list, install, uninstall, and switch between
/// different Node.js and Python runtime versions, all isolated within
/// the app's data directory (no impact on the user's system).
use crate::services::{config_service, runtime_env};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeVersion {
    pub version: String,
    /// Whether this version is installed (always true for "system")
    pub installed: bool,
    /// Whether this version is currently selected
    pub active: bool,
}

/// Popular Node.js LTS versions offered for installation.
const NODE_VERSIONS: &[&str] = &["22.14.0", "20.18.3", "18.20.7"];

/// Popular Python versions offered for installation (managed via uv).
const PYTHON_VERSIONS: &[&str] = &["3.13", "3.12", "3.11", "3.10"];

// ────────────────────────────────────────────────────────────────────────────
// List commands
// ────────────────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_node_versions() -> Result<Vec<RuntimeVersion>, String> {
    let active = runtime_env::get_active_node();
    let mut result = vec![RuntimeVersion {
        version: "system".to_string(),
        installed: true,
        active: active == "system",
    }];

    for &ver in NODE_VERSIONS {
        let installed = node_version_installed(ver);
        result.push(RuntimeVersion {
            version: ver.to_string(),
            installed,
            active: active == ver,
        });
    }
    Ok(result)
}

#[tauri::command]
pub async fn list_python_versions() -> Result<Vec<RuntimeVersion>, String> {
    let active = runtime_env::get_active_python();
    let installed = get_installed_python_versions().await;

    let mut result = vec![RuntimeVersion {
        version: "system".to_string(),
        installed: true,
        active: active == "system",
    }];

    for &ver in PYTHON_VERSIONS {
        let is_installed = installed.iter().any(|v| v.starts_with(ver));
        result.push(RuntimeVersion {
            version: ver.to_string(),
            installed: is_installed,
            active: active == ver,
        });
    }
    Ok(result)
}

// ────────────────────────────────────────────────────────────────────────────
// Install commands
// ────────────────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn install_node_version(version: String) -> Result<(), String> {
    let base = runtime_env::node_versions_base()
        .ok_or_else(|| "Cannot determine app data directory".to_string())?;
    let dest = base.join(&version);

    if node_bin_in(&dest).exists() {
        return Ok(()); // already installed
    }

    let url = node_download_url(&version);
    log::info!("[runtime] Downloading Node.js {version} from {url}");

    let response = reqwest::get(&url)
        .await
        .map_err(|e| format!("Download failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("Download failed with HTTP {}", response.status()));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("Download read failed: {e}"))?;

    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;

    extract_node_archive(&bytes, &dest)?;

    // Ensure node binary is executable on Unix
    #[cfg(unix)]
    set_executable(&node_bin_in(&dest));

    log::info!("[runtime] Node.js {version} installed to {dest:?}");
    Ok(())
}

#[tauri::command]
pub async fn install_python_version(version: String) -> Result<(), String> {
    let uv = runtime_env::get_uv_path()
        .ok_or_else(|| "Bundled uv not found. Run the download-runtimes script first.".to_string())?;
    let python_dir = runtime_env::uv_python_install_dir()
        .ok_or_else(|| "Cannot determine Python install directory".to_string())?;

    log::info!("[runtime] Installing Python {version} via uv...");

    let output = tokio::process::Command::new(&uv)
        .args(["python", "install", &format!("cpython-{version}")])
        .env("UV_PYTHON_INSTALL_DIR", &python_dir)
        .output()
        .await
        .map_err(|e| format!("Failed to run uv: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("uv python install failed: {stderr}"));
    }

    log::info!("[runtime] Python {version} installed successfully");
    Ok(())
}

// ────────────────────────────────────────────────────────────────────────────
// Uninstall commands
// ────────────────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn uninstall_node_version(version: String) -> Result<(), String> {
    let base = runtime_env::node_versions_base()
        .ok_or_else(|| "Cannot determine app data directory".to_string())?;
    let dir = base.join(&version);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    // Reset to system if this was the active version
    if runtime_env::get_active_node() == version {
        runtime_env::set_active_node("system".to_string());
        let _ = save_version_to_db("nodeVersion", "system").await;
    }
    Ok(())
}

#[tauri::command]
pub async fn uninstall_python_version(version: String) -> Result<(), String> {
    let uv = runtime_env::get_uv_path()
        .ok_or_else(|| "Bundled uv not available".to_string())?;
    let python_dir = runtime_env::uv_python_install_dir()
        .ok_or_else(|| "Cannot determine Python install directory".to_string())?;

    let output = tokio::process::Command::new(&uv)
        .args(["python", "uninstall", &format!("cpython-{version}")])
        .env("UV_PYTHON_INSTALL_DIR", &python_dir)
        .output()
        .await
        .map_err(|e| format!("Failed to run uv: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("uv python uninstall failed: {stderr}"));
    }

    // Reset to system if this was the active version
    if runtime_env::get_active_python() == version {
        runtime_env::set_active_python("system".to_string());
        let _ = save_version_to_db("pythonVersion", "system").await;
    }
    Ok(())
}

// ────────────────────────────────────────────────────────────────────────────
// Active version get / set
// ────────────────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_active_node_version() -> Result<String, String> {
    Ok(runtime_env::get_active_node())
}

#[tauri::command]
pub async fn get_active_python_version() -> Result<String, String> {
    Ok(runtime_env::get_active_python())
}

#[tauri::command]
pub async fn set_active_node_version(version: String) -> Result<(), String> {
    runtime_env::set_active_node(version.clone());
    save_version_to_db("nodeVersion", &version).await
}

#[tauri::command]
pub async fn set_active_python_version(version: String) -> Result<(), String> {
    runtime_env::set_active_python(version.clone());
    save_version_to_db("pythonVersion", &version).await
}

// ────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ────────────────────────────────────────────────────────────────────────────

fn node_version_installed(version: &str) -> bool {
    runtime_env::node_versions_base()
        .map(|b| node_bin_in(&b.join(version)).exists())
        .unwrap_or(false)
}

fn node_download_url(version: &str) -> String {
    let platform = if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "win"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" };

    if cfg!(target_os = "windows") {
        format!("https://nodejs.org/dist/v{version}/node-v{version}-{platform}-{arch}.zip")
    } else {
        format!("https://nodejs.org/dist/v{version}/node-v{version}-{platform}-{arch}.tar.gz")
    }
}

fn node_bin_in(dir: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    return dir.join("node.exe");
    #[cfg(not(target_os = "windows"))]
    dir.join("bin").join("node")
}

#[cfg(not(target_os = "windows"))]
fn extract_node_archive(bytes: &[u8], dest: &Path) -> Result<(), String> {
    use flate2::read::GzDecoder;
    use std::io::Cursor;
    use tar::Archive;

    let cursor = Cursor::new(bytes);
    let gz = GzDecoder::new(cursor);
    let mut archive = Archive::new(gz);

    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?;
        // Strip the top-level directory (e.g. node-v22.14.0-darwin-arm64/)
        let stripped: PathBuf = path.components().skip(1).collect();
        if stripped.as_os_str().is_empty() {
            continue;
        }
        let out = dest.join(&stripped);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        entry.unpack(&out).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn extract_node_archive(bytes: &[u8], dest: &Path) -> Result<(), String> {
    use std::io::Cursor;
    use zip::read::ZipArchive;

    let cursor = Cursor::new(bytes);
    let mut archive = ZipArchive::new(cursor).map_err(|e| e.to_string())?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
        let raw_path = file.mangled_name();
        let stripped: PathBuf = raw_path.components().skip(1).collect();
        if stripped.as_os_str().is_empty() {
            continue;
        }
        let out = dest.join(&stripped);
        if file.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut outfile = std::fs::File::create(&out).map_err(|e| e.to_string())?;
            std::io::copy(&mut file, &mut outfile).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o755);
        let _ = std::fs::set_permissions(path, perms);
    }
}

async fn get_installed_python_versions() -> Vec<String> {
    let Some(uv) = runtime_env::get_uv_path() else {
        return vec![];
    };
    let Some(python_dir) = runtime_env::uv_python_install_dir() else {
        return vec![];
    };

    let Ok(output) = tokio::process::Command::new(&uv)
        .args(["python", "list", "--only-installed"])
        .env("UV_PYTHON_INSTALL_DIR", &python_dir)
        .output()
        .await
    else {
        return vec![];
    };

    if !output.status.success() {
        return vec![];
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            // Lines look like: "cpython-3.12.8-macos-aarch64  /path/to/python"
            // Extract the version part
            let ver_part = trimmed.split_whitespace().next()?;
            // Strip the "cpython-" prefix
            let ver = ver_part.strip_prefix("cpython-").unwrap_or(ver_part);
            // Return just major.minor (e.g. "3.12" from "3.12.8-macos-aarch64")
            let major_minor = ver.splitn(3, '.').take(2).collect::<Vec<_>>().join(".");
            if major_minor.is_empty() { None } else { Some(major_minor) }
        })
        .collect()
}

async fn save_version_to_db(key: &str, value: &str) -> Result<(), String> {
    let patch = serde_json::json!({ "install": { key: value } });
    config_service::update(&patch)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}
