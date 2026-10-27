/// stdio transport helpers — shared by the rmcp-based stdio client
/// (`rmcp_stdio_transport.rs`). The legacy hand-rolled JSON-RPC transport was
/// retired during the rmcp migration; only these utilities remain.

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Resolve a command name to a full path by searching in PATH.
/// On Windows, also checks for .exe, .cmd, .bat extensions.
pub(crate) fn resolve_in_path(cmd: &str, path_env: &str) -> Option<String> {
    let sep = if cfg!(target_os = "windows") { ';' } else { ':' };
    let extensions: &[&str] = if cfg!(target_os = "windows") {
        &["", ".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };

    let dirs: Vec<&str> = path_env.split(sep).filter(|d| !d.is_empty()).collect();
    log::info!("[resolve_in_path] Searching for '{}' in {} PATH dirs", cmd, dirs.len());

    for dir in &dirs {
        let dir_path = std::path::Path::new(dir);
        // Check if directory exists first
        if !dir_path.exists() {
            continue;
        }
        for ext in extensions {
            let candidate = dir_path.join(format!("{}{}", cmd, ext));
            if candidate.exists() {
                log::info!("[resolve_in_path] Found: {}", candidate.display());
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }

    log::warn!("[resolve_in_path] '{}' not found in any PATH directory", cmd);
    // Log all dirs for debugging
    for (i, dir) in dirs.iter().enumerate() {
        let exists = std::path::Path::new(dir).exists();
        log::warn!("[resolve_in_path]   PATH[{}]: {} (exists={})", i, dir, exists);
    }
    None
}

/// Kill the entire process tree of a stdio transport's child process.
/// When the server is launched through a wrapper like `npx` / `npm exec`,
/// the wrapper does not forward signals to its descendants, so the real
/// server process is left running as an orphan. Walk the whole tree and
/// force-kill it.
pub(crate) fn kill_process_tree(pid: u32) {
    #[cfg(unix)]
    {
        // Send SIGTERM to the process group first
        unsafe {
            libc::kill(-(pid as i32), libc::SIGTERM);
        }
    }
    #[cfg(windows)]
    {
        // On Windows, use taskkill /F /T /PID to kill the process tree
        // CREATE_NO_WINDOW prevents a visible console window from flashing
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .creation_flags(0x0800_0000)
            .spawn();
    }
}

/// Best-effort extraction of a download percentage from an npx/uvx stderr line.
///
/// Recognizes `42%`-style and `12/34`-style progress. Returns `None` for lines
/// without parseable progress (the caller shows an indeterminate bar + the raw
/// line as the message).
pub(crate) fn parse_progress_pct(line: &str) -> Option<u8> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            // try to read a number here
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let num_str = &line[start..i];
            // 解析失败（如超长数字段溢出 u64）只跳过该段，不放弃整行扫描
            let Ok(num) = num_str.parse::<u64>() else {
                continue;
            };
            // skip spaces
            let mut j = i;
            while j < bytes.len() && bytes[j] == b' ' {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'%' {
                return Some((num.min(100)) as u8);
            }
            if j < bytes.len() && bytes[j] == b'/' {
                // Fraction done/total. Reject slash-chain tokens (dates like
                // 2026/10/27, paths like a/b/c): a candidate whose number is
                // immediately preceded by '/' is part of a longer chain, not
                // a standalone "12/34" progress token.
                if start > 0 && bytes[start - 1] == b'/' {
                    continue;
                }
                let mut k = j + 1;
                while k < bytes.len() && bytes[k] == b' ' {
                    k += 1;
                }
                let tstart = k;
                while k < bytes.len() && bytes[k].is_ascii_digit() {
                    k += 1;
                }
                if k > tstart {
                    if let Ok(total) = line[tstart..k].parse::<u64>() {
                        // A real progress fraction has num <= total. Anything
                        // else (dates, version paths, counts) would saturate
                        // to 100% and pin the bar full — skip this number and
                        // keep scanning the line instead.
                        if total > 0 && num <= total {
                            return Some((num.saturating_mul(100) / total).min(100) as u8);
                        }
                    }
                }
            }
            // continue scanning after this number
        } else {
            i += 1;
        }
    }
    None
}

/// Heuristically decide whether an npx/uvx stderr line represents package
/// download/install progress (vs. ordinary server log output). Only lines
/// that look like progress trigger a "downloading" event, so a cached
/// package that starts instantly - or a server that prints info to stderr -
/// does not falsely show a "下载中" indicator.
pub(crate) fn looks_like_download_progress(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("download")
        || lower.contains("downloading")
        || ((lower.contains("added") || lower.contains("installed")) && lower.contains("package"))
        || parse_progress_pct(line).is_some()
}

#[cfg(test)]
mod tests {
    use super::{looks_like_download_progress, parse_progress_pct};

    #[test]
    fn parse_progress_percent_forms() {
        assert_eq!(parse_progress_pct("downloading 42%"), Some(42));
        assert_eq!(parse_progress_pct("100%"), Some(100));
        assert_eq!(parse_progress_pct("fetch: 12/34"), Some(35));
        assert_eq!(parse_progress_pct("fetch: 5/100"), Some(5));
        assert_eq!(parse_progress_pct("no progress here"), None);
    }

    /// Regression: date-prefixed stderr lines (`2026/10/27`) previously
    /// parsed as fraction 2026/10 → saturating 100%, pinning the download
    /// bar at 100% for the whole install. num > total must not count as a
    /// fraction, and the line must not be misread as progress at all.
    #[test]
    fn parse_rejects_non_fraction_slash_patterns() {
        assert_eq!(parse_progress_pct("[2026/10/27 15:00:00] server started"), None);
        assert_eq!(parse_progress_pct("listening on port 3000 v1.2.3"), None);
        assert_eq!(parse_progress_pct("GET /api/users/42 200"), None);
        // real fractions still work after a rejected candidate earlier in line
        assert_eq!(parse_progress_pct("[2026/10/27] fetch 12/34"), Some(35));
    }

    #[test]
    fn looks_like_progress_classification() {
        assert!(looks_like_download_progress("fetch: 12/34"));
        assert!(looks_like_download_progress("Downloading package..."));
        assert!(looks_like_download_progress("added 12 packages in 3s"));
        // date-only lines are ordinary logs, not download progress
        assert!(!looks_like_download_progress("[2026/10/27 15:00:00] server started"));
    }
}
