/// mcp_manager — loads server configs from DB and connects all enabled servers at startup.
use crate::{
    mcp::{pool, recovery},
    models::server::ServerType,
    services::{app_logger, config_service, server_service},
};
use anyhow::Result;
use tauri::AppHandle;

/// Called once at application startup — reads all enabled servers from DB and connects.
pub async fn start_all(app: &AppHandle) -> Result<()> {
    let _ = app; // AppHandle reserved for future event emitting
    let configs = server_service::list_all_enabled().await?;
    let msg = format!("Starting {} MCP servers...", configs.len());
    log::info!("{}", msg);
    app_logger::log_to_db("info", &msg);

    for (idx, cfg) in configs.into_iter().enumerate() {
        let name = cfg.name.clone();
        let delay = std::time::Duration::from_secs(idx as u64 * 2);
        tokio::spawn(async move {
            // Stagger startup to avoid file lock conflicts on shared cache directories
            if !delay.is_zero() {
                log::info!("[{}] Waiting {:.0}s before starting (staggered startup)...", name, delay.as_secs_f64());
                tokio::time::sleep(delay).await;
            }
            let status = pool::connect_server(&cfg).await;
            if status.connected {
                let msg = format!("Server '{}' connected ({} tools)", name, status.tool_count);
                log::info!("{}", msg);
                app_logger::log_to_db("info", &msg);
            } else if cfg.start_on_demand.unwrap_or(false) && status.error.is_none() {
                let msg = format!("Server '{}' sleeping (on-demand)", name);
                log::info!("{}", msg);
                app_logger::log_to_db("info", &msg);
            } else {
                let err = status.error.as_deref().unwrap_or("unknown");
                let msg = format!("Server '{}' failed to connect: {}", name, err);
                log::warn!("{}", msg);
                app_logger::log_to_db("warn", &msg);
            }
        });
    }

    // Background task: auto-reconnect disconnected servers when enableSessionRebuild is true
    tokio::spawn(async {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            let enabled = config_service::get().await.ok()
                .and_then(|c| c.get("enableSessionRebuild").and_then(|v| v.as_bool()))
                .unwrap_or(false);
            if !enabled {
                continue;
            }
            // Find enabled servers that are currently disconnected
            let all_statuses = pool::get_all_statuses().await;
            // Skip starting servers: their in-flight connect holds the
            // per-name lock; connect_server would block on it and then tear
            // down the JUST-established live connection to reconnect
            // (spurious churn on every rebuild tick during a slow npx
            // handshake — round 12 review).
            let disconnected: Vec<String> = all_statuses
                .into_iter()
                .filter(|s| !s.connected && !s.starting)
                .map(|s| s.name)
                .collect();
            if disconnected.is_empty() {
                continue;
            }
            let enabled_configs = match server_service::list_all_enabled().await {
                Ok(cfgs) => cfgs,
                Err(e) => {
                    log::warn!("[session_rebuild] Failed to list enabled servers: {}", e);
                    continue;
                }
            };
            for cfg in enabled_configs {
                // On-demand servers are intentionally sleeping; never wake them
                // here (would defeat the feature and could kill an in-flight
                // awake client via connect_server's cleanup).
                if cfg.start_on_demand.unwrap_or(false) {
                    continue;
                }
                // Remote (sse/streamable-http) servers recover via the
                // background backoff scheduler below (origin #1385) —
                // independent of enableSessionRebuild. Handling them here too
                // would double-schedule reconnects against the recovery
                // state machine.
                if matches!(cfg.server_type, ServerType::Sse | ServerType::StreamableHttp) {
                    continue;
                }
                if disconnected.contains(&cfg.name) {
                    let name = cfg.name.clone();
                    log::info!("[session_rebuild] Reconnecting server '{}'", name);
                    tokio::spawn(async move {
                        let status = pool::connect_server(&cfg).await;
                        if status.connected {
                            // Post-connect "still enabled?" recheck — the same
                            // fail-closed invariant as the toggle paths. The
                            // rebuild tick read the config BEFORE connecting;
                            // a disable/delete landing in that window leaves
                            // this spawn reconnecting a disabled server with
                            // nothing else ever disconnecting it.
                            let still_enabled = {
                                match server_service::get_by_name(&name).await {
                                    Ok(c) => c.map(|s| s.enabled).unwrap_or(false),
                                    Err(_) => false,
                                }
                            };
                            if !still_enabled {
                                log::info!(
                                    "[session_rebuild] '{}' disabled/deleted during rebuild — disconnecting",
                                    name
                                );
                                let _ = pool::disconnect_server(&name).await;
                                return;
                            }
                            log::info!("[session_rebuild] Server '{}' reconnected", name);
                        } else {
                            log::warn!("[session_rebuild] Server '{}' still failed: {}", name,
                                status.error.as_deref().unwrap_or("unknown"));
                        }
                    });
                }
            }
        }
    });

    // Origin #1385: disconnected REMOTE (sse/streamable-http) servers ALWAYS
    // auto-reconnect in the background with exponential backoff + equal
    // jitter — independent of enableSessionRebuild (which above keeps
    // governing stdio/openapi rebuilds only).
    //
    // Race discipline: this is a poll-based scheduler, so there are no
    // per-server timers to cancel. Every attempt re-reads the DB config and
    // re-checks pool state, so a server deleted/renamed/disabled mid-flight
    // renders the pending attempt harmless by identity guards:
    //   - deleted   → get_by_name None → recovery cleared, no connect;
    //   - disabled  → cfg.enabled false → recovery cleared, no connect;
    //   - renamed   → old name no longer resolves → cleared (the new name is
    //                 seeded fresh by the pool-status scan below);
    //   - connected by another path meanwhile → pool status check skips;
    //   - starting (another connect in flight) → skip; success clears
    //     recovery inside connect_server, failure is re-seeded next tick.
    // No lock is held across an await (recovery map ops are short critical
    // sections; connect_server manages its own per-name connect lock).
    tokio::spawn(async {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            // Seed/keep recovery entries: every pooled remote server that is
            // disconnected (and not starting / not on-demand) must have a
            // scheduled retry. Also reschedules attempts that died without
            // recording a failure, and sweeps entries for servers that left
            // the pool (deleted/disabled) unless an attempt is in flight.
            let remote_disconnected = pool::disconnected_remote_servers().await;
            for name in &remote_disconnected {
                recovery::ensure_scheduled(name).await;
                recovery::reschedule_stalled(name).await;
            }
            // Release attempt claims whose spawned task was lost (panic or
            // cancellation) — otherwise the entry stays `attempting` forever
            // and that server never auto-reconnects again.
            recovery::reclaim_stalled().await;
            recovery::sweep(&remote_disconnected).await;
            let due = recovery::take_due().await;
            for name in due {
                tokio::spawn(async move {
                    // Identity guard 1: config must still exist and be enabled.
                    let cfg = match server_service::get_by_name(&name).await {
                        Ok(Some(c)) => c,
                        // Deleted mid-flight, or DB read failed: fail-closed —
                        // never reconnect on uncertainty; drop recovery.
                        _ => {
                            recovery::clear(&name).await;
                            return;
                        }
                    };
                    if !cfg.enabled
                        || cfg.start_on_demand.unwrap_or(false)
                        || !matches!(cfg.server_type, ServerType::Sse | ServerType::StreamableHttp)
                    {
                        recovery::clear(&name).await;
                        return;
                    }
                    // Origin #1385 parity: an OAuth flow awaiting user
                    // authorization must not be retried in the background —
                    // every attempt fails with "authorization is pending" and
                    // the churn would mask the real state. Stop retrying
                    // (clear recovery); the connect flow re-seeds recovery on
                    // failure after the user completes or cancels.
                    if crate::services::oauth_upstream::status_of(&name).await == "pending" {
                        recovery::clear(&name).await;
                        return;
                    }
                    // Identity guard 2: another path already connected or is
                    // connecting — do not double-connect.
                    if let Some(st) = pool::get_status(&name).await {
                        if st.connected {
                            recovery::clear(&name).await;
                            return;
                        }
                        if st.starting {
                            // In-flight connect owned by another path; release
                            // our attempt claim — its outcome resolves recovery
                            // (success clears inside connect_server; failure is
                            // re-seeded next tick).
                            recovery::abort_attempt(&name).await;
                            return;
                        }
                    }
                    log::info!("[recovery] Reconnecting remote server '{}' (attempt {})", name,
                        recovery::get(&name).await.map(|r| r.attempts).unwrap_or(0));
                    let status = pool::connect_server(&cfg).await;
                    if status.connected {
                        // Post-connect "still enabled?" recheck — the same
                        // fail-closed invariant as the toggle/reload/rebuild
                        // paths: a disable/delete landing during the connect
                        // window must not leave a live process serving a
                        // disabled server.
                        let still_enabled = {
                            match server_service::get_by_name(&name).await {
                                Ok(c) => c.map(|s| s.enabled).unwrap_or(false),
                                Err(_) => false,
                            }
                        };
                        if !still_enabled {
                            log::info!(
                                "[recovery] '{}' disabled/deleted during reconnect — disconnecting",
                                name
                            );
                            let _ = pool::disconnect_server(&name).await;
                            recovery::clear(&name).await;
                            return;
                        }
                        recovery::clear(&name).await;
                        log::info!("[recovery] Server '{}' reconnected", name);
                    } else {
                        let err = status.error.as_deref().unwrap_or("unknown");
                        log::warn!("[recovery] '{}' reconnect failed: {}", name, err);
                        // Failed attempt: update error (connect_server already
                        // persisted it on the pool entry), increment attempts,
                        // reschedule with backoff + jitter.
                        recovery::record_failure(&name).await;
                    }
                });
            }
        }
    });

    Ok(())
}

/// Reload (disconnect + reconnect) a single server
pub async fn reload_server(server_name: &str) -> Result<()> {
    log::info!("[{}] Reloading server...", server_name);
    app_logger::log_to_db("info", &format!("[{}] Reloading server...", server_name));

    // Disconnect if connected
    pool::disconnect_server(server_name).await.ok();

    // Re-fetch config and reconnect in the background so the caller returns
    // immediately; the npx/uvx download / handshake may take a while.
    if let Some(cfg) = server_service::get_by_name(server_name).await? {
        if cfg.enabled {
            tokio::spawn(async move {
                pool::connect_server(&cfg).await;
                // Post-connect enabled recheck (toggle/rebuild family): a
                // disable landing during the connect window must not leave a
                // live process serving a disabled server (fail-closed on
                // read error, same as toggle's settle recheck).
                match server_service::get_by_name(&cfg.name).await {
                    Ok(Some(c)) if c.enabled => {}
                    _ => {
                        log::warn!("[{}] disabled/removed during reload connect — disconnecting", cfg.name);
                        let _ = pool::disconnect_server(&cfg.name).await;
                    }
                }
            });
        }
    }
    Ok(())
}

/// Toggle enabled/disabled for a server
pub async fn toggle_server(server_name: &str) -> Result<bool> {
    let cfg = server_service::toggle_enabled(server_name).await?;
    let action = if cfg.enabled { "Enabling" } else { "Disabling" };
    log::info!("[{}] {} server...", server_name, action);
    app_logger::log_to_db("info", &format!("[{}] {} server...", server_name, action));

    if cfg.enabled {
        // Always disconnect first to clean up any zombie processes before re-connecting
        // disconnect_server is fully synchronous — waits for process exit + file lock release
        if let Err(e) = pool::disconnect_server(server_name).await {
            log::warn!("[{}] Pre-enable disconnect failed (may be already disconnected): {}", server_name, e);
        }
        // Connect in the background so the toggle returns immediately.
        // Re-check `enabled` after connecting: a rapid enable→disable while
        // the connect was in flight would otherwise leave the server
        // permanently connected and serving over HTTP despite being disabled
        // (session_rebuild only reconnects enabled servers, and the HTTP
        // scope filter does not consult the DB `enabled` flag).
        let cfg_clone = cfg.clone();
        let name_for_recheck = server_name.to_string();
        tokio::spawn(async move {
            pool::connect_server(&cfg_clone).await;
            let still_enabled = {
                match crate::services::server_service::get_by_name(&name_for_recheck).await {
                    // Row gone (deleted mid-connect) or DB error → NOT enabled:
                    // fail-open here orphans a live process with no DB row —
                    // nothing reaps it until restart. Mirrors the disable-path
                    // recheck below.
                    Ok(c) => c.map(|s| s.enabled).unwrap_or(false),
                    Err(_) => false,
                }
            };
            if !still_enabled {
                log::info!("[{}] disabled during connect — disconnecting after connect finished", name_for_recheck);
                let _ = pool::disconnect_server(&name_for_recheck).await;
            }
        });
    } else {
        // Check if server is still in "starting" state — if so, let the connect finish
        // rather than killing the just-spawned process (race condition from rapid clicks)
        let is_starting = pool::get_status(server_name).await
            .map(|s| s.starting)
            .unwrap_or(false);
        if is_starting {
            log::info!("[{}] Server is still connecting, will not kill process — waiting for connect to finish", server_name);
            app_logger::log_to_db("info", &format!("[{}] Server still connecting, skipping disconnect to avoid race condition", server_name));
            // Post-connect re-check (mirrors the enable branch): without this
            // the in-flight connect completes and the server ends up
            // connected+serving despite enabled=false — session_rebuild only
            // reconnects enabled servers and the HTTP scope filter doesn't
            // consult the DB flag, so nothing else would ever disconnect it.
            let name_for_recheck = server_name.to_string();
            tokio::spawn(async move {
                // Wait for the starting phase to settle (poll, cheap).
                for _ in 0..600 {
                    let still_starting = pool::get_status(&name_for_recheck)
                        .await
                        .map(|s| s.starting)
                        .unwrap_or(false);
                    if !still_starting {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                let still_enabled = {
                    match crate::services::server_service::get_by_name(&name_for_recheck).await {
                        Ok(c) => c.map(|s| s.enabled).unwrap_or(false),
                        // Fail-closed parity with the enable branch and
                        // session_rebuild: a DB read error during the settle
                        // window must not leave a disabled server connected.
                        Err(_) => false,
                    }
                };
                if !still_enabled {
                    log::info!("[{}] disabled during starting — disconnecting after connect finished", name_for_recheck);
                    let _ = pool::disconnect_server(&name_for_recheck).await;
                }
            });
        } else {
            if let Err(e) = pool::disconnect_server(server_name).await {
                log::error!("[{}] Failed to disconnect: {}", server_name, e);
                app_logger::log_to_db("error", &format!("[{}] Failed to disconnect: {}", server_name, e));
            }
        }
        // Disabled servers contribute no Smart Routing rows — drop them
        // (best-effort; logged inside).
        let n = server_name.to_string();
        tokio::spawn(async move {
            if let Err(e) = crate::smart_routing::index::remove_server_embeddings(&n).await {
                log::warn!("[smart] remove embeddings for disabled '{}': {}", n, e);
            }
        });
    }
    Ok(cfg.enabled)
}
