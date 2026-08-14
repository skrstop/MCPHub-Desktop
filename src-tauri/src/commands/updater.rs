// Cancelable app update install.
//
// The official tauri-plugin-updater IPC commands (download_and_install) have
// no cancellation API. These commands wrap the plugin's public Rust API
// (`Update::download_and_install`) in a spawned tokio task so the frontend can
// abort an in-flight download: aborting the JoinHandle drops the future, which
// drops the reqwest byte stream and closes the connection. The install step is
// a synchronous section inside the same future — an abort can only land before
// or after it, never mid-install, so a cancelled update never leaves the app
// half-installed.
//
// Conventions:
// - `install_id` (frontend-generated, unique per attempt) tags every terminal
//   event so a stale attempt's result can't confuse the UI.
// - Terminal outcomes are emitted exactly once as `updater://install-result`:
//   "ok" | "cancelled" | "error". The frontend treats the first terminal event
//   for its install_id as authoritative.
// - The completed task does NOT clear its own slot (a finishing task aborting
//   whatever the slot holds could kill a newly started install). The slot may
//   hold a finished handle — abort on it is a no-op.

use std::sync::OnceLock;

use serde::Serialize;
use tauri::{ipc::Channel, Emitter, Manager, ResourceId, Webview};
use tauri_plugin_updater::Update;

/// Same wire shape as the plugin's DownloadEvent, so the frontend
/// `DownloadEvent` type keeps working over our Channel.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "event", content = "data")]
pub enum UpdateDownloadEvent {
    #[serde(rename_all = "camelCase")]
    Started {
        content_length: Option<u64>,
    },
    #[serde(rename_all = "camelCase")]
    Progress {
        chunk_length: usize,
    },
    Finished,
}

/// Terminal outcome of an install attempt, emitted as `updater://install-result`.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallResult {
    install_id: u64,
    /// "ok" | "cancelled" | "error"
    status: String,
    error: Option<String>,
}

/// Single-flight install task handle. The About dialog is the only trigger, so
/// one slot suffices; starting a new install aborts any previous one.
static CURRENT_TASK: OnceLock<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>> =
    OnceLock::new();

/// install_id of the most recently started install, so `cancel_update_install`
/// can tag the "cancelled" event without the frontend having to pass it back.
static CURRENT_INSTALL_ID: OnceLock<std::sync::Mutex<u64>> = OnceLock::new();

fn task_slot() -> &'static tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>> {
    CURRENT_TASK.get_or_init(|| tokio::sync::Mutex::new(None))
}

fn install_id_slot() -> &'static std::sync::Mutex<u64> {
    CURRENT_INSTALL_ID.get_or_init(|| std::sync::Mutex::new(0))
}

/// Download and install the update identified by the plugin's `check()` result
/// (rid in the resources table). Returns immediately; progress streams over the
/// channel, the terminal result over the `updater://install-result` event.
#[tauri::command]
pub async fn install_update_cancelable(
    webview: Webview,
    install_id: u64,
    rid: ResourceId,
    on_event: Channel<UpdateDownloadEvent>,
) -> Result<(), String> {
    let update = webview
        .resources_table()
        .get::<Update>(rid)
        .map_err(|e| e.to_string())?;
    let app = webview.app_handle().clone();

    let mut slot = task_slot().lock().await;
    if let Some(previous) = slot.take() {
        previous.abort();
    }
    *install_id_slot().lock().unwrap() = install_id;

    let handle = tokio::spawn(async move {
        let mut first_chunk = true;
        let result = update
            .download_and_install(
                |chunk_length, content_length| {
                    if first_chunk {
                        first_chunk = false;
                        let _ = on_event.send(UpdateDownloadEvent::Started { content_length });
                    }
                    let _ = on_event.send(UpdateDownloadEvent::Progress { chunk_length });
                },
                || {
                    let _ = on_event.send(UpdateDownloadEvent::Finished);
                },
            )
            .await;

        let payload = match result {
            Ok(()) => InstallResult {
                install_id,
                status: "ok".into(),
                error: None,
            },
            Err(e) => InstallResult {
                install_id,
                status: "error".into(),
                error: Some(e.to_string()),
            },
        };
        let _ = app.emit("updater://install-result", payload);
    });

    *slot = Some(handle);
    Ok(())
}

/// Abort the running install (only meaningful during the download phase — the
/// install section is synchronous and cannot be interrupted). Returns whether
/// a still-running task was actually aborted.
#[tauri::command]
pub async fn cancel_update_install(app: tauri::AppHandle) -> Result<bool, String> {
    let install_id = *install_id_slot().lock().unwrap();
    let mut slot = task_slot().lock().await;
    if let Some(handle) = slot.take() {
        if handle.is_finished() {
            // Task already completed and emitted its own ok/error result —
            // nothing to cancel.
            return Ok(false);
        }
        handle.abort();
        let _ = app.emit(
            "updater://install-result",
            InstallResult {
                install_id,
                status: "cancelled".into(),
                error: None,
            },
        );
        Ok(true)
    } else {
        Ok(false)
    }
}
