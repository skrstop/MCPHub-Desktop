/// HTTP server management commands
use crate::services::{config_service, http_server};
use serde_json::{json, Value};

/// Start the embedded HTTP server on the given port.
/// Reads the body limit from current config.
#[tauri::command]
pub async fn start_http_server(port: u16) -> Result<Value, String> {
    let body_limit_bytes = config_service::get()
        .await
        .ok()
        .and_then(|c| c.get("routing").and_then(|r| r.get("jsonBodyLimit")).and_then(|v| v.as_str()).map(|s| http_server::parse_body_limit(s)))
        .unwrap_or(1024 * 1024);
    http_server::start(port, body_limit_bytes)
        .await
        .map_err(|e| e.to_string())?;
    Ok(json!({ "success": true, "port": port }))
}

/// Stop the embedded HTTP server.
#[tauri::command]
pub async fn stop_http_server() -> Result<Value, String> {
    http_server::stop().await;
    Ok(json!({ "success": true }))
}

/// Get the current HTTP server status — running / port / last error.
///
/// The `error` field carries the last bind/start failure message (Windows
/// firewall / port-in-use, etc.); it is None when the server is running or has
/// never been started. The frontend fetches this on mount to catch a startup
/// failure it may have missed (the server starts before the webview registers
/// its event listener); live updates arrive on the `http://server-status` event.
#[tauri::command]
pub async fn get_http_server_status() -> Result<Value, String> {
    let s = http_server::current_status();
    serde_json::to_value(&s).map_err(|e| e.to_string())
}

/// One process listening on the HTTP port (from `detect_port_occupier`).
#[derive(serde::Serialize, Clone)]
pub struct PortOccupier {
    pub pid: u32,
    pub name: String,
}

/// Find processes currently listening on `port` (any interface/loopback).
///
/// Used by the failure dialog's "kill occupier & restart" flow. Returns an
/// empty list when nothing is listening (firewall/permission failures) or the
/// platform probe is unavailable.
#[tauri::command]
pub async fn detect_port_occupier(port: u16) -> Result<Vec<PortOccupier>, String> {
    tokio::task::spawn_blocking(move || {
        let mut out = Vec::new();
        // Never list our own process: our server binds *:port (all
        // interfaces), so the LISTEN probe matches it too — showing
        // "mcphub" as a squatter confuses users and the kill guard would
        // refuse it anyway.
        let self_pid = std::process::id();
        #[cfg(target_os = "macos")]
        {
            if let Ok(output) = std::process::Command::new("lsof")
                .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN"])
                .output()
            {
                let text = String::from_utf8_lossy(&output.stdout);
                for line in text.lines().skip(1) {
                    let cols: Vec<&str> = line.split_whitespace().collect();
                    // COMMAND PID USER ... NAME
                    if cols.len() >= 2 {
                        if let Ok(pid) = cols[1].parse::<u32>() {
                            let name = cols[0].to_string();
                            if pid != self_pid && !out.iter().any(|o: &PortOccupier| o.pid == pid) {
                                out.push(PortOccupier { pid, name });
                            }
                        }
                    }
                }
            }
        }
        #[cfg(target_os = "linux")]
        {
            if let Ok(output) = std::process::Command::new("ss")
                .args(["-tlnp"])
                .output()
            {
                let text = String::from_utf8_lossy(&output.stdout);
                for line in text.lines().skip(1) {
                    if !line.contains(&format!(":{port} ")) && !line.ends_with(&format!(":{port}")) {
                        continue;
                    }
                    // users:(("name",pid=1234,fd=...))
                    if let Some(start) = line.find("pid=") {
                        let after = &line[start + 4..];
                        if let Some(end) = after.find(',') {
                            if let Ok(pid) = after[..end].parse::<u32>() {
                                let name = line
                                    .split("((\"").nth(1)
                                    .and_then(|r| r.split('"').next())
                                    .unwrap_or("unknown")
                                    .to_string();
                                if pid != self_pid && !out.iter().any(|o: &PortOccupier| o.pid == pid) {
                                    out.push(PortOccupier { pid, name });
                                }
                            }
                        }
                    }
                }
            }
        }
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            if let Ok(output) = {
                let mut c = std::process::Command::new("netstat");
                c.args(["-ano", "-p", "TCP"]);
                #[cfg(windows)]
                {
                    c.creation_flags(0x0800_0000);
                }
                c.output()
            } {
                let text = String::from_utf8_lossy(&output.stdout);
                for line in text.lines() {
                    let cols: Vec<&str> = line.split_whitespace().collect();
                    // proto local foreign state pid
                    if cols.len() >= 5 && cols[3] == "LISTENING" {
                        let local = cols[1];
                        let port_str = format!(":{port}");
                        if local.ends_with(&port_str) {
                            if let Ok(pid) = cols[4].parse::<u32>() {
                                let name = std::process::Command::new("tasklist")
                                    .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
                                    .output()
                                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                                    .map(|s| {
                                        s.trim_matches('"')
                                            .split('"')
                                            .next()
                                            .unwrap_or("unknown")
                                            .to_string()
                                    })
                                    .unwrap_or_else(|_| "unknown".to_string());
                                if pid != self_pid && !out.iter().any(|o: &PortOccupier| o.pid == pid) {
                                    out.push(PortOccupier { pid, name });
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Kill the process occupying the HTTP port (from the failure dialog's
/// "kill occupier & restart" flow). Refuses PIDs that are never killable:
/// 0/1, our own process, and our parent.
#[tauri::command]
pub async fn kill_port_occupier(pid: u32) -> Result<(), String> {
    if pid <= 1 {
        return Err("refusing to kill system process".to_string());
    }
    let self_pid = std::process::id();
    if pid == self_pid {
        return Err("refusing to kill own process".to_string());
    }
    #[cfg(unix)]
    {
        if pid == (unsafe { libc::getppid() as u32 }) {
            return Err("refusing to kill parent process".to_string());
        }
    }
    #[cfg(unix)]
    {
        let status = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!("failed to kill process {pid}"));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut c = std::process::Command::new("taskkill");
        c.args(["/F", "/PID", &pid.to_string()]);
        #[cfg(windows)]
        {
            c.creation_flags(0x0800_0000);
        }
        let status = c.status().map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!("failed to kill process {pid}"));
        }
    }
    crate::services::app_logger::log_to_db("warn", &format!("[http] killed port occupier pid={pid}"));
    Ok(())
}
