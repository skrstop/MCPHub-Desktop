//! MCPB (`.mcpb` bundle) upload & extraction service.
//!
//! Mirrors origin `src/controllers/mcpbController.ts`: pick a `.mcpb` file via
//! the native dialog, extract the ZIP to a temp dir under
//! `app_data/uploads/mcpb/`, validate `manifest.json`, resolve the final
//! `server-{name}` directory (with path-escape re-check), clean up any
//! previous version of the same server, then rename into place and return
//! `{ manifest, extractDir }` so the frontend can build a stdio ServerConfig
//! (replacing `${__dirname}` with `extractDir`) and create/update the server
//! through the normal create_server/update_server commands.
//!
//! Desktop differences from origin (intentional):
//! - The file comes from a native picker instead of a multipart HTTP upload
//!   (the Tauri webview cannot stream FormData), so the user's original
//!   `.mcpb` file is never deleted — origin deletes its own upload-dir copy.
//! - Gate is "any authenticated user or guest" (see commands/mcpb.rs), not
//!   admin-only.
//! - Extra hardening beyond origin: cumulative uncompressed-size cap (zip
//!   bomb) and strict rejection of any `..` component inside archive entry
//!   names (matches the tar-escape guard in commands/runtime.rs).

use anyhow::{anyhow, bail, Result};
use serde::Serialize;
use serde_json::Value;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

/// Origin multer limit: 500MB.
const MAX_MCPB_FILE_BYTES: u64 = 500 * 1024 * 1024;
/// Desktop hardening (origin has no cap): cumulative uncompressed size.
/// Checked incrementally DURING extraction — before each entry is written —
/// so a lying header cannot bypass it.
const MAX_TOTAL_UNCOMPRESSED_BYTES: u64 = 1024 * 1024 * 1024;
/// Origin serverNameValidation.ts SERVER_NAME_MAX_LENGTH.
const SERVER_NAME_MAX_LENGTH: usize = 128;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpbUploadResult {
    pub manifest: Value,
    pub extract_dir: String,
}

/// `app_data_dir()/uploads/mcpb` — desktop counterpart of origin's
/// `process.cwd()/data/uploads/mcpb`.
fn upload_root_dir() -> Result<PathBuf> {
    crate::services::runtime_env::app_data_dir("uploads/mcpb")
        .ok_or_else(|| anyhow!("failed to resolve app data directory"))
}

/// Native file picker (blocking dialog must run off async workers —
/// same pattern as skill_service::pick_directory). `None` = user cancelled.
async fn pick_mcpb_file(app: &AppHandle) -> Result<Option<PathBuf>> {
    let app = app.clone();
    let picked = tokio::task::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("Select MCPB bundle")
            .add_filter("MCPB bundle", &["mcpb"])
            .blocking_pick_file()
    })
    .await
    .map_err(|e| anyhow!("file picker task failed: {e}"))?;
    Ok(match picked {
        Some(fp) => Some(fp.into_path().map_err(|e| anyhow!("{}", e))?),
        None => None,
    })
}

/// JS-truthiness check for a manifest field (origin: `if (!manifest.x)`).
fn require_manifest_field(manifest: &Value, key: &str) -> Result<()> {
    let truthy = match manifest.get(key) {
        None | Some(Value::Null) => false,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().map_or(false, |f| f != 0.0),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    };
    if !truthy {
        bail!("Invalid manifest: missing {key}");
    }
    Ok(())
}

/// Server types defined by the MCPB spec (anthropics/mcpb MANIFEST.md).
/// Anything else is rejected at upload time with an explicit message —
/// origin silently falls through to a stdio config that hangs at connect.
const SUPPORTED_MCPB_SERVER_TYPES: [&str; 4] = ["node", "python", "binary", "uv"];

fn non_empty_command_of(server: &serde_json::Map<String, Value>) -> Option<&str> {
    server
        .get("mcp_config")
        .and_then(|m| m.get("command"))
        .and_then(|c| c.as_str())
        .map(str::trim)
        .filter(|c| !c.is_empty())
}

/// Validate the `server` section beyond the four required top-level fields:
/// - `server.type` (when present) must be one of the four spec types;
/// - node/python/binary require `mcp_config.command` (non-empty string) —
///   origin's silent `|| 'node'` default starts a hanging node REPL instead;
/// - uv may omit `mcp_config` (host-managed execution) but then needs an
///   `entry_point` so the frontend can synthesize `uv run --directory ...`.
fn validate_server_section(manifest: &Value) -> Result<()> {
    let Some(server) = manifest.get("server").and_then(|s| s.as_object()) else {
        bail!("Invalid manifest: server configuration must be an object");
    };
    let has_command = non_empty_command_of(server).is_some();
    let has_entry_point = server
        .get("entry_point")
        .and_then(|e| e.as_str())
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .is_some();
    if let Some(t) = server.get("type") {
        let Some(ts) = t.as_str() else {
            bail!("Invalid manifest: server.type must be a string");
        };
        if !SUPPORTED_MCPB_SERVER_TYPES.contains(&ts) {
            bail!(
                "Unsupported server type '{ts}' in manifest (supported: node, python, binary, uv)"
            );
        }
        if ts == "uv" {
            // uv may omit mcp_config (host-managed execution) but then needs
            // an entry_point for the frontend to synthesize the command.
            if !has_command && !has_entry_point {
                bail!("Invalid manifest: uv server requires either mcp_config.command or entry_point");
            }
        } else if !has_command {
            bail!("Invalid manifest: server.mcp_config.command is required for server type '{ts}'");
        }
    } else {
        // No type declared — origin parity: accept, but a usable command is
        // still required (otherwise the silent 'node' default hangs).
        if !has_command {
            bail!("Invalid manifest: server.mcp_config.command is missing");
        }
    }
    Ok(())
}

/// Origin `validateMcpbServerName`: shared server-name charset rule
/// (`[A-Za-z0-9._-]`, ≤128, no `..`) with the MCPB-specific error wording.
/// The charset already excludes `$`, so `$smart`-reserved names are covered.
fn validate_mcpb_server_name(name: &Value) -> Result<String> {
    let Some(s) = name.as_str() else {
        bail!("Invalid manifest: name contains unsafe characters");
    };
    let normalized = s.trim();
    let ok = !normalized.is_empty()
        && normalized.len() <= SERVER_NAME_MAX_LENGTH
        && !normalized.contains("..")
        && normalized
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !ok {
        bail!("Invalid manifest: name contains unsafe characters");
    }
    Ok(normalized.to_string())
}

/// Origin `resolveMcpbServerExtractDir`: `server-{name}` under the upload
/// root, with a path-escape re-check (defense in depth — the charset rule
/// already makes escape impossible).
fn resolve_server_extract_dir(base: &Path, server_name: &str) -> Result<PathBuf> {
    let resolved_base = std::path::absolute(base)
        .map_err(|e| anyhow!("failed to resolve upload directory: {e}"))?;
    let target = resolved_base.join(format!("server-{server_name}"));
    if target.strip_prefix(&resolved_base).is_err() {
        bail!("Invalid manifest: name resolves outside upload directory");
    }
    Ok(target)
}

/// Origin `cleanupOldMcpbServer`: remove `server-{name}`* directories from
/// the upload root. Failures are warned and do NOT block installation.
fn cleanup_old_mcpb_server(upload_dir: &Path, server_name: &str) {
    let prefix = format!("server-{server_name}");
    let Ok(entries) = std::fs::read_dir(upload_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(&prefix) && entry.path().is_dir() {
            if let Err(e) = std::fs::remove_dir_all(entry.path()) {
                log::warn!("Failed to cleanup old MCPB server directory {:?}: {e}", entry.path());
            }
        }
    }
}

/// Extract the archive into `temp_dir` with per-entry guards. Returns the
/// manifest JSON text read from the archive root.
fn extract_mcpb_archive(mcpb_path: &Path, temp_dir: &Path) -> Result<String> {
    let file = std::fs::File::open(mcpb_path)
        .map_err(|e| anyhow!("failed to open MCPB file: {e}"))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| anyhow!("failed to read MCPB archive: {e}"))?;

    let mut total_written: u64 = 0;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| anyhow!("corrupt MCPB archive entry #{i}: {e}"))?;

        let rel = entry
            .enclosed_name()
            .ok_or_else(|| anyhow!("unsafe path in MCPB archive: {}", entry.name()))?;
        if rel.components().any(|c| matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_))) {
            bail!("unsafe path in MCPB archive: {}", entry.name());
        }

        let out_path = temp_dir.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)
                .map_err(|e| anyhow!("failed to create directory {:?}: {e}", out_path))?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow!("failed to create directory {:?}: {e}", parent))?;
        }
        let mut out = std::fs::File::create(&out_path)
            .map_err(|e| anyhow!("failed to create file {:?}: {e}", out_path))?;
        // Count ACTUAL copied bytes (the header size can lie).
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = entry.read(&mut buf).map_err(|e| anyhow!("failed to read archive entry {}: {e}", entry.name()))?;
            if n == 0 {
                break;
            }
            total_written += n as u64;
            if total_written > MAX_TOTAL_UNCOMPRESSED_BYTES {
                bail!("MCPB archive exceeds the maximum total uncompressed size");
            }
            use std::io::Write;
            out.write_all(&buf[..n])
                .map_err(|e| anyhow!("failed to write {:?}: {e}", out_path))?;
        }
    }

    let manifest_path = temp_dir.join("manifest.json");
    if !manifest_path.exists() {
        bail!("manifest.json not found in MCPB file");
    }
    std::fs::read_to_string(&manifest_path).map_err(|e| anyhow!("failed to read manifest.json: {e}"))
}

/// Full pipeline after a file has been picked. Sync — runs inside
/// spawn_blocking.
fn process_mcpb(mcpb_path: &Path) -> Result<McpbUploadResult> {
    let upload_root = upload_root_dir()?;
    process_mcpb_in(&upload_root, mcpb_path)
}

/// Pipeline with an injected upload root (unit-testable without touching
/// process-global env vars).
fn process_mcpb_in(upload_root: &Path, mcpb_path: &Path) -> Result<McpbUploadResult> {
    std::fs::create_dir_all(upload_root)
        .map_err(|e| anyhow!("failed to create upload directory: {e}"))?;

    // Size pre-check BEFORE any extraction work (origin multer limit).
    let meta = std::fs::metadata(mcpb_path)
        .map_err(|e| anyhow!("failed to read MCPB file metadata: {e}"))?;
    if meta.len() > MAX_MCPB_FILE_BYTES {
        bail!("MCPB file exceeds the 500MB size limit");
    }

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let temp_extract_dir = upload_root.join(format!("temp-extracted-{now_ms}"));
    // Nanosecond suffix guards against a same-millisecond collision.
    let temp_extract_dir = if temp_extract_dir.exists() {
        upload_root.join(format!("temp-extracted-{now_ms}-{}", std::process::id()))
    } else {
        temp_extract_dir
    };

    let result = (|| -> Result<McpbUploadResult> {
        std::fs::create_dir_all(&temp_extract_dir)
            .map_err(|e| anyhow!("failed to create temp extract directory: {e}"))?;

        let manifest_content = extract_mcpb_archive(mcpb_path, &temp_extract_dir)?;
        let manifest: Value = serde_json::from_str(&manifest_content)
            .map_err(|e| anyhow!("Invalid manifest JSON: {e}"))?;

        require_manifest_field(&manifest, "manifest_version")?;
        require_manifest_field(&manifest, "name")?;
        require_manifest_field(&manifest, "version")?;
        require_manifest_field(&manifest, "server")?;
        validate_server_section(&manifest)?;

        let safe_server_name = validate_mcpb_server_name(manifest.get("name").unwrap())?;
        let final_extract_dir = resolve_server_extract_dir(&upload_root, &safe_server_name)?;

        // Clean up any existing version of this server. cleanup swallows its
        // own errors, so re-check before the rename (Windows rename onto an
        // existing directory fails with EPERM).
        cleanup_old_mcpb_server(&upload_root, &safe_server_name);
        if final_extract_dir.exists() {
            std::fs::remove_dir_all(&final_extract_dir)
                .map_err(|e| anyhow!("failed to remove old MCPB server directory: {e}"))?;
        }

        std::fs::rename(&temp_extract_dir, &final_extract_dir)
            .map_err(|e| anyhow!("failed to move extracted server into place: {e}"))?;
        log::info!("MCPB server extracted to: {}", final_extract_dir.display());

        Ok(McpbUploadResult {
            manifest,
            extract_dir: final_extract_dir.to_string_lossy().into_owned(),
        })
    })();

    if result.is_err() {
        // Error cleanup (origin: remove temp dir; the uploaded file is the
        // user's own and is intentionally kept on desktop).
        if temp_extract_dir.exists() {
            let _ = std::fs::remove_dir_all(&temp_extract_dir);
        }
    }
    result
}

/// Pick + extract + validate in one command. `Ok(None)` = user cancelled the
/// file picker (frontend stays silent).
pub async fn upload_mcpb(app: &AppHandle) -> Result<Option<McpbUploadResult>> {
    let Some(path) = pick_mcpb_file(app).await? else {
        return Ok(None);
    };
    tokio::task::spawn_blocking(move || process_mcpb(&path))
        .await
        .map_err(|e| anyhow!("MCPB processing task failed: {e}"))?
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_base(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("mcpb-test-{}-{tag}-{nanos}", std::process::id()))
    }

    /// Build a .mcpb (ZIP) with the given (name, content) entries.
    fn build_mcpb(path: &Path, entries: &[(&str, &[u8])]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let file = std::fs::File::create(path).unwrap();
        let mut w = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = Default::default();
        for (name, content) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(content).unwrap();
        }
        w.finish().unwrap();
    }

    const GOOD_MANIFEST: &str = r#"{
        "manifest_version": "0.2",
        "name": "demo-server",
        "version": "1.2.3",
        "description": "demo",
        "server": { "mcp_config": { "command": "node", "args": ["${__dirname}/index.js"] } }
    }"#;

    #[test]
    fn happy_path_installs_and_returns_manifest() {
        let base = temp_base("happy");
        let root = base.join("uploads/mcpb");
        let mcpb = base.join("demo.mcpb");
        build_mcpb(
            &mcpb,
            &[
                ("manifest.json", GOOD_MANIFEST.as_bytes()),
                ("index.js", b"console.log('hi')"),
                ("lib/util.js", b"export {}"),
            ],
        );

        let result = process_mcpb_in(&root, &mcpb).expect("should install");
        assert_eq!(result.manifest["name"], "demo-server");
        let dir = PathBuf::from(&result.extract_dir);
        assert!(dir.join("manifest.json").exists());
        assert!(dir.join("index.js").exists());
        assert!(dir.join("lib/util.js").exists());
        assert!(dir.file_name().unwrap().to_string_lossy() == "server-demo-server");
        // User's original file is kept (desktop difference).
        assert!(mcpb.exists());
        // No temp dirs left behind.
        let leftovers: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("temp-extracted-"))
            .collect();
        assert!(leftovers.is_empty(), "leftover temp dirs: {leftovers:?}");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn reinstall_replaces_old_version() {
        let base = temp_base("reinstall");
        let root = base.join("uploads/mcpb");
        let mcpb = base.join("demo.mcpb");
        build_mcpb(
            &mcpb,
            &[("manifest.json", GOOD_MANIFEST.as_bytes()), ("index.js", b"v2")],
        );

        process_mcpb_in(&root, &mcpb).expect("first install");
        // Simulate a stale file from a previous version.
        let dir = root.join("server-demo-server");
        std::fs::write(dir.join("stale-old-version.js"), b"old").unwrap();

        process_mcpb_in(&root, &mcpb).expect("second install");
        assert!(dir.join("index.js").exists());
        assert!(!dir.join("stale-old-version.js").exists(), "old version file must be cleaned");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn missing_manifest_rejected() {
        let base = temp_base("nomanifest");
        let root = base.join("uploads/mcpb");
        let mcpb = base.join("demo.mcpb");
        build_mcpb(&mcpb, &[("index.js", b"x")]);

        let err = process_mcpb_in(&root, &mcpb).unwrap_err();
        assert!(err.to_string().contains("manifest.json not found"), "{err}");
        assert!(!root.join("server-demo-server").exists());
        // Temp dir cleaned up on error.
        let leftovers = std::fs::read_dir(&root).unwrap().flatten().count();
        assert_eq!(leftovers, 0);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn missing_required_field_rejected() {
        for omit in ["manifest_version", "name", "version", "server"] {
            let manifest: Value = serde_json::from_str(GOOD_MANIFEST).unwrap();
            let manifest = drop_key(manifest, omit);
            let base = temp_base("nofield");
            let root = base.join("uploads/mcpb");
            let mcpb = base.join("demo.mcpb");
            build_mcpb(
                &mcpb,
                &[("manifest.json", serde_json::to_string(&manifest).unwrap().as_bytes())],
            );
            let err = process_mcpb_in(&root, &mcpb).unwrap_err();
            assert!(
                err.to_string().contains(&format!("missing {omit}")),
                "key={omit} err={err}"
            );
            let _ = std::fs::remove_dir_all(&base);
        }
    }

    fn drop_key(mut v: Value, key: &str) -> Value {
        v.as_object_mut().unwrap().remove(key);
        v
    }

    #[test]
    fn unsafe_names_rejected() {
        for bad in ["../evil", "a/b", "a\\b", "..", "", "   ", "a..b", "has space", "$smart"] {
            let manifest: Value = serde_json::from_str(GOOD_MANIFEST).unwrap();
            let mut m = manifest.as_object().unwrap().clone();
            m.insert("name".into(), Value::String(bad.into()));
            let err = validate_mcpb_server_name(&Value::Object(m)).unwrap_err();
            assert!(
                err.to_string().contains("unsafe characters"),
                "name={bad:?} err={err}"
            );
        }
        // Non-string name is rejected too.
        assert!(validate_mcpb_server_name(&serde_json::json!(123)).is_err());
        // Valid names pass and are trimmed.
        assert_eq!(
            validate_mcpb_server_name(&serde_json::json!("  demo-server_1.0  ")).unwrap(),
            "demo-server_1.0"
        );
    }

    #[test]
    fn zip_slip_entry_rejected() {
        let base = temp_base("zipslip");
        let root = base.join("uploads/mcpb");
        let mcpb = base.join("evil.mcpb");
        // Try to build an archive with an escaping entry name. The zip writer
        // may sanitize on write; if it refuses, craft the archive via a raw
        // stored entry instead. Either way the READ side must reject.
        let built = std::panic::catch_unwind(|| {
            build_mcpb(
                &mcpb,
                &[
                    ("manifest.json", GOOD_MANIFEST.as_bytes()),
                    ("../evil.txt", b"pwned"),
                ],
            );
        });
        if built.is_err() {
            // Writer refused the unsafe name — nothing to test on the read side.
            let _ = std::fs::remove_dir_all(&base);
            return;
        }
        let err = process_mcpb_in(&root, &mcpb).unwrap_err();
        assert!(
            err.to_string().contains("unsafe path"),
            "zip-slip must be rejected: {err}"
        );
        assert!(!base.join("evil.txt").exists(), "must not write outside root");
        assert!(!root.join("server-demo-server").exists());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn manifest_field_truthiness_matches_js() {
        let ok = |v: Value| require_manifest_field(&serde_json::json!({ "k": v }), "k").is_ok();
        let bad = |v: Value| require_manifest_field(&serde_json::json!({ "k": v }), "k").is_err();
        // Truthy
        assert!(ok(serde_json::json!(2)));
        assert!(ok(serde_json::json!("0.2")));
        assert!(ok(serde_json::json!(true)));
        assert!(ok(serde_json::json!({ "a": 1 })));
        assert!(ok(serde_json::json!([1])));
        // Falsy (JS semantics)
        assert!(bad(Value::Null));
        assert!(bad(serde_json::json!("")));
        assert!(bad(serde_json::json!(false)));
        assert!(bad(serde_json::json!(0)));
        assert!(bad(serde_json::json!({})));
        assert!(bad(serde_json::json!([])));
        // Missing key
        assert!(require_manifest_field(&serde_json::json!({}), "k").is_err());
    }

    #[test]
    fn extract_dir_escape_check() {
        let base = temp_base("escape");
        let root = base.join("uploads/mcpb");
        std::fs::create_dir_all(&root).unwrap();
        // Charset-validated names can never escape, but the guard must hold.
        let dir = resolve_server_extract_dir(&root, "demo").unwrap();
        assert!(dir.starts_with(std::path::absolute(&root).unwrap()));
        let _ = std::fs::remove_dir_all(&base);
    }

    fn server_manifest(server: Value) -> Value {
        let mut m = serde_json::from_str::<Value>(GOOD_MANIFEST)
            .unwrap()
            .as_object()
            .unwrap()
            .clone();
        m.insert("server".into(), server);
        Value::Object(m)
    }

    #[test]
    fn unsupported_server_type_rejected() {
        for bad in ["rust", "deno", "go", "exe", "NODE"] {
            let err = validate_server_section(&server_manifest(serde_json::json!({
                "type": bad,
                "mcp_config": { "command": "node", "args": ["x.js"] }
            })))
            .unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("Unsupported server type"), "type={bad} err={msg}");
            assert!(msg.contains(bad), "type={bad} err={msg}");
        }
        // Non-string type is malformed, not "missing".
        let err = validate_server_section(&server_manifest(serde_json::json!({
            "type": 1,
            "mcp_config": { "command": "node" }
        })))
        .unwrap_err();
        assert!(err.to_string().contains("server.type must be a string"), "{err}");
    }

    #[test]
    fn missing_mcp_config_command_rejected_for_known_types() {
        for ty in ["node", "python", "binary"] {
            // mcp_config absent entirely
            let err = validate_server_section(&server_manifest(serde_json::json!({
                "type": ty, "entry_point": "server/index.js"
            })))
            .unwrap_err();
            assert!(
                err.to_string().contains("mcp_config.command is required"),
                "type={ty} err={err}"
            );
            // command present but empty/whitespace
            let err = validate_server_section(&server_manifest(serde_json::json!({
                "type": ty, "mcp_config": { "command": "   " }
            })))
            .unwrap_err();
            assert!(
                err.to_string().contains("mcp_config.command is required"),
                "type={ty} err={err}"
            );
        }
    }

    #[test]
    fn uv_type_command_or_entry_point() {
        // uv without mcp_config but with entry_point → accepted (frontend
        // synthesizes `uv run --directory ...`).
        validate_server_section(&server_manifest(serde_json::json!({
            "type": "uv", "entry_point": "src/server.py"
        })))
        .expect("uv with entry_point only must be accepted");
        // uv with explicit mcp_config → accepted.
        validate_server_section(&server_manifest(serde_json::json!({
            "type": "uv", "mcp_config": { "command": "uv", "args": ["run", "src/server.py"] }
        })))
        .expect("uv with mcp_config must be accepted");
        // uv with neither → rejected.
        let err = validate_server_section(&server_manifest(serde_json::json!({
            "type": "uv"
        })))
        .unwrap_err();
        assert!(err.to_string().contains("uv server requires"), "{err}");
    }

    #[test]
    fn missing_type_with_command_accepted() {
        // Origin parity: no type declared but a usable command exists.
        validate_server_section(&server_manifest(serde_json::json!({
            "mcp_config": { "command": "node", "args": ["x.js"] }
        })))
        .expect("typeless manifest with command must be accepted");
        // ...but typeless without command is rejected (would hang as node REPL).
        let err = validate_server_section(&server_manifest(serde_json::json!({
            "entry_point": "server/index.js"
        })))
        .unwrap_err();
        assert!(err.to_string().contains("mcp_config.command is missing"), "{err}");
    }

    #[test]
    fn end_to_end_unsupported_type_rejected_at_upload() {
        let base = temp_base("badtype");
        let root = base.join("uploads/mcpb");
        let mcpb = base.join("demo.mcpb");
        let manifest = server_manifest(serde_json::json!({
            "type": "rust",
            "mcp_config": { "command": "node", "args": ["x.js"] }
        }));
        build_mcpb(
            &mcpb,
            &[("manifest.json", serde_json::to_string(&manifest).unwrap().as_bytes())],
        );
        let err = process_mcpb_in(&root, &mcpb).unwrap_err();
        assert!(err.to_string().contains("Unsupported server type 'rust'"), "{err}");
        assert!(!root.join("server-demo-server").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Per-type happy path: each of the four spec types installs end-to-end
    /// (extract + validate + rename) and round-trips its manifest.
    #[test]
    fn each_supported_type_installs_end_to_end() {
        let cases: [(&str, Value); 3] = [
            (
                "node",
                serde_json::json!({
                    "type": "node", "entry_point": "server/index.js",
                    "mcp_config": { "command": "node", "args": ["${__dirname}/server/index.js"] }
                }),
            ),
            (
                "python",
                serde_json::json!({
                    "type": "python", "entry_point": "server/main.py",
                    "mcp_config": { "command": "python", "args": ["${__dirname}/server/main.py"],
                                    "env": { "PYTHONPATH": "${__dirname}/server/lib" } }
                }),
            ),
            (
                "binary",
                serde_json::json!({
                    "type": "binary", "entry_point": "server/my-tool",
                    "mcp_config": { "command": "server/my-tool", "args": ["--config", "server/config.json"] }
                }),
            ),
        ];
        for (ty, server) in cases {
            let base = temp_base("type-e2e");
            let root = base.join("uploads/mcpb");
            let mcpb = base.join("demo.mcpb");
            let manifest = server_manifest(server);
            build_mcpb(
                &mcpb,
                &[
                    ("manifest.json", serde_json::to_string(&manifest).unwrap().as_bytes()),
                    ("server/index.js", b"entry"),
                    ("server/lib/pkg.py", b"dep"),
                ],
            );
            let result = process_mcpb_in(&root, &mcpb)
                .unwrap_or_else(|e| panic!("type={ty} must install: {e}"));
            assert_eq!(result.manifest["server"]["type"], ty, "type={ty}");
            let dir = PathBuf::from(&result.extract_dir);
            assert!(dir.join("server/index.js").exists(), "type={ty}");
            assert!(dir.join("server/lib/pkg.py").exists(), "type={ty}");
            assert!(mcpb.exists(), "type={ty}: user file kept");
            let _ = std::fs::remove_dir_all(&base);
        }
    }

    /// uv type: both spec-legal forms install end-to-end.
    #[test]
    fn uv_type_installs_end_to_end_both_forms() {
        // Form 1: explicit mcp_config (used as-is by the frontend).
        // Form 2: entry_point only (frontend synthesizes `uv run --directory`).
        let cases = [
            (
                "explicit-mcp-config",
                serde_json::json!({
                    "type": "uv", "entry_point": "src/server.py",
                    "mcp_config": { "command": "uv", "args": ["run", "src/server.py"] }
                }),
            ),
            (
                "entry-point-only",
                serde_json::json!({ "type": "uv", "entry_point": "src/server.py" }),
            ),
        ];
        for (form, server) in cases {
            let base = temp_base("uv-e2e");
            let root = base.join("uploads/mcpb");
            let mcpb = base.join("demo.mcpb");
            let manifest = server_manifest(server);
            build_mcpb(
                &mcpb,
                &[
                    ("manifest.json", serde_json::to_string(&manifest).unwrap().as_bytes()),
                    ("pyproject.toml", b"[project]\nname='demo'\n"),
                    ("src/server.py", b"print('hi')"),
                ],
            );
            let result = process_mcpb_in(&root, &mcpb)
                .unwrap_or_else(|e| panic!("uv form={form} must install: {e}"));
            assert_eq!(result.manifest["server"]["type"], "uv", "form={form}");
            let dir = PathBuf::from(&result.extract_dir);
            assert!(dir.join("pyproject.toml").exists(), "form={form}");
            assert!(dir.join("src/server.py").exists(), "form={form}");
            let _ = std::fs::remove_dir_all(&base);
        }
    }
}
