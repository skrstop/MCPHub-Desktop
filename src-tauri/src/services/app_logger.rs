/// Database log writer that receives log entries via a channel and writes them to app_log.
///
/// This works alongside env_logger — env_logger handles stderr output,
/// while this module handles database persistence.
use tokio::sync::mpsc;

static LOG_SENDER: std::sync::OnceLock<mpsc::UnboundedSender<LogEntry>> = std::sync::OnceLock::new();

struct LogEntry {
    level: String,
    message: String,
    server_name: Option<String>,
}

/// Initialize the database log writer.
///
/// Call this once after the database is initialized.
/// Then use `log_to_db()` to send log messages to the database.
///
/// Uses a dedicated thread with its own Tokio runtime because `init()` is called
/// from Tauri's `setup` closure which runs outside any Tokio runtime context.
pub fn init() {
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
