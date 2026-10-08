//! Upstream list freshness tracking (origin #1277 mirror).
//!
//! Origin contract (`src/utils/listFreshness.ts` + #1277): a downstream
//! 2026 list projection may only advertise a positive `ttlMs` when every
//! participating upstream snapshot recorded freshness (upstream supplied a
//! positive TTL) and that freshness has not expired; the advertised value is
//! capped at 5s minus projection elapsed time. Anything unknown / expired /
//! empty → `ttlMs: 0`. Gateway-generated content (Smart Routing meta tools,
//! builtin prompts/resources) never participates → always 0.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Hard cap for downstream list TTLs, mirroring origin's 5s positive-list cap.
pub const MAX_LIST_TTL_MS: u64 = 5_000;

#[derive(Default)]
struct Entry {
    /// None = recorded without a positive upstream TTL (invalid → 0).
    expires_at: Option<Instant>,
}

static REGISTRY: std::sync::OnceLock<Mutex<HashMap<(String, &'static str), Entry>>> =
    std::sync::OnceLock::new();

fn registry() -> &'static Mutex<HashMap<(String, &'static str), Entry>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Record freshness for one upstream list snapshot. `ttl_ms` = the TTL the
/// upstream itself advertised (paged results / absent / 0 → no freshness).
pub fn record(server: &str, kind: &'static str, ttl_ms: Option<u64>) {
    let entry = Entry {
        expires_at: ttl_ms
            .filter(|t| *t > 0)
            .map(|t| Instant::now() + Duration::from_millis(t)),
    };
    if let Ok(mut map) = registry().lock() {
        map.insert((server.to_string(), kind), entry);
    }
}

/// Drop all recorded freshness for a server (disconnect / cache eviction).
pub fn invalidate_server(server: &str) {
    if let Ok(mut map) = registry().lock() {
        map.retain(|(s, _), _| s != server);
    }
}

/// Downstream TTL for a projection over `servers`: 0 when the participation
/// set is empty, any snapshot is missing/expired/unrecorded, or the 5s cap
/// minus elapsed time falls to zero. Otherwise the minimum remaining.
pub fn remaining(servers: &[String], kind: &str, elapsed_ms: u64) -> u64 {
    if servers.is_empty() {
        return 0;
    }
    let now = Instant::now();
    let mut best = MAX_LIST_TTL_MS.saturating_sub(elapsed_ms);
    let map = match registry().lock() {
        Ok(m) => m,
        Err(_) => return 0,
    };
    for s in servers {
        match map.get(&(s.clone(), kind_static(kind))) {
            Some(Entry {
                expires_at: Some(exp),
            }) if *exp > now => {
                let left = exp.saturating_duration_since(now).as_millis() as u64;
                best = best.min(left);
            }
            _ => return 0,
        }
    }
    best
}

// Kinds: keep the caller's static kind as-is so future prompts/resources
// recording never share keys across kinds (R118).
fn kind_static(kind: &str) -> &'static str {
    match kind {
        "tools" => "tools",
        "prompts" => "prompts",
        "resources" => "resources",
        _ => "other",
    }
}
