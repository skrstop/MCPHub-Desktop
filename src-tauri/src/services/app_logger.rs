/// Database log writer that receives log entries via a channel and writes them to app_log.
///
/// This works alongside env_logger — env_logger handles stderr output,
/// while this module handles database persistence.
use std::io::Write;
use std::path::PathBuf;
use tokio::sync::mpsc;

static LOG_SENDER: std::sync::OnceLock<mpsc::UnboundedSender<LogEntry>> = std::sync::OnceLock::new();

/// `<app_data_dir>/logs/` - where daily log files (`app-YYYY-MM-DD.log`) live.
/// Set by `init()`; when unset (dir unavailable) file mirroring is silently
/// skipped (DB logging still works).
static LOG_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

struct LogEntry {
    level: String,
    message: String,
    server_name: Option<String>,
}

/// Initialize the database log writer + file mirror.
///
/// Call this once after the database is initialized, passing the app data dir
/// (file logs go to `<app_data_dir>/logs/app-YYYY-MM-DD.log`). Then use
/// `log_to_db()` to send log messages - each entry is written to BOTH the DB
/// (`app_log` table) and today's log file.
///
/// Uses a dedicated thread with its own Tokio runtime because `init()` is called
/// from Tauri's `setup` closure which runs outside any Tokio runtime context.
pub fn init(app_data_dir: Option<PathBuf>) {
    // File mirror goes to `<app_data_dir>/logs/`. When the dir can't be resolved
    // (shouldn't happen post-setup) file mirroring is skipped - DB logging still
    // works. Never create a relative "logs" dir in cwd.
    if let Some(app_data_dir) = app_data_dir {
        let log_dir = app_data_dir.join("logs");
        if std::fs::create_dir_all(&log_dir).is_ok() {
            let _ = LOG_DIR.set(log_dir);
        }
    }

    let (tx, mut rx) = mpsc::unbounded_channel::<LogEntry>();

    LOG_SENDER.set(tx).ok();

    // Spawn a dedicated thread with its own Tokio runtime for DB writes.
    // This avoids the "no reactor running" panic when called from setup().
    std::thread::Builder::new()
        .name("app-logger".to_string())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create logger runtime");
            rt.block_on(async move {
                while let Some(entry) = rx.recv().await {
                    if let Err(e) = crate::services::log_service::add_log(
                        &entry.level,
                        &entry.message,
                        entry.server_name.as_deref(),
                    )
                    .await
                    {
                        eprintln!("[app_logger] Failed to write log to DB: {}", e);
                    }
                    // Mirror to the daily log file (independent of DB success).
                    write_log_file(&entry);
                }
            });
        })
        .expect("Failed to spawn app-logger thread");
}

/// Send a log entry to the database.
///
/// Extracts server name from messages like "[server-name] Connected ..."
pub fn log_to_db(level: &str, message: &str) {
    let server_name = extract_server_name(message);
    if let Some(sender) = LOG_SENDER.get() {
        let _ = sender.send(LogEntry {
            level: level.to_string(),
            message: message.to_string(),
            server_name,
        });
    }
}

/// Extract server name from log messages like "[server-name] ..."
fn extract_server_name(message: &str) -> Option<String> {
    if message.starts_with('[') {
        let end = message.find(']')?;
        if end > 1 {
            return Some(message[1..end].to_string());
        }
    }
    None
}

/// Append a log entry to today's daily log file (`<log_dir>/app-YYYY-MM-DD.log`).
/// Sync IO on the dedicated app-logger thread - fine since it's the only writer
/// and the thread's sole job is draining the log channel. Best-effort: any IO
/// error is swallowed (the DB already has the entry) so logging never panics.
fn write_log_file(entry: &LogEntry) {
    let Some(dir) = LOG_DIR.get() else { return };
    let now = chrono::Local::now();
    let path = dir.join(format!("app-{}.log", now.format("%Y-%m-%d")));
    let server = entry
        .server_name
        .as_deref()
        .map(|s| format!("[{}] ", s))
        .unwrap_or_default();
    let line = format!(
        "[{}] [{}] {}{}\n",
        now.format("%Y-%m-%d %H:%M:%S"),
        entry.level,
        server,
        entry.message,
    );
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Delete daily log files older than `retention_days` (by the date in the
/// filename `app-YYYY-MM-DD.log`). Called from the same periodic cleanup that
/// trims the DB logs, so on-disk and DB retention stay in sync. ISO date
/// strings compare lexicographically == chronologically, so a string compare
/// against the cutoff date is exact.
pub fn cleanup_old_log_files(retention_days: i64) {
    let Some(dir) = LOG_DIR.get() else { return };
    let cutoff = chrono::Local::now() - chrono::Duration::days(retention_days);
    let cutoff_str = cutoff.format("%Y-%m-%d").to_string();
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(date_str) = name.strip_prefix("app-").and_then(|s| s.strip_suffix(".log")) {
            if date_str < cutoff_str.as_str() {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// Custom writer that duplicates env_logger output to the database.
///
/// Usage: wrap env_logger's target with this struct to also write to DB.
pub struct DualWriter<W: std::io::Write> {
    inner: W,
}

impl<W: std::io::Write> DualWriter<W> {
    pub fn new(inner: W) -> Self {
        Self { inner }
    }
}

impl<W: std::io::Write> std::io::Write for DualWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        // Also send to database
        if let Ok(msg) = std::str::from_utf8(buf) {
            let trimmed = msg.trim();
            if !trimmed.is_empty() {
                // Determine level from env_logger format: "[LEVEL] message"
                let (level, message) = if trimmed.starts_with("[ERROR]") {
                    ("error", trimmed.strip_prefix("[ERROR]").unwrap_or(trimmed).trim())
                } else if trimmed.starts_with("[WARN]") {
                    ("warn", trimmed.strip_prefix("[WARN]").unwrap_or(trimmed).trim())
                } else if trimmed.starts_with("[INFO]") {
                    ("info", trimmed.strip_prefix("[INFO]").unwrap_or(trimmed).trim())
                } else if trimmed.starts_with("[DEBUG]") {
                    ("debug", trimmed.strip_prefix("[DEBUG]").unwrap_or(trimmed).trim())
                } else {
                    ("info", trimmed)
                };
                log_to_db(level, message);
            }
        }
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
