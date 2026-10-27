//! Last-known tool snapshots for on-demand servers (#1384 parity).
//!
//! On-demand (startOnDemand) stdio servers keep no live connection between
//! calls; after an app restart their pool placeholder would advertise zero
//! tools until the first cold start. This service persists the tool list
//! captured at the last wake so the sleeping placeholder can restore it.
//!
//! Scope mirrors origin: only on-demand servers use snapshots. Writes are a
//! single upsert on wake; the row is deleted with the server and rewritten on
//! rename (same transaction as the other name-keyed cascades).

use crate::db;
use anyhow::Result;
use crate::models::server::{ServerConfig, Tool};

/// Stable hash of the launch-affecting config (origin e4c46bf fingerprints
/// the launch config so a stale snapshot is never restored after the
/// server's command/args/env/endpoint were edited). Name/description and
/// per-tool state are excluded — they don't affect what the upstream serves.
///
/// Origin scrypt-stretches the digest "because it may contain low-entropy
/// secrets" — parity here: `v1:<salt-hex>:<scrypt-hex>` with a per-save
/// random salt (N=2^11, r=8, p=1, 32 bytes). Verification parses the salt
/// from the stored fingerprint and recomputes; format mismatch (legacy/
/// corrupt) never matches.
pub fn launch_fingerprint(cfg: &ServerConfig) -> String {
    let bytes = launch_config_bytes(cfg);
    let mut salt = [0u8; 16];
    // Cheap entropy source is fine: the salt only needs uniqueness per row,
    // not unpredictability (it is stored alongside the digest).
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    salt[..8].copy_from_slice(&nanos.to_le_bytes());
    salt[8..].copy_from_slice(&std::process::id().to_le_bytes().repeat(2));
    let digest = scrypt_digest(&bytes, &salt);
    format!("v1:{}:{}", hex_encode(&salt), hex_encode(&digest))
}

/// Verify `fingerprint` against the config: parses the salt out of the
/// stored `v1:salt:hash` form and recomputes. Returns false for any
/// malformed/legacy fingerprint.
pub fn verify_fingerprint(cfg: &ServerConfig, fingerprint: &str) -> bool {
    let parts: Vec<&str> = fingerprint.split(':').collect();
    if parts.len() != 3 || parts[0] != "v1" {
        return false;
    }
    let Ok(salt) = hex_decode(parts[1]) else {
        return false;
    };
    let Ok(expected) = hex_decode(parts[2]) else {
        return false;
    };
    let digest = scrypt_digest(&launch_config_bytes(cfg), &salt);
    // Constant-time-ish compare (digest lengths are fixed at 32).
    digest.len() == expected.len()
        && digest
            .iter()
            .zip(expected.iter())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

fn launch_config_bytes(cfg: &ServerConfig) -> Vec<u8> {
    // env/headers are HashMaps whose iteration order is randomized per
    // process — sort them into BTreeMaps so the serialized bytes are stable
    // across restarts.
    let env = cfg
        .env
        .as_ref()
        .map(|m| m.iter().collect::<std::collections::BTreeMap<_, _>>());
    let headers = cfg
        .headers
        .as_ref()
        .map(|m| m.iter().collect::<std::collections::BTreeMap<_, _>>());
    let launch = serde_json::json!({
        "serverType": cfg.server_type,
        "command": cfg.command,
        "args": cfg.args,
        "env": env,
        "url": cfg.url,
        "headers": headers,
        "options": cfg.options,
        "oauth": cfg.oauth,
        "openapi": cfg.openapi,
    });
    serde_json::to_vec(&launch).unwrap_or_default()
}

fn scrypt_digest(input: &[u8], salt: &[u8]) -> [u8; 32] {
    let params = scrypt::Params::new(11, 8, 1, 32).expect("static scrypt params are valid");
    let mut out = [0u8; 32];
    scrypt::scrypt(input, salt, &params, &mut out).expect("scrypt with valid params cannot fail");
    out
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    if s.len() % 2 != 0 {
        return Err(());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| ()))
        .collect()
}

/// Persist (or replace) the last-known tool list for an on-demand server.
/// Best-effort: a snapshot failure must never break a wake.
pub async fn save_snapshot(server_name: &str, fingerprint: &str, tools: &[Tool]) -> Result<()> {
    let json = serde_json::to_string(tools)?;
    let now = chrono::Utc::now().to_rfc3339();
    // Conditional upsert: only when the server row still exists. A delete
    // committing mid-wake must not be resurrected by the in-flight save.
    sqlx::query(
        "INSERT INTO capability_snapshots (server_name, tools_json, fingerprint, updated_at) \
         SELECT ?, ?, ?, ? WHERE EXISTS (SELECT 1 FROM servers WHERE name = ?) \
         ON CONFLICT(server_name) DO UPDATE SET tools_json = excluded.tools_json, \
         fingerprint = excluded.fingerprint, updated_at = excluded.updated_at",
    )
    .bind(server_name)
    .bind(&json)
    .bind(fingerprint)
    .bind(&now)
    .bind(server_name)
    .execute(db::pool())
    .await?;
    Ok(())
}

/// Load the persisted tool list for a server, if any. Corrupt JSON is treated
/// as absent (warn + None) rather than failing the placeholder insert.
pub async fn load_snapshot(cfg: &ServerConfig) -> Option<Vec<Tool>> {
    let server_name = cfg.name.as_str();
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT tools_json, fingerprint FROM capability_snapshots WHERE server_name = ?")
            .bind(server_name)
            .fetch_optional(db::pool())
            .await
            .ok()
            .flatten();
    let (json, stored_fp) = row?;
    // A snapshot captured under a different launch config must not be
    // restored (origin parity): the tools belong to the old command/args.
    // Salted scrypt: verify by recomputing with the stored row's salt.
    if !verify_fingerprint(cfg, &stored_fp) {
        return None;
    }
    match serde_json::from_str::<Vec<Tool>>(&json) {
        Ok(tools) if !tools.is_empty() => Some(tools),
        Ok(_) => None,
        Err(e) => {
            log::warn!(
                "[capability-snapshot] corrupt snapshot for '{}': {}",
                server_name,
                e
            );
            None
        }
    }
}

/// Delete the snapshot row (server delete cascade). Must run inside the
/// caller's transaction.
pub async fn delete_snapshot_tx(tx: &mut sqlx::SqliteConnection, server_name: &str) -> Result<()> {
    sqlx::query("DELETE FROM capability_snapshots WHERE server_name = ?")
        .bind(server_name)
        .execute(tx)
        .await?;
    Ok(())
}

/// Rewrite the snapshot key after a server rename (same transaction as the
/// other name-keyed cascades).
pub async fn rename_snapshot_tx(
    tx: &mut sqlx::SqliteConnection,
    old_name: &str,
    new_name: &str,
) -> Result<()> {
    sqlx::query("UPDATE capability_snapshots SET server_name = ? WHERE server_name = ?")
        .bind(new_name)
        .bind(old_name)
        .execute(tx)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str) -> Tool {
        serde_json::from_value(serde_json::json!({
            "name": name,
            "description": format!("{} desc", name),
            "inputSchema": {"type": "object"},
            "serverName": "od-srv"
        }))
        .expect("tool from json")
    }

    fn test_cfg(name: &str) -> ServerConfig {
        serde_json::from_value(serde_json::json!({
            "name": name, "serverType": "stdio", "command": "node",
        }))
        .expect("cfg")
    }

    #[tokio::test]
    async fn snapshot_round_trip() {
        crate::db::ensure_test_pool().await;
        sqlx::query("DELETE FROM capability_snapshots WHERE server_name IN ('od-srv','od-srv2')")
            .execute(crate::db::pool())
            .await
            .unwrap();

        assert!(load_snapshot(&test_cfg("od-srv")).await.is_none());

        // The conditional upsert only fires when the server row exists —
        // seed one (and clean it up) so the save path is exercised.
        sqlx::query("INSERT INTO servers (id, name) VALUES ('od-srv-id', 'od-srv')")
            .execute(crate::db::pool())
            .await
            .unwrap();

        save_snapshot("od-srv", &launch_fingerprint(&test_cfg("od-srv")), &[tool("alpha"), tool("beta")])
            .await
            .expect("save");
        let loaded = load_snapshot(&test_cfg("od-srv")).await.expect("loaded");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].name, "alpha");
        assert_eq!(loaded[1].name, "beta");

        // A fingerprint mismatch (config edited since the snapshot) restores
        // nothing — the tools belong to the old launch config.
        let mut edited = test_cfg("od-srv");
        edited.args = Some(vec!["--changed".into()]);
        assert!(load_snapshot(&edited).await.is_none());

        // Upsert replaces the previous snapshot.
        save_snapshot("od-srv", &launch_fingerprint(&test_cfg("od-srv")), &[tool("gamma")]).await.expect("resave");
        let loaded = load_snapshot(&test_cfg("od-srv")).await.expect("loaded");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "gamma");

        // Rename cascade rewrites the key.
        let mut tx = crate::db::pool().begin().await.unwrap();
        rename_snapshot_tx(&mut tx, "od-srv", "od-srv2").await.unwrap();
        tx.commit().await.unwrap();
        assert!(load_snapshot(&test_cfg("od-srv")).await.is_none());
        assert_eq!(load_snapshot(&test_cfg("od-srv2")).await.unwrap().len(), 1);

        // Delete cascade removes the row.
        let mut tx = crate::db::pool().begin().await.unwrap();
        delete_snapshot_tx(&mut tx, "od-srv2").await.unwrap();
        tx.commit().await.unwrap();
        assert!(load_snapshot(&test_cfg("od-srv2")).await.is_none());

        sqlx::query("DELETE FROM servers WHERE id = 'od-srv-id'")
            .execute(crate::db::pool())
            .await
            .unwrap();
    }

    #[test]
    fn fingerprint_verifies_and_is_config_sensitive() {
        let cfg: ServerConfig =
            serde_json::from_value(serde_json::json!({
                "name": "fp-srv", "serverType": "stdio", "command": "node",
            }))
            .expect("cfg");
        let fp = launch_fingerprint(&cfg);
        assert!(fp.starts_with("v1:"), "salted format");
        // Per-save random salt: two digests differ but both verify.
        assert_ne!(fp, launch_fingerprint(&cfg), "salt is per-save");
        assert!(verify_fingerprint(&cfg, &fp), "same config verifies");
        let mut changed = cfg.clone();
        changed.args = Some(vec!["--flag".into()]);
        assert!(!verify_fingerprint(&changed, &fp), "args change breaks verify");
        assert!(!verify_fingerprint(&cfg, "garbage"), "malformed never verifies");
        assert!(!verify_fingerprint(&cfg, "v1:zz:zz"), "bad hex never verifies");
    }
}
