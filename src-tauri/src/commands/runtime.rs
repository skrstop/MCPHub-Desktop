/// Runtime version management commands.
///
/// Allows the frontend to list, install, uninstall, and switch between
/// different Node.js and Python runtime versions, all isolated within
/// the app's data directory (no impact on the user's system).
use crate::services::{config_service, runtime_env};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex as AsyncMutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeVersion {
    pub version: String,
    /// Whether this version is installed (always true for "system")
    pub installed: bool,
    /// Whether this version is currently selected
    pub active: bool,
    /// 已安装版本的健康状态。未安装或 system 默认为 true（无意义）。
    /// 安装目录残缺、可执行文件无法运行、版本号不匹配等情况会标记为 false。
    #[serde(default = "default_true")]
    pub healthy: bool,
    /// When `version == "system"`, holds the detected system version (e.g. "22.14.0").
    /// Null if system runtime is not found or version is not "system".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_version: Option<String>,
    /// Whether this version is bundled with the app (read-only, shipped in resources).
    /// Bundled versions cannot be reinstalled or uninstalled.
    #[serde(default)]
    pub bundled: bool,
}

fn default_true() -> bool {
    true
}

/// 安装/重装过程中向前端推送的进度事件
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeProgress {
    /// "node" | "python"
    pub runtime: String,
    pub version: String,
    /// "started" | "downloading" | "extracting" | "verifying" | "running" | "done" | "error"
    pub phase: String,
    /// 0..100；不可知时为 None（前端走 indeterminate 进度条）
    pub progress: Option<u8>,
    /// 一行人类可读消息（日志/错误等）
    pub message: Option<String>,
}

/// 事件名常量；前端 listen 同名
const RUNTIME_PROGRESS_EVENT: &str = "runtime://progress";

fn emit_progress(app: &AppHandle, payload: RuntimeProgress) {
    if let Err(e) = app.emit(RUNTIME_PROGRESS_EVENT, &payload) {
        log::warn!("[runtime] 推送进度事件失败: {e}");
    }
}

// 进程内缓存：避免每次刷新列表都发起远程请求
static NODE_VERSIONS_CACHE: OnceLock<AsyncMutex<Option<Vec<String>>>> = OnceLock::new();
static PYTHON_VERSIONS_CACHE: OnceLock<AsyncMutex<Option<Vec<String>>>> = OnceLock::new();

/// 获取 Node.js LTS 版本列表（带缓存）
async fn get_node_versions() -> Vec<String> {
    let cache = NODE_VERSIONS_CACHE.get_or_init(|| AsyncMutex::new(None));
    let mut guard = cache.lock().await;
    if let Some(v) = guard.as_ref() {
        return v.clone();
    }
    let fetched = fetch_node_lts_versions().await;
    *guard = Some(fetched.clone());
    fetched
}

/// 获取 Python 版本列表（带缓存）
async fn get_python_versions() -> Vec<String> {
    let cache = PYTHON_VERSIONS_CACHE.get_or_init(|| AsyncMutex::new(None));
    let mut guard = cache.lock().await;
    if let Some(v) = guard.as_ref() {
        return v.clone();
    }
    let fetched = fetch_python_versions().await;
    *guard = Some(fetched.clone());
    fetched
}

/// 从 nodejs.org 拉取所有发布信息，按主版本聚合，取最近 10 个 LTS 主版本的最新 patch。
async fn fetch_node_lts_versions() -> Vec<String> {
    #[derive(Deserialize)]
    struct NodeRelease {
        version: String, // 形如 "v22.14.0"
        #[serde(default)]
        lts: serde_json::Value, // false 或 "Jod" 等字符串
    }

    let url = "https://nodejs.org/dist/index.json";
    let releases: Vec<NodeRelease> = match reqwest::get(url).await {
        Ok(resp) => match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                log::warn!("[runtime] 解析 Node.js 版本列表失败，回退到内置列表: {e}");
                return fallback_node_versions();
            }
        },
        Err(e) => {
            log::warn!("[runtime] 拉取 Node.js 版本列表失败，回退到内置列表: {e}");
            return fallback_node_versions();
        }
    };

    use std::collections::BTreeMap;
    // major -> (min, patch, version_string) ，仅保留每个 major 的最新 patch
    let mut latest_per_major: BTreeMap<u32, (u32, u32, String)> = BTreeMap::new();
    for r in releases {
        // 仅保留 LTS（lts 字段为字符串）
        if !r.lts.is_string() {
            continue;
        }
        let v = r.version.trim_start_matches('v');
        let parts: Vec<&str> = v.split('.').collect();
        if parts.len() != 3 {
            continue;
        }
        let (Ok(maj), Ok(min), Ok(pat)) = (
            parts[0].parse::<u32>(),
            parts[1].parse::<u32>(),
            parts[2].parse::<u32>(),
        ) else {
            continue;
        };
        match latest_per_major.get(&maj) {
            Some(&(emin, epat, _)) if (emin, epat) >= (min, pat) => {}
            _ => {
                latest_per_major.insert(maj, (min, pat, v.to_string()));
            }
        }
    }

    // 按主版本号倒序，取最近 10 个
    let mut entries: Vec<(u32, String)> = latest_per_major
        .into_iter()
        .map(|(maj, (_, _, ver))| (maj, ver))
        .collect();
    entries.sort_by(|a, b| b.0.cmp(&a.0));
    let result: Vec<String> = entries.into_iter().take(10).map(|(_, v)| v).collect();
    if result.is_empty() {
        return fallback_node_versions();
    }
    result
}

/// 从 endoflife.date 拉取 Python 各 minor 版本的最新 patch，3.x 与 2.x 各取最多 10 个。
async fn fetch_python_versions() -> Vec<String> {
    #[derive(Deserialize)]
    struct PyCycle {
        cycle: String, // 形如 "3.13" / "2.7"
        #[serde(default)]
        latest: Option<String>, // 形如 "3.13.1"
    }

    let url = "https://endoflife.date/api/python.json";
    let client = match reqwest::Client::builder()
        .user_agent("mcphub-desktop")
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            log::warn!("[runtime] 创建 HTTP client 失败: {e}");
            return fallback_python_versions();
        }
    };

    let cycles: Vec<PyCycle> = match client.get(url).send().await {
        Ok(resp) => match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                log::warn!("[runtime] 解析 Python 版本列表失败，回退到内置列表: {e}");
                return fallback_python_versions();
            }
        },
        Err(e) => {
            log::warn!("[runtime] 拉取 Python 版本列表失败，回退到内置列表: {e}");
            return fallback_python_versions();
        }
    };

    let mut v3: Vec<(u32, String)> = vec![];
    let mut v2: Vec<(u32, String)> = vec![];
    for c in cycles {
        let Some(latest) = c.latest else {
            continue;
        };
        let parts: Vec<&str> = c.cycle.split('.').collect();
        if parts.len() < 2 {
            continue;
        }
        let (Ok(maj), Ok(min)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) else {
            continue;
        };
        match maj {
            3 => v3.push((min, latest)),
            2 => v2.push((min, latest)),
            _ => {}
        }
    }
    v3.sort_by(|a, b| b.0.cmp(&a.0));
    v2.sort_by(|a, b| b.0.cmp(&a.0));

    let mut result: Vec<String> = v3.into_iter().take(10).map(|(_, v)| v).collect();
    result.extend(v2.into_iter().take(10).map(|(_, v)| v));
    if result.is_empty() {
        return fallback_python_versions();
    }
    result
}

/// 网络异常时使用的内置 Node.js LTS 版本列表（保底）
fn fallback_node_versions() -> Vec<String> {
    vec![
        "24.17.0".to_string(),
        "22.14.0".to_string(),
        "20.18.3".to_string(),
        "18.20.7".to_string(),
        "16.20.2".to_string(),
        "14.21.3".to_string(),
        "12.22.12".to_string(),
        "10.24.1".to_string(),
    ]
}

/// 网络异常时使用的内置 Python 版本列表（保底）
fn fallback_python_versions() -> Vec<String> {
    vec![
        "3.14.0".to_string(),
        "3.13.1".to_string(),
        "3.12.8".to_string(),
        "3.11.11".to_string(),
        "3.10.16".to_string(),
        "3.9.21".to_string(),
        "3.8.20".to_string(),
    ]
}

// ────────────────────────────────────────────────────────────────────────────
// List commands
// ────────────────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_node_versions() -> Result<Vec<RuntimeVersion>, String> {
    let active = runtime_env::get_active_node();
    let system_ver = detect_system_node_version();
    let bundled_ver = detect_bundled_node_version();
    let mut result: Vec<RuntimeVersion> = Vec::new();

    // Only include "system" if the system actually has Node.js installed
    if let Some(ref ver) = system_ver {
        result.push(RuntimeVersion {
            version: "system".to_string(),
            installed: true,
            active: active == "system",
            healthy: true,
            system_version: Some(ver.clone()),
            bundled: false,
        });
    }

    let mut versions = get_node_versions().await;

    // Ensure bundled version is in the list (insert at correct sorted position)
    if let Some(ref bv) = bundled_ver {
        if !versions.iter().any(|v| v == bv) {
            insert_version_sorted(&mut versions, bv);
        }
    }

    for ver in versions {
        let installed = node_version_installed(&ver);
        let healthy = if installed { verify_node_version(&ver).await } else { true };
        let is_bundled = bundled_ver.as_deref() == Some(&ver);
        // If system has no runtime and user has "system" selected, activate the first installed (bundled) version
        let is_active = if active == "system" && system_ver.is_none() {
            installed && !result.iter().any(|r| r.active)
        } else {
            active == ver
        };
        result.push(RuntimeVersion {
            active: is_active,
            installed,
            healthy,
            version: ver,
            system_version: None,
            bundled: is_bundled,
        });
    }
    Ok(result)
}

#[tauri::command]
pub async fn list_python_versions() -> Result<Vec<RuntimeVersion>, String> {
    let active = runtime_env::get_active_python();
    let installed_entries = get_installed_python_versions().await;
    let system_ver = detect_system_python_version();

    let mut result: Vec<RuntimeVersion> = Vec::new();

    // Only include "system" if the system actually has Python installed
    if let Some(ref ver) = system_ver {
        result.push(RuntimeVersion {
            version: "system".to_string(),
            installed: true,
            active: active == "system",
            healthy: true,
            system_version: Some(ver.clone()),
            bundled: false,
        });
    }

    let mut versions = get_python_versions().await;

    // Ensure bundled Python versions (from installed_entries) are in the list
    for (mm, _, _) in &installed_entries {
        if !versions.iter().any(|v| {
            v.splitn(3, '.').take(2).collect::<Vec<_>>().join(".") == *mm
        }) {
            // Construct a representative version string, e.g. "3.12" → "3.12.0"
            insert_version_sorted(&mut versions, &format!("{mm}.0"));
        }
    }

    for ver in versions {
        let major_minor = ver
            .splitn(3, '.')
            .take(2)
            .collect::<Vec<_>>()
            .join(".");
        let installed_entry = installed_entries
            .iter()
            .find(|(mm, _, _)| mm == &major_minor)
            .cloned();
        let is_installed = installed_entry.is_some();
        let is_bundled = installed_entry.as_ref().map(|(_, _, b)| *b).unwrap_or(false);
        let healthy = match installed_entry {
            Some((_, exec, _)) => verify_python_executable(&exec).await,
            None => true,
        };
        // If system has no runtime and user has "system" selected, activate the first installed version
        let is_active = if active == "system" && system_ver.is_none() {
            is_installed && !result.iter().any(|r| r.active)
        } else {
            active == ver
        };
        result.push(RuntimeVersion {
            active: is_active,
            installed: is_installed,
            healthy,
            version: ver,
            system_version: None,
            bundled: is_bundled,
        });
    }
    Ok(result)
}

// ────────────────────────────────────────────────────────────────────────────
// Install commands
// ────────────────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn install_node_version(
    app: AppHandle,
    version: String,
    force: Option<bool>,
) -> Result<(), String> {
    let force = force.unwrap_or(false);
    let base = runtime_env::node_versions_base()
        .ok_or_else(|| "Cannot determine app data directory".to_string())?;
    let dest = base.join(&version);

    emit_progress(
        &app,
        RuntimeProgress {
            runtime: "node".into(),
            version: version.clone(),
            phase: "started".into(),
            progress: Some(0),
            message: Some(format!("准备安装 Node.js v{version}")),
        },
    );

    // 强制重装：先把整个版本目录清掉，确保走完整下载/解压流程
    if force && dest.exists() {
        log::warn!("[runtime] 强制重装 Node.js {version}，清理目录: {dest:?}");
        let _ = std::fs::remove_dir_all(&dest);
    }

    // 兜底：若已存在残留目录但缺少可执行文件，先清理；正常已安装的直接复用
    if dest.exists() && !node_bin_in(&dest).exists() {
        log::warn!("[runtime] 清理 Node.js {version} 残留目录: {dest:?}");
        let _ = std::fs::remove_dir_all(&dest);
    }
    if !force && node_bin_in(&dest).exists() {
        emit_progress(
            &app,
            RuntimeProgress {
                runtime: "node".into(),
                version: version.clone(),
                phase: "done".into(),
                progress: Some(100),
                message: Some(format!("Node.js v{version} 已安装，跳过下载")),
            },
        );
        return Ok(()); // already installed
    }

    let url = node_download_url(&version);
    log::info!("[runtime] Downloading Node.js {version} from {url}");
    emit_progress(
        &app,
        RuntimeProgress {
            runtime: "node".into(),
            version: version.clone(),
            phase: "downloading".into(),
            progress: Some(0),
            message: Some(format!("开始下载 {url}")),
        },
    );

    let response = reqwest::get(&url)
        .await
        .map_err(|e| {
            let msg = format!("下载失败: {e}");
            emit_error(&app, "node", &version, &msg);
            msg
        })?;
    if !response.status().is_success() {
        let msg = format!("下载失败 HTTP {}", response.status());
        emit_error(&app, "node", &version, &msg);
        return Err(msg);
    }

    let total = response.content_length();
    let mut buf: Vec<u8> = Vec::with_capacity(total.unwrap_or(0) as usize);
    let mut downloaded: u64 = 0;
    let mut last_pct: i32 = -1;
    // 即便没有 Content-Length，也每累计这么多字节推送一次心跳，避免前端假死
    const HEARTBEAT_BYTES: u64 = 512 * 1024;
    let mut last_heartbeat: u64 = 0;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            let msg = format!("下载读取失败: {e}");
            emit_error(&app, "node", &version, &msg);
            msg
        })?;
        downloaded += chunk.len() as u64;
        buf.extend_from_slice(&chunk);

        match total {
            Some(t) => {
                let pct = ((downloaded.saturating_mul(100)) / t.max(1)) as i32;
                // 节流：百分比变化才推送
                if pct != last_pct {
                    last_pct = pct;
                    emit_progress(
                        &app,
                        RuntimeProgress {
                            runtime: "node".into(),
                            version: version.clone(),
                            phase: "downloading".into(),
                            progress: Some(pct.clamp(0, 100) as u8),
                            message: Some(format!(
                                "已下载 {} / {}",
                                human_bytes(downloaded),
                                human_bytes(t)
                            )),
                        },
                    );
                }
            }
            None => {
                // 总大小未知 → 走 indeterminate，每 512KB 推一次心跳
                if downloaded - last_heartbeat >= HEARTBEAT_BYTES {
                    last_heartbeat = downloaded;
                    emit_progress(
                        &app,
                        RuntimeProgress {
                            runtime: "node".into(),
                            version: version.clone(),
                            phase: "downloading".into(),
                            progress: None,
                            message: Some(format!(
                                "已下载 {}（总大小未知）",
                                human_bytes(downloaded)
                            )),
                        },
                    );
                }
            }
        }
    }

    emit_progress(
        &app,
        RuntimeProgress {
            runtime: "node".into(),
            version: version.clone(),
            phase: "extracting".into(),
            progress: None,
            message: Some(format!("正在解压到 {dest:?}")),
        },
    );

    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    if let Err(e) = extract_node_archive(&buf, &dest) {
        // 解压失败 → 清理残留，避免下次被错误识别为「已安装」
        log::warn!("[runtime] Node.js {version} 解压失败，清理目录 {dest:?}");
        let _ = std::fs::remove_dir_all(&dest);
        emit_error(&app, "node", &version, &e);
        return Err(e);
    }

    // Ensure node binary is executable on Unix
    #[cfg(unix)]
    set_executable(&node_bin_in(&dest));

    // 二次校验：解压完仍无可执行文件 → 视为失败并清理
    if !node_bin_in(&dest).exists() {
        let _ = std::fs::remove_dir_all(&dest);
        let msg = format!("Node.js {version} 安装失败：解压完成但未找到可执行文件");
        emit_error(&app, "node", &version, &msg);
        return Err(msg);
    }

    emit_progress(
        &app,
        RuntimeProgress {
            runtime: "node".into(),
            version: version.clone(),
            phase: "verifying".into(),
            progress: None,
            message: Some("校验可执行文件...".into()),
        },
    );
    let healthy = verify_node_version(&version).await;
    if !healthy {
        let msg = format!("Node.js {version} 校验失败：版本号或可执行文件异常");
        emit_error(&app, "node", &version, &msg);
        return Err(msg);
    }

    log::info!("[runtime] Node.js {version} installed to {dest:?}");
    emit_progress(
        &app,
        RuntimeProgress {
            runtime: "node".into(),
            version: version.clone(),
            phase: "done".into(),
            progress: Some(100),
            message: Some(format!("Node.js v{version} 安装完成")),
        },
    );
    Ok(())
}

#[tauri::command]
pub async fn install_python_version(
    app: AppHandle,
    version: String,
    #[allow(unused_variables)] force: Option<bool>,
) -> Result<(), String> {
    let uv = runtime_env::get_uv_path()
        .ok_or_else(|| "Bundled uv not found. Run the download-runtimes script first.".to_string())?;
    let python_dir = runtime_env::uv_python_install_dir()
        .ok_or_else(|| "Cannot determine Python install directory".to_string())?;

    emit_progress(
        &app,
        RuntimeProgress {
            runtime: "python".into(),
            version: version.clone(),
            phase: "started".into(),
            progress: Some(0),
            message: Some(format!("准备安装 Python {version}")),
        },
    );

    // 预清理：移除阻塞 uv 创建 minor 软链的残留真实目录
    // 注意 uv 装新 patch 时会重建「所有已安装 minor 系列」的软链，
    // 不仅仅是当前 version 对应那个；因此必须全量扫描清理。
    cleanup_all_uv_python_minor_links(&python_dir);

    log::info!("[runtime] Installing Python {version} via uv...");

    let mut child = tokio::process::Command::new(&uv)
        .args([
            "python",
            "install",
            "--reinstall",
            &format!("cpython-{version}"),
        ])
        .env("UV_PYTHON_INSTALL_DIR", &python_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            let msg = format!("启动 uv 失败: {e}");
            emit_error(&app, "python", &version, &msg);
            msg
        })?;

    // 同步消费 stdout / stderr，按行向前端推送；同时缓存最近若干行用于失败时拼错误消息
    let recent_logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::with_capacity(64)));
    const MAX_RECENT: usize = 30;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let app_out = app.clone();
    let ver_out = version.clone();
    let logs_out = recent_logs.clone();
    let stdout_task = tokio::spawn(async move {
        if let Some(s) = stdout {
            let mut lines = BufReader::new(s).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                push_recent(&logs_out, &line, MAX_RECENT);
                emit_progress(
                    &app_out,
                    RuntimeProgress {
                        runtime: "python".into(),
                        version: ver_out.clone(),
                        phase: "running".into(),
                        progress: None,
                        message: Some(line),
                    },
                );
            }
        }
    });
    let app_err = app.clone();
    let ver_err = version.clone();
    let logs_err = recent_logs.clone();
    let stderr_task = tokio::spawn(async move {
        if let Some(s) = stderr {
            let mut lines = BufReader::new(s).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                push_recent(&logs_err, &line, MAX_RECENT);
                emit_progress(
                    &app_err,
                    RuntimeProgress {
                        runtime: "python".into(),
                        version: ver_err.clone(),
                        phase: "running".into(),
                        progress: None,
                        message: Some(line),
                    },
                );
            }
        }
    });

    let status = child.wait().await.map_err(|e| {
        let msg = format!("等待 uv 进程失败: {e}");
        emit_error(&app, "python", &version, &msg);
        msg
    })?;
    let _ = stdout_task.await;
    let _ = stderr_task.await;

    if !status.success() {
        // 把最近若干行日志拼到错误消息里，让前端能看到真正原因
        let tail = recent_logs
            .lock()
            .map(|v| v.join("\n"))
            .unwrap_or_default();
        let exit_str = status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".to_string());
        let msg = if tail.is_empty() {
            format!("uv python install 失败 (exit {exit_str})，无日志输出")
        } else {
            format!("uv python install 失败 (exit {exit_str}):\n{tail}")
        };
        emit_error(&app, "python", &version, &msg);
        return Err(msg);
    }

    emit_progress(
        &app,
        RuntimeProgress {
            runtime: "python".into(),
            version: version.clone(),
            phase: "done".into(),
            progress: Some(100),
            message: Some(format!("Python {version} 安装完成")),
        },
    );
    log::info!("[runtime] Python {version} installed successfully");
    Ok(())
}


/// 全量扫描 python_dir，清理所有作为「假 minor link」存在的真实目录。
/// 形如 `cpython-X.Y-<platform>-<arch>-none`（注意只匹配两段 major.minor，不带 patch）。
/// uv 安装新 patch 时会尝试重建所有已安装 minor 的软链，任何残留真实目录都会触发 EISDIR。
fn cleanup_all_uv_python_minor_links(python_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(python_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !is_uv_minor_link_name(name) {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let ft = meta.file_type();
        if ft.is_symlink() {
            continue; // 正常软链，跳过
        }
        if ft.is_dir() {
            log::warn!("[runtime] 全量清理阻塞 uv minor link 的残留目录: {path:?}");
            if let Err(e) = std::fs::remove_dir_all(&path) {
                log::warn!("[runtime] 清理 {path:?} 失败: {e}");
            }
        } else if let Err(e) = std::fs::remove_file(&path) {
            log::warn!("[runtime] 清理 {path:?} 失败: {e}");
        }
    }
}

/// 判断目录名是否符合 uv minor link 的命名规范：`cpython-<MAJOR>.<MINOR>-<platform>-<arch>-none`
/// 注意：只接受两段版本号（不含 patch），patch 版本目录不应被视作 link。
fn is_uv_minor_link_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("cpython-") else {
        return false;
    };
    if !name.ends_with("-none") {
        return false;
    }
    // 形如 "3.12-macos-aarch64-none" 或 "2.7-linux-x86_64-none"
    let mut parts = rest.splitn(2, '-');
    let Some(ver) = parts.next() else { return false };
    let dot_count = ver.chars().filter(|c| *c == '.').count();
    if dot_count != 1 {
        return false; // 必须是 X.Y 而不是 X.Y.Z
    }
    ver.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
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

/// Parse a version string like "22.14.0" into (major, minor, patch) for comparison.
fn parse_version(v: &str) -> (u64, u64, u64) {
    let parts: Vec<u64> = v.split('.').filter_map(|s| s.parse().ok()).collect();
    (
        parts.first().copied().unwrap_or(0),
        parts.get(1).copied().unwrap_or(0),
        parts.get(2).copied().unwrap_or(0),
    )
}

/// Insert a version string into a sorted (newest→oldest) list at the correct position.
fn insert_version_sorted(list: &mut Vec<String>, version: &str) {
    let parsed = parse_version(version);
    let pos = list
        .iter()
        .position(|v| parse_version(v) < parsed)
        .unwrap_or(list.len());
    list.insert(pos, version.to_string());
}

/// Detect the system's Node.js version by running `node -v`.
/// Returns the version string (e.g. "22.14.0") or None if not found.
fn detect_system_node_version() -> Option<String> {
    let output = std::process::Command::new("node")
        .arg("-v")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let ver = String::from_utf8_lossy(&output.stdout)
        .trim()
        .trim_start_matches('v')
        .to_string();
    if ver.is_empty() { None } else { Some(ver) }
}

/// Detect the bundled Node.js version by running the bundled `node -v`.
/// Returns the version string (e.g. "22.14.0") or None if not available.
fn detect_bundled_node_version() -> Option<String> {
    let bundled = runtime_env::get_bundled_node_path()?;
    let output = std::process::Command::new(&bundled)
        .arg("-v")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let ver = String::from_utf8_lossy(&output.stdout)
        .trim()
        .trim_start_matches('v')
        .to_string();
    if ver.is_empty() { None } else { Some(ver) }
}

/// Detect the system's Python version by running `python3 --version` (then `python`).
/// Returns the version string (e.g. "3.12.8") or None if not found.
fn detect_system_python_version() -> Option<String> {
    for cmd in &["python3", "python"] {
        if let Ok(output) = std::process::Command::new(cmd)
            .arg("--version")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
        {
            if output.status.success() {
                let raw = String::from_utf8_lossy(&output.stdout);
                // Output like "Python 3.12.8"
                let ver = raw.trim().strip_prefix("Python ").unwrap_or(raw.trim());
                let ver = ver.trim();
                if !ver.is_empty() {
                    return Some(ver.to_string());
                }
            }
        }
    }
    None
}

fn node_version_installed(version: &str) -> bool {
    // Check user-managed node-versions directory
    let in_user_dir = runtime_env::node_versions_base()
        .map(|b| node_bin_in(&b.join(version)).exists())
        .unwrap_or(false);
    if in_user_dir {
        return true;
    }
    // Check bundled node: run `node --version` and compare
    if let Some(bundled) = runtime_env::get_bundled_node_path() {
        if let Ok(output) = std::process::Command::new(&bundled).arg("-v").output() {
            if output.status.success() {
                let reported = String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .trim_start_matches('v')
                    .to_string();
                return reported == version;
            }
        }
    }
    false
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

/// Returns (major_minor, exec_path, is_bundled)
async fn get_installed_python_versions() -> Vec<(String, PathBuf, bool)> {
    let mut result: Vec<(String, PathBuf, bool)> = Vec::new();

    // ── 1. Scan bundled Python directory (read-only, in resource dir) ──
    if let Some(bundled_dir) = runtime_env::get_bundled_python_dir() {
        if let Ok(entries) = std::fs::read_dir(&bundled_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                // Directory names look like: cpython-3.12.13-macos-aarch64-none
                if !name.starts_with("cpython-") {
                    continue;
                }
                let ver = name.strip_prefix("cpython-").unwrap_or(&name);
                let major_minor = ver.splitn(3, '.').take(2).collect::<Vec<_>>().join(".");
                if major_minor.is_empty() {
                    continue;
                }
                // Find the python3 binary
                #[cfg(target_os = "windows")]
                let exec = entry.path().join("python.exe");
                #[cfg(not(target_os = "windows"))]
                let exec = entry.path().join("bin").join("python3");
                if exec.exists() {
                    result.push((major_minor, exec, true));
                }
            }
        }
    }

    // ── 2. Scan user-installed Python directory (writable, via uv) ──
    if let Some(uv) = runtime_env::get_uv_path() {
        if let Some(python_dir) = runtime_env::uv_python_install_dir() {
            if let Ok(output) = tokio::process::Command::new(&uv)
                .args(["python", "list", "--only-installed"])
                .env("UV_PYTHON_INSTALL_DIR", &python_dir)
                .output()
                .await
            {
                if output.status.success() {
                    for line in String::from_utf8_lossy(&output.stdout).lines() {
                        let trimmed = line.trim();
                        let mut iter = trimmed.splitn(2, char::is_whitespace);
                        let ver_part = match iter.next() {
                            Some(v) => v,
                            None => continue,
                        };
                        let exec_path = iter.next().unwrap_or("").trim();
                        if exec_path.is_empty() {
                            continue;
                        }
                        let ver = ver_part.strip_prefix("cpython-").unwrap_or(ver_part);
                        let major_minor = ver.splitn(3, '.').take(2).collect::<Vec<_>>().join(".");
                        if !major_minor.is_empty() {
                            // Avoid duplicates from bundled dir
                            if !result.iter().any(|(mm, _, _)| mm == &major_minor) {
                                result.push((major_minor, PathBuf::from(exec_path), false));
                            }
                        }
                    }
                }
            }
        }
    }

    result
}

/// 校验 Node.js 指定版本是否可用：执行 `node -v` 并验证版本号匹配
async fn verify_node_version(version: &str) -> bool {
    // Check user-managed node-versions directory
    if let Some(base) = runtime_env::node_versions_base() {
        let bin = node_bin_in(&base.join(version));
        if bin.exists() {
            if let Ok(output) = tokio::process::Command::new(&bin)
                .arg("-v")
                .output()
                .await
            {
                if output.status.success() {
                    let reported = String::from_utf8_lossy(&output.stdout)
                        .trim()
                        .trim_start_matches('v')
                        .to_string();
                    return reported == version;
                }
            }
        }
    }
    // Check bundled node
    if let Some(bundled) = runtime_env::get_bundled_node_path() {
        if let Ok(output) = tokio::process::Command::new(&bundled)
            .arg("-v")
            .output()
            .await
        {
            if output.status.success() {
                let reported = String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .trim_start_matches('v')
                    .to_string();
                return reported == version;
            }
        }
    }
    false
}

/// 校验 Python 解释器是否可用：执行 `python --version`
async fn verify_python_executable(exec: &Path) -> bool {
    if !exec.exists() {
        return false;
    }
    let Ok(output) = tokio::process::Command::new(exec)
        .arg("--version")
        .output()
        .await
    else {
        return false;
    };
    output.status.success()
}

async fn save_version_to_db(key: &str, value: &str) -> Result<(), String> {
    let patch = serde_json::json!({ "install": { key: value } });
    config_service::update(&patch)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// 推送 phase=error 终态事件
fn emit_error(app: &AppHandle, runtime: &str, version: &str, msg: &str) {
    emit_progress(
        app,
        RuntimeProgress {
            runtime: runtime.to_string(),
            version: version.to_string(),
            phase: "error".into(),
            progress: None,
            message: Some(msg.to_string()),
        },
    );
}

/// 字节数转人类可读字符串
fn human_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.2} MB", b / MB)
    } else if b >= KB {
        format!("{:.2} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

/// 把一行追加到环形缓冲，超过容量时丢最早的
fn push_recent(buf: &Arc<Mutex<Vec<String>>>, line: &str, max: usize) {
    if let Ok(mut v) = buf.lock() {
        v.push(line.to_string());
        let len = v.len();
        if len > max {
            v.drain(0..(len - max));
        }
    }
}

