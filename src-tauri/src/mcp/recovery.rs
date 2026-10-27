//! Per-server reconnect recovery state for remote (sse/streamable-http) MCP
//! servers. Mirrors origin #1385: a disconnected remote server ALWAYS
//! auto-reconnects in the background with exponential backoff + equal jitter,
//! independent of the keep-alive toggle (`enableKeepAlive` only governs
//! periodic health checks of connected servers).
//!
//! State is in-memory only (runtime state, no DB migration) and keyed by
//! server name. The scheduler lives in `services/mcp_manager.rs`; this module
//! owns the state map and the backoff computation.

use crate::models::server::RecoveryInfo;
use std::collections::HashMap;
use std::sync::OnceLock;
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub(crate) struct RecoveryState {
    pub attempts: u32,
    /// Epoch milliseconds of the last reconnect attempt (None = never tried).
    pub last_attempt_ms: Option<u64>,
    /// Epoch milliseconds of the next scheduled attempt (None = attempt in
    /// flight or awaiting reschedule).
    pub next_attempt_ms: Option<u64>,
    /// True while a spawned reconnect attempt for this server is in flight.
    /// Guards against double-scheduling; `reclaim_stalled` re-arms a claim
    /// whose task died without recording a failure.
    pub attempting: bool,
    /// Epoch ms when `take_due` claimed this entry (None = not claimed).
    /// A claim older than STALLED_CLAIM_LIMIT_MS is reclaimed — the spawned
    /// task was lost (panic/cancel) and would otherwise wedge the entry in
    /// `attempting` forever.
    pub claimed_at_ms: Option<u64>,
}

/// Generous bound for reclaiming a lost attempt claim. Worst-case live
/// attempt inside `connect_server`: up to 3 retries × (connect 120s +
/// tools/list 60s) + retry sleeps ≈ 542s — the limit must exceed that so a
/// slow-but-live attempt is never reclaimed (a reclaimed live attempt would
/// double-connect and double-count failures).
const STALLED_CLAIM_LIMIT_MS: u64 = 600_000;

static RECOVERY: OnceLock<RwLock<HashMap<String, RecoveryState>>> = OnceLock::new();

fn map() -> &'static RwLock<HashMap<String, RecoveryState>> {
    RECOVERY.get_or_init(|| RwLock::new(HashMap::new()))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn rfc3339(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339()
}

/// Exponential backoff ceiling in milliseconds (origin #1385):
/// `min(300000, 10000 * 2^min(attempts, 5))` — bounded between 10s and 5min.
pub fn backoff_ceiling_ms(attempts: u32) -> u64 {
    let exp = attempts.min(5);
    10_000u64.saturating_mul(1u64 << exp).min(300_000)
}

/// Equal jitter (origin #1385): `delay = ceiling * (0.5 + rand * 0.5)`.
/// `frac` is the random fraction in `[0, 1)`; clamped defensively so a
/// malformed input can never push the delay past the ceiling.
pub fn jittered_delay_ms(ceiling_ms: u64, frac: f64) -> u64 {
    let frac = if frac.is_finite() { frac.clamp(0.0, 0.999_999) } else { 0.0 };
    ((ceiling_ms as f64) * (0.5 + frac * 0.5)).round() as u64
}

/// Current recovery info for a server (None when not recovering).
pub async fn get(name: &str) -> Option<RecoveryInfo> {
    let m = map().read().await;
    m.get(name).map(|s| RecoveryInfo {
        attempts: s.attempts,
        last_attempt_at: s.last_attempt_ms.map(rfc3339),
        next_attempt_at: s.next_attempt_ms.map(rfc3339),
    })
}

/// Ensure a recovery entry exists for `name` (attempts=0, first retry
/// scheduled with the attempts=0 backoff). No-op when one already exists.
pub async fn ensure_scheduled(name: &str) {
    let mut m = map().write().await;
    let entry = m.entry(name.to_string()).or_insert_with(|| RecoveryState {
        attempts: 0,
        last_attempt_ms: None,
        next_attempt_ms: None,
        attempting: false,
        claimed_at_ms: None,
    });
    if !entry.attempting && entry.next_attempt_ms.is_none() {
        let ceiling = backoff_ceiling_ms(entry.attempts);
        let delay = jittered_delay_ms(ceiling, rand_frac());
        entry.next_attempt_ms = Some(now_ms() + delay);
    }
}

/// Reschedule an entry that is neither attempting nor scheduled (e.g. the
/// seeded entry's delay elapsed while the server left and rejoined the pool).
/// A claim LOST mid-attempt (panic/cancel between `take_due` and
/// `record_failure`) is handled by `reclaim_stalled` instead.
pub async fn reschedule_stalled(name: &str) {
    let mut m = map().write().await;
    if let Some(entry) = m.get_mut(name) {
        if !entry.attempting && entry.next_attempt_ms.is_none() {
            let ceiling = backoff_ceiling_ms(entry.attempts);
            let delay = jittered_delay_ms(ceiling, rand_frac());
            entry.next_attempt_ms = Some(now_ms() + delay);
        }
    }
}

/// Refresh the claim timestamp of an in-flight attempt. Called by
/// `connect_server` AFTER it acquires the per-name connect lock: the claim is
/// stamped at `take_due`, but the attempt may then queue behind a slow
/// connect (toggle/reload, worst ~542s) — without this refresh,
/// `reclaim_stalled`'s 600s budget could expire while the attempt is still
/// legitimately waiting, releasing a duplicate attempt that would tear down
/// the freshly built connection.
pub async fn touch_claim(name: &str) {
    let mut m = map().write().await;
    if let Some(entry) = m.get_mut(name) {
        if entry.attempting {
            entry.claimed_at_ms = Some(now_ms());
        }
    }
}

/// Take all servers whose next attempt is due: clears `next_attempt_at` and
/// marks the entry attempting so concurrent ticks cannot double-fire.
pub async fn take_due() -> Vec<String> {
    let now = now_ms();
    let mut m = map().write().await;
    let mut due = Vec::new();
    for (name, entry) in m.iter_mut() {
        if !entry.attempting {
            if let Some(next) = entry.next_attempt_ms {
                if next <= now {
                    entry.next_attempt_ms = None;
                    entry.attempting = true;
                    entry.claimed_at_ms = Some(now);
                    due.push(name.clone());
                }
            }
        }
    }
    due
}

/// Record a failed reconnect attempt: increments the attempt counter, stamps
/// `lastAttemptAt`, and schedules the next retry with backoff + jitter.
pub async fn record_failure(name: &str) {
    let mut m = map().write().await;
    let entry = m.entry(name.to_string()).or_insert_with(|| RecoveryState {
        attempts: 0,
        last_attempt_ms: None,
        next_attempt_ms: None,
        attempting: false,
        claimed_at_ms: None,
    });
    entry.attempts = entry.attempts.saturating_add(1);
    entry.claimed_at_ms = None;
    entry.last_attempt_ms = Some(now_ms());
    let ceiling = backoff_ceiling_ms(entry.attempts);
    let delay = jittered_delay_ms(ceiling, rand_frac());
    entry.next_attempt_ms = Some(now_ms() + delay);
    entry.attempting = false;
}

/// Clear recovery state (successful connect, server disabled/deleted, or the
/// server is not a backoff-recovering remote type).
pub async fn clear(name: &str) {
    map().write().await.remove(name);
}

/// Clear ALL recovery state (application shutdown).
pub async fn clear_all() {
    map().write().await.clear();
}

/// Release an in-flight attempt claim without recording a failure — used when
/// the attempt yielded to another path's connect (pool entry `starting`). The
/// seeder reschedules once the entry settles; a successful connect clears the
/// entry inside `connect_server`.
pub async fn abort_attempt(name: &str) {
    let mut m = map().write().await;
    if let Some(entry) = m.get_mut(name) {
        entry.attempting = false;
        entry.claimed_at_ms = None;
    }
}

/// Re-arm attempt claims that never settled: a spawned task lost to panic or
/// cancellation between `take_due` and `record_failure`/`abort_attempt`
/// leaves the entry stuck in `attempting` (take_due skips it, sweep retains
/// it) — that server would never auto-reconnect again. Claims older than
/// STALLED_CLAIM_LIMIT_MS are released and rescheduled with backoff.
pub async fn reclaim_stalled() {
    let now = now_ms();
    let mut m = map().write().await;
    for entry in m.values_mut() {
        if !entry.attempting {
            continue;
        }
        let stalled = match entry.claimed_at_ms {
            Some(claimed) => now.saturating_sub(claimed) > STALLED_CLAIM_LIMIT_MS,
            // attempting without a claim stamp cannot be live — release it.
            None => true,
        };
        if stalled {
            entry.attempting = false;
            entry.claimed_at_ms = None;
            let ceiling = backoff_ceiling_ms(entry.attempts);
            let delay = jittered_delay_ms(ceiling, rand_frac());
            entry.next_attempt_ms = Some(now + delay);
        }
    }
}

/// Drop recovery entries for servers no longer present in the pool, unless an
/// attempt for them is still in flight (the attempt task will clear or record
/// a failure; a failure recorded for a deleted server is swept next tick).
pub async fn sweep(present: &[String]) {
    let mut m = map().write().await;
    m.retain(|name, entry| entry.attempting || present.iter().any(|p| p == name));
}

fn rand_frac() -> f64 {
    // Cheap pseudo-random fraction in [0, 1) for jitter — a full RNG crate is
    // unnecessary here. Nanosecond sub-second fraction mixed with a stack
    // address gives enough entropy to spread retries across the window.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as f64 / 1_000_000_000.0)
        .unwrap_or(0.5);
    let salt = &nanos as *const f64 as usize;
    let mixed = ((nanos * 7919.0) + (salt % 1009) as f64) % 1.0;
    mixed.abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_ceiling_doubles_and_caps() {
        assert_eq!(backoff_ceiling_ms(0), 10_000);
        assert_eq!(backoff_ceiling_ms(1), 20_000);
        assert_eq!(backoff_ceiling_ms(2), 40_000);
        assert_eq!(backoff_ceiling_ms(3), 80_000);
        assert_eq!(backoff_ceiling_ms(4), 160_000);
        assert_eq!(backoff_ceiling_ms(5), 300_000); // 320_000 capped at 5min
        assert_eq!(backoff_ceiling_ms(6), 300_000);
        assert_eq!(backoff_ceiling_ms(50), 300_000);
        assert_eq!(backoff_ceiling_ms(u32::MAX), 300_000);
    }

    #[test]
    fn jitter_stays_within_half_to_full_ceiling() {
        assert_eq!(jittered_delay_ms(10_000, 0.0), 5_000);
        assert_eq!(jittered_delay_ms(10_000, 1.0), 10_000); // clamped below 1
        assert_eq!(jittered_delay_ms(10_000, 0.5), 7_500);
        assert_eq!(jittered_delay_ms(300_000, 0.999_999), 300_000);
        // Negative / NaN / infinity fall back to the lower bound.
        assert_eq!(jittered_delay_ms(10_000, -3.0), 5_000);
        assert_eq!(jittered_delay_ms(10_000, f64::NAN), 5_000);
        assert_eq!(jittered_delay_ms(10_000, f64::INFINITY), 5_000);
        for frac in [0.0f64, 0.1, 0.25, 0.5, 0.75, 0.9] {
            let d = jittered_delay_ms(160_000, frac);
            assert!((80_000..=160_000).contains(&d));
        }
    }
}
