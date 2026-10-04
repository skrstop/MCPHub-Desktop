//! mv — shared local model + vector runtime (Model & Vector).
//!
//! Owns the process-wide single GGUF embedding model and the single lancedb
//! connection, with a consumer registration count (currently: `rag`, and
//! `smart` from Phase 3). Any enabled consumer keeps the runtime alive; the
//! last consumer to disable releases it (mimalloc归还, matching the old
//! rag::stop semantics).
//!
//! Structural invariants (unit-tested in Phase 2):
//! - at most ONE model instance and ONE lancedb connection process-wide
//!   (guaranteed by the single `RUNTIME` OnceLock slot, not by convention)
//! - `ensure_started`/`release` interleave freely; running state always
//!   equals "at least one consumer registered"
//! - model-loading types (`Embedder`, `load_embedder`) live only in this
//!   module; consumers get embed access via `with_model` / `embed_dim`
//! - consumers own their own lancedb table handles (`rag_chunk` via
//!   `rag::vectordb::VectorDb`), built on the shared connection from
//!   `connection()`
//!
//! Model selection is persisted as `mv.model` (read fallback `rag.model` for
//! existing installs — zero migration). The runtime reloads the model
//! whenever `ensure_started` observes the persisted selection differs from
//! the loaded one, which is what makes a model switch take effect even when
//! another consumer kept the runtime alive.

pub mod embedder;
pub mod gguf;
pub mod gguf_gemma;
pub mod gguf_lfm2;
pub mod gguf_modernbert;
pub mod gguf_nomic;
pub mod gguf_qwen3;
pub mod models;

use anyhow::{anyhow, Result};
use lancedb::Connection;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Manager};

pub use embedder::Embedder;
pub use models::{
    current_model, download_model, ensure_model_ready, list_models, model_chunk_recommendation,
    model_max_context, persist_selection, RagModelInfo,
};

/// The shared runtime: one embedding model + one lancedb connection.
pub struct MvRuntime {
    model: Box<dyn Embedder>,
    conn: Connection,
    /// Persisted selection this runtime was loaded for (e.g. "default").
    active_model: String,
    embed_dim: usize,
}

static RUNTIME: OnceLock<tokio::sync::Mutex<Option<MvRuntime>>> = OnceLock::new();
static INITIALIZING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Registered consumer ids ("rag", "smart", future ones). Owning an entry
/// means "this consumer wants the runtime up".
static CONSUMERS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
/// Embedding dimension of the loaded model (0 = nothing loaded). Atomic so
/// the status line can show it without taking the runtime lock.
static EMBED_DIM: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn runtime_slot() -> &'static tokio::sync::Mutex<Option<MvRuntime>> {
    RUNTIME.get_or_init(|| tokio::sync::Mutex::new(None))
}

fn consumers() -> &'static Mutex<HashSet<String>> {
    CONSUMERS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn mv_log(level: &str, msg: impl std::fmt::Display) {
    crate::rag::service::rag_log(level, msg);
}

/// `<app_data>/rag` — shared data root (paths unchanged from the pre-mv
/// layout so existing installs need zero migration).
fn data_dir(app: &AppHandle) -> Result<PathBuf> {
    Ok(app.path().app_data_dir()?.join("rag"))
}

/// lancedb dir shared by every consumer's tables.
pub fn lancedb_dir(app: &AppHandle) -> Result<PathBuf> {
    Ok(data_dir(app)?.join("lancedb"))
}

/// Snapshot info returned by `ensure_started` / `info()`.
#[derive(Clone, Debug)]
pub struct MvInfo {
    pub active_model: String,
    pub embed_dim: usize,
}

/// True when the shared runtime holds a loaded model.
pub fn is_running() -> bool {
    EMBED_DIM.load(Ordering::SeqCst) != 0
}

/// Whether any consumer is registered (authoritative "should be running").
pub fn consumer_count() -> usize {
    consumers().lock().map(|c| c.len()).unwrap_or(0)
}

/// The embedding dimension of the loaded model (None when not running).
pub fn embed_dim() -> Option<usize> {
    match EMBED_DIM.load(Ordering::SeqCst) {
        0 => None,
        n => Some(n),
    }
}

/// Async variant: clone the shared connection handle.
pub async fn connection_async() -> Result<Connection> {
    let guard = runtime_slot().lock().await;
    let Some(rt) = guard.as_ref() else {
        return Err(anyhow!("mv runtime not running"));
    };
    Ok(rt.conn.clone())
}

/// The model's max context (tokens). Errors when not running.
pub async fn model_max_context_loaded() -> Result<u32> {
    let guard = runtime_slot().lock().await;
    let Some(rt) = guard.as_ref() else {
        return Err(anyhow!("mv runtime not running"));
    };
    Ok(rt.model.max_context().max(1) as u32)
}

/// Run a synchronous closure with exclusive mutable access to the loaded
/// model. The closure must not await (mv's lock is not held across awaits)
/// and must not call back into mv. This is THE embed/tokenizer access path
/// for every consumer. Mutable because `embed`/`embed_batch`/tokenizer state
/// require `&mut` (matches the pre-mv `rt.model` usage).
pub async fn with_model<R>(
    f: impl FnOnce(&mut dyn Embedder) -> Result<R>,
) -> Result<R> {
    let mut guard = runtime_slot().lock().await;
    let Some(rt) = guard.as_mut() else {
        return Err(anyhow!("mv runtime not running"));
    };
    f(rt.model.as_mut())
}

/// Run a closure with access to model + connection (sync closure; same rules
/// as `with_model`).
pub async fn with_runtime<R>(
    f: impl FnOnce(&mut dyn Embedder, &Connection) -> Result<R>,
) -> Result<R> {
    let mut guard = runtime_slot().lock().await;
    let Some(rt) = guard.as_mut() else {
        return Err(anyhow!("mv runtime not running"));
    };
    f(rt.model.as_mut(), &rt.conn)
}

/// Whether a model reload is needed: nothing loaded, or the persisted
/// selection differs from the loaded one.
async fn needs_reload() -> Result<bool> {
    let guard = runtime_slot().lock().await;
    match guard.as_ref() {
        None => Ok(true),
        Some(rt) => {
            let selected = models::current_model()
                .await
                .ok_or_else(|| anyhow!("mv: no model selected"))?;
            Ok(selected != rt.active_model)
        }
    }
}

/// Ensure the shared runtime is running for `consumer`. Idempotent: a running
/// runtime with the correct model just registers the consumer. When the
/// persisted model selection changed (or nothing is loaded), the model is
/// (re)loaded — the lancedb connection is kept if the runtime was already up
/// (tables are per-consumer and dim-checked by their own ensure logic).
pub async fn ensure_started(consumer: &str, app: &AppHandle) -> Result<MvInfo> {
    // Register the consumer FIRST: even if loading fails below, the intent is
    // recorded and a retry (next toggle / auto-restore) has work to do.
    {
        let mut c = consumers()
            .lock()
            .map_err(|_| anyhow!("mv consumers poisoned"))?;
        c.insert(consumer.to_string());
    }
    mv_log("info", format!("[mv] ensure_started({}) — consumers={:?}", consumer, consumers_snapshot()));

    if !needs_reload().await? {
        let guard = runtime_slot().lock().await;
        let rt = guard.as_ref().expect("checked running");
        return Ok(MvInfo {
            active_model: rt.active_model.clone(),
            embed_dim: rt.embed_dim,
        });
    }

    if INITIALIZING.swap(true, Ordering::SeqCst) {
        // Another task is already loading. Wait for it to finish by polling
        // the atomic (loading can take tens of seconds — model load).
        while INITIALIZING.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        if !needs_reload().await? {
            let guard = runtime_slot().lock().await;
            let rt = guard.as_ref().expect("checked");
            return Ok(MvInfo {
                active_model: rt.active_model.clone(),
                embed_dim: rt.embed_dim,
            });
        }
        return Err(anyhow!("mv: concurrent model load failed"));
    }

    // Panic guard: if load_locked panics, INITIALIZING must be cleared or
    // every waiter spins forever. (Normal failure paths return Err.)
    struct InitGuard;
    impl Drop for InitGuard {
        fn drop(&mut self) {
            INITIALIZING.store(false, Ordering::SeqCst);
        }
    }
    let _guard = InitGuard;
    let info = load_locked(app).await?;
    mv_log(
        "info",
        format!(
            "[mv] runtime ready: model={} dim={} consumers={:?}",
            info.active_model,
            info.embed_dim,
            consumers_snapshot()
        ),
    );
    Ok(info)
}

fn consumers_snapshot() -> Vec<String> {
    consumers()
        .lock()
        .map(|c| c.iter().cloned().collect::<Vec<_>>())
        .unwrap_or_default()
}

/// Load (or reload) the model for the persisted selection + (re)open the
/// lancedb connection. Caller must hold INITIALIZING.
async fn load_locked(app: &AppHandle) -> Result<MvInfo> {
    let size = models::current_model()
        .await
        .ok_or_else(|| anyhow!("mv: no model selected (mv.model / rag.model)"))?;
    let size_dir = models::resolve_model_paths(app, &size)?
        .ok_or_else(|| anyhow!("model '{}' not ready - download it first", size))?;

    mv_log("info", format!("[mv] loading model '{}'…", size));
    // Phase 4: the user's `mv.device` setting (AUTO/GPU/CPU selector) drives
    // the engine at load. Precedence: RAG_GGUF_DEVICE env (debug) > user
    // setting > the model's deploy.json.
    let user_platform = match device().await.as_str() {
        "gpu" => Some(embedder::Platform::Gpu),
        "cpu" => Some(embedder::Platform::Cpu),
        _ => None,
    };
    let model = embedder::load_embedder_with_user_platform(&size_dir, user_platform)?;
    let dim = model.embed_dim();

    let dir = lancedb_dir(app)?;
    std::fs::create_dir_all(&dir).map_err(|e| anyhow!("create lancedb dir {}: {}", dir.display(), e))?;
    let uri = dir
        .to_str()
        .ok_or_else(|| anyhow!("lancedb path not UTF-8: {}", dir.display()))?;
    let conn = lancedb::connect(uri)
        .execute()
        .await
        .map_err(|e| anyhow!("lancedb connect {}: {}", dir.display(), e))?;

    {
        let mut guard = runtime_slot().lock().await;
        // Drop the old model first (frees GBs) then swap.
        if let Some(old) = guard.take() {
            drop(old);
        }
        *guard = Some(MvRuntime {
            model,
            conn,
            active_model: size.clone(),
            embed_dim: dim,
        });
    }
    EMBED_DIM.store(dim, Ordering::SeqCst);
    let rss = embedder::process_rss_mib().unwrap_or(0);
    mv_log(
        "info",
        format!("[mv] model '{}' loaded (dim={}, RSS: {} MiB)", size, dim, rss),
    );
    // Model actually (re)loaded — the smart index may hold rows embedded by a
    // previous model (same-dim swaps don't recreate the table). Purge + reindex
    // in the background; no-op when Smart Routing is off.
    {
        let size = size.clone();
        tokio::spawn(async move {
            crate::smart_routing::index::on_model_reloaded(size).await;
        });
    }
    Ok(MvInfo {
        active_model: size,
        embed_dim: dim,
    })
}

/// Release one consumer. When no consumers remain, the model + connection are
/// dropped and memory is returned to the OS (mimalloc collect — the old
/// rag::stop semantics). Idempotent per consumer.
pub async fn release(consumer: &str) {
    let remaining = match consumers().lock() {
        Ok(mut c) => {
            c.remove(consumer);
            c.len()
        }
        // Poisoned lock: the consumer set is unreadable — treat the remaining
        // count as UNKNOWN, not zero. Tearing down the shared runtime here
        // would kill a model other consumers may still be using (fail-closed
        // parity with ensure_started, review round 8, 2026-10-04).
        Err(_) => {
            mv_log("warn", format!("[mv] release({}) — consumers lock poisoned; leaving runtime state untouched", consumer));
            return;
        }
    };
    if remaining > 0 {
        mv_log(
            "info",
            format!("[mv] release({}) — {} consumer(s) remain, runtime kept", consumer, remaining),
        );
        return;
    }
    let rss_before = embedder::process_rss_mib().unwrap_or(0);
    let had = {
        let mut guard = runtime_slot().lock().await;
        guard.take().is_some()
    };
    EMBED_DIM.store(0, Ordering::SeqCst);
    if had {
        mv_log("info", format!("[mv] all consumers gone — runtime released (RSS {} -> pending mimalloc)", rss_before));
    }
    // Same rationale as the old rag::stop: force mimalloc to return pages.
    unsafe { libmimalloc_sys::mi_collect(true) };
    let rss_after = embedder::process_rss_mib().unwrap_or(0);
    mv_log("info", format!("[mv] runtime stopped (RSS: {} -> {} MiB)", rss_before, rss_after));
}

/// The model the shared runtime currently holds (the one all embeds go
/// through). None when not running. Used by reindex completion to persist
/// `rag.indexedModel` with the model that ACTUALLY produced the embeddings.
/// Wait (bounded) for any in-flight model load to settle. Callers that
/// release a consumer right after a just-triggered ensure_started MUST wait
/// first: release() takes the runtime slot only if it is populated — if the
/// load is still running, the slot is filled AFTER release returns and the
/// model would spin up with zero consumers (leaked, never released).
pub async fn wait_while_initializing(timeout: std::time::Duration) {
    let start = std::time::Instant::now();
    while INITIALIZING.load(Ordering::SeqCst) {
        if start.elapsed() > timeout {
            log::warn!("[mv] wait_while_initializing timed out after {:?} — proceeding", timeout);
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

pub async fn active_model() -> Option<String> {
    let guard = runtime_slot().lock().await;
    guard.as_ref().map(|rt| rt.active_model.clone())
}

/// The device preference (`mv.device`, default "auto"). Pure config read —
/// the backend consumes it at model load (Phase 4 wires the actual engine
/// parameter through).
pub async fn device() -> String {
    crate::services::config_service::get()
        .await
        .ok()
        .and_then(|c| {
            c.get("mv")
                .and_then(|m| m.get("device"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "auto".to_string())
}

/// The persisted `smartRouting.enabled` intent (Smart Routing consumer's
/// enable flag). Phase 2 uses it only for the boot auto-restore of the mv
/// runtime (model preload when SR is on but RAG is off); Phase 3 wires SR's
/// own indexing/search startup to it.
pub async fn smart_routing_config_enabled() -> bool {
    crate::services::config_service::get()
        .await
        .ok()
        .and_then(|c| {
            c.get("smartRouting")
                .and_then(|r| r.get("enabled"))
                .and_then(|v| v.as_bool())
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    /// Reference counting + idempotency, in one serialized test (the consumer
    /// set is a process-wide global, so parallel tests would race on it).
    #[tokio::test]
    async fn release_refcount_and_idempotency() {
        // Start from a clean consumer set (other tests must not have left
        // entries behind; consumers() is the same global the real flow uses).
        {
            let mut c = consumers().lock().unwrap();
            c.clear();
            c.insert("rag".into());
            c.insert("smart".into());
        }
        assert_eq!(consumer_count(), 2);
        release("rag").await;
        assert_eq!(consumer_count(), 1, "runtime must stay wanted for 'smart'");
        // Releasing an unregistered consumer is a no-op (idempotent per
        // consumer, never takes the runtime down on its own).
        release("ghost").await;
        release("ghost").await;
        assert_eq!(consumer_count(), 1, "unknown release must not touch real consumers");
        release("smart").await;
        assert_eq!(consumer_count(), 0);
        // Nothing was loaded in the test env → slot still None, dim still 0.
        assert!(runtime_slot().lock().await.is_none());
        assert_eq!(EMBED_DIM.load(Ordering::SeqCst), 0);
    }

    /// Singleton invariant: without a loaded model every accessor errors and
    /// `needs_reload` reports true (nothing loaded → must load).
    #[tokio::test]
    async fn accessors_error_and_need_reload_when_nothing_loaded() {
        EMBED_DIM.store(0, Ordering::SeqCst);
        assert!(!is_running());
        assert_eq!(embed_dim(), None);
        assert!(with_model(|_| Ok(())).await.is_err(), "with_model must fail without a runtime");
        assert!(with_runtime(|_, _| Ok(())).await.is_err());
        assert!(connection_async().await.is_err());
        assert!(model_max_context_loaded().await.is_err());
        // needs_reload with an empty slot returns true WITHOUT consulting the
        // config store (short-circuit on None) — safe in the test env.
        assert!(needs_reload().await.unwrap());
    }
}
