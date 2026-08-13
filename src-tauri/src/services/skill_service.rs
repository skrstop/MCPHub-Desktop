//! Skills service: agent config + library + scan/import + reconcile.
//!
//! - 2.2: agent config (`list_agents` / `save_agents`)
//! - 2.3: home/path resolution, library dir, copy_dir_recursive, scan_for_import,
//!        list_library, get_skill, import_skills, reconcile_pending
//! - 2.4–2.5 (export/uninstall/delete) added later; see `doc/agent_20260724.md` §3.8.

use crate::{db, models::skill::*, services::config_service};
use anyhow::{anyhow, Result};
// serde is only used by the Windows elevation manifest structs below.
#[cfg(windows)]
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

// ── 2.2: agent config ───────────────────────────────────────────────────────

/// The known-agents catalog bundled into the binary at compile time. Map of
/// agent display name → skills path relative to the user's home (e.g.
/// "Claude Code" → ".claude/skills"). Source: `runtimes/skill/install.json`.
const KNOWN_AGENTS_JSON: &str = include_str!("../../runtimes/skill/install.json");

/// Slugify a display name into a stable id: lowercase, non-alphanumeric runs
/// collapse to a single hyphen, no leading/trailing hyphens.
fn slugify(name: &str) -> String {
    let mut s = String::with_capacity(name.len());
    let mut prev_dash = true; // suppress leading hyphens
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            s.push('-');
            prev_dash = true;
        }
    }
    while s.ends_with('-') {
        s.pop();
    }
    s
}

/// Default known agents parsed from the bundled `install.json` catalog. Each
/// entry: id = slugify(name), name = display name, skillsPath = `~/` + the
/// catalog's relative path (resolved at scan/export time). Used to seed
/// config on first migration (v13), backfill/replace via v14, and as the
/// `list_agents` fallback when the user has cleared the list.
pub(crate) fn default_agents() -> Vec<SkillAgent> {
    // BTreeMap → deterministic iteration (sorted by name) → stable ids.
    let map: std::collections::BTreeMap<String, String> =
        serde_json::from_str(KNOWN_AGENTS_JSON).unwrap_or_default();
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut out = Vec::with_capacity(map.len());
    for (name, rel) in map {
        let base = slugify(&name);
        // Deterministic dedup: on collision, append -2, -3, ...
        let id = match seen.get_mut(&base) {
            Some(n) => {
                *n += 1;
                format!("{}-{}", base, n)
            }
            None => {
                seen.insert(base.clone(), 1);
                base
            }
        };
        let rel = rel.trim_start_matches('/');
        let skills_path = format!("~/{}", rel);
        out.push(SkillAgent { id, name, skills_path, custom: false });
    }
    out
}

/// Whether an agent id belongs to the bundled catalog (built-in / read-only).
pub fn is_builtin_id(id: &str) -> bool {
    default_agents().iter().any(|a| a.id == id)
}

pub async fn list_agents() -> Result<Vec<SkillAgent>> {
    let cfg = config_service::get().await?;
    let agents: Vec<SkillAgent> = if let Some(arr) = cfg.get("skills").and_then(|s| s.get("agents")).and_then(|a| a.as_array()) {
        arr.iter()
            .filter_map(|a| serde_json::from_value::<SkillAgent>(a.clone()).ok())
            .collect()
    } else {
        default_agents()
    };
    // Recompute `custom` from the catalog: built-in ids → false, user-added → true.
    let builtin: std::collections::HashSet<String> =
        default_agents().into_iter().map(|a| a.id).collect();
    Ok(agents
        .into_iter()
        .map(|mut a| {
            a.custom = !builtin.contains(&a.id);
            a
        })
        .collect())
}

pub async fn save_agents(agents: Vec<SkillAgent>) -> Result<()> {
    let patch = json!({ "skills": { "agents": agents } });
    config_service::update(&patch).await?;
    Ok(())
}

/// Create a new custom agent (user-added). Refuses to overwrite a built-in id.
/// `skillsPath` may start with `~` (expanded at scan/export time). Validates:
///   - name non-empty, not a built-in name
///   - no existing agent with the same name (case-insensitive) or same path
///   - `skillsPath` resolves to an existing directory (after `~` expansion)
pub async fn create_custom_agent(name: &str, skills_path: &str) -> Result<SkillAgent> {
    let name = name.trim();
    let skills_path = skills_path.trim();
    if name.is_empty() {
        return Err(anyhow!("agent name is required"));
    }
    if skills_path.is_empty() {
        return Err(anyhow!("agent skills path is required"));
    }
    let base = slugify(name);
    if base.is_empty() {
        return Err(anyhow!("agent name must contain alphanumeric chars"));
    }
    if is_builtin_id(&base) {
        return Err(anyhow!("'{}' is a built-in agent name", name));
    }

    let mut agents = list_agents().await.unwrap_or_default();

    // Duplicate name check (case-insensitive).
    if agents
        .iter()
        .any(|a| a.name.eq_ignore_ascii_case(name))
    {
        return Err(anyhow!("an agent named '{}' already exists", name));
    }

    // Path must resolve to an existing directory.
    let resolved = resolve_agent_path(skills_path).ok_or_else(|| {
        anyhow!("skills path could not be resolved (failed to expand '~'): {}", skills_path)
    })?;
    match std::fs::metadata(&resolved) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => return Err(anyhow!("skills path is not a directory: {}", resolved.display())),
        Err(_) => {
            return Err(anyhow!(
                "skills path does not exist: {} (resolved to {})",
                skills_path,
                resolved.display()
            ))
        }
    }
    // Duplicate path check (compare resolved paths).
    for a in &agents {
        if let Some(r) = resolve_agent_path(&a.skills_path) {
            if r == resolved {
                return Err(anyhow!(
                    "skills path already used by agent '{}'",
                    a.name
                ));
            }
        }
    }
    // De-dup against existing custom ids with the same base (-2, -3, ...).
    let mut n = 1;
    let mut id = base.clone();
    loop {
        if !agents.iter().any(|a| a.id == id) {
            break;
        }
        n += 1;
        id = format!("{}-{}", base, n);
    }
    let agent = SkillAgent {
        id,
        name: name.to_string(),
        skills_path: skills_path.to_string(),
        custom: true,
    };
    agents.push(agent.clone());
    save_agents(agents).await?;
    Ok(agent)
}

/// Delete a custom agent by id. Refuses to delete built-in agents.
pub async fn delete_custom_agent(id: &str) -> Result<()> {
    if is_builtin_id(id) {
        return Err(anyhow!("built-in agents cannot be deleted"));
    }
    let mut agents = list_agents().await?;
    let before = agents.len();
    agents.retain(|a| a.id != id);
    if agents.len() == before {
        return Err(anyhow!("agent not found: {}", id));
    }
    save_agents(agents).await?;
    Ok(())
}

// ── 2.3: path helpers ──────────────────────────────────────────────────────

/// User home root (cross-platform): mac/linux=HOME, windows=USERPROFILE.
/// Distinct from `runtime_env::app_data_dir` (which appends `mcphub-desktop`).
pub fn home_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    { std::env::var("USERPROFILE").ok().map(PathBuf::from) }
    #[cfg(not(target_os = "windows"))]
    { std::env::var("HOME").ok().map(PathBuf::from) }
}

/// Resolve a skillsPath that may start with `~` (expanded via home_dir) or be
/// an absolute path. Returns None if `~` can't be expanded or the path is empty.
pub fn resolve_agent_path(raw: &str) -> Option<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(rest) = raw.strip_prefix('~') {
        let home = home_dir()?;
        let rest = rest.trim_start_matches(['/', '\\']);
        Some(home.join(rest))
    } else {
        Some(PathBuf::from(raw))
    }
}

/// App-managed skills library dir: `$APPDATA/skills`.
pub fn library_dir(app: &AppHandle) -> Result<PathBuf> {
    Ok(app.path().app_data_dir()?.join("skills"))
}

// ── 2.3: file copy (delete-then-copy, no overlay) ───────────────────────────

/// Recursively copy `src` into `dst`. **If dst exists it is removed first**
/// (never incremental overlay — so files deleted in src don't linger in dst).
/// Symlinks inside src are followed so the library stays all-real-files.
pub fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(dst) {
        Ok(m) => {
            if m.file_type().is_symlink() {
                remove_link(dst)?;
            } else if m.is_dir() {
                fs::remove_dir_all(dst)?;
            } else {
                fs::remove_file(dst)?;
            }
        }
        Err(_) => {}
    }
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let meta = fs::symlink_metadata(&from)?;
        if meta.file_type().is_symlink() {
            // Follow the link: copy target contents (keep library real).
            if fs::metadata(&from).map(|m| m.is_dir()).unwrap_or(false) {
                copy_dir_recursive(&from, &to)?;
            } else {
                fs::copy(&from, &to)?;
            }
        } else if meta.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Remove a path that may be a symlink (to dir or file) or a real dir/file.
/// Never `remove_dir_all` a symlink — that would recurse into the target and
/// delete real library files. For a symlink: remove_file then fall back to
/// remove_dir (covers Windows dir-symlinks where remove_file fails).
pub fn remove_link(p: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(p) {
        Ok(m) => {
            if m.file_type().is_symlink() {
                fs::remove_file(p).or_else(|_| fs::remove_dir(p))
            } else if m.is_dir() {
                fs::remove_dir_all(p)
            } else {
                fs::remove_file(p)
            }
        }
        Err(_) => Ok(()), // not present
    }
}

// ── 2.3: SKILL.md frontmatter parsing ───────────────────────────────────────

/// Read `<dir>/SKILL.md` (case-insensitive: SKILL.md / skill.md) and parse the
/// YAML frontmatter `name` / `description`. Supports inline scalars
/// (`key: value`, optionally quoted) and folded (`>`) / literal (`|`) block
/// scalars (e.g. `description: >` followed by indented lines). Falls back to
/// dir name / empty.
fn parse_skill_md(dir: &Path) -> (String, String) {
    let dir_name = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let content = match read_skill_md(dir) {
        Some(c) => c,
        None => return (dir_name, String::new()),
    };
    let mut lines = content.lines().peekable();
    let first = lines.next().map(|s| s.trim()).unwrap_or("");
    if first != "---" {
        return (dir_name, String::new()); // no frontmatter
    }
    let mut name: Option<String> = None;
    let mut desc: Option<String> = None;
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        // `key: value` (split at first colon).
        let Some((key, value_part)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value_part = value_part.trim();

        // Folded (>) or literal (|) block scalar: the value is the following
        // indented lines (possibly with chomp/indent indicators like `>-`).
        let value = if is_block_indicator(value_part) {
            let folded = value_part.starts_with('>');
            let block = collect_indented_block(&mut lines);
            if folded {
                // Folded: join non-blank lines with a space.
                block.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ")
            } else {
                // Literal: preserve newlines, strip trailing.
                block.join("\n")
            }
        } else {
            trim_yaml_value(value_part)
        };

        match key {
            "name" => name = Some(value),
            "description" => desc = Some(value),
            _ => {}
        }
    }
    (name.unwrap_or(dir_name), desc.unwrap_or_default().trim().to_string())
}

/// Collect indented (or blank) lines following a block scalar indicator, until
/// a non-indented non-blank line, `---`, or EOF. Returns each content line
/// trimmed (blank lines → empty string).
fn collect_indented_block<'a, I: Iterator<Item = &'a str>>(lines: &mut std::iter::Peekable<I>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    while let Some(&next) = lines.peek() {
        if next.trim() == "---" {
            break;
        }
        if next.trim().is_empty() {
            // blank line: part of the block (paragraph break for folded).
            out.push(String::new());
            lines.next();
            continue;
        }
        if next.starts_with(char::is_whitespace) {
            out.push(next.trim().to_string());
            lines.next();
        } else {
            // non-indented → block ends.
            break;
        }
    }
    out
}

/// Read `<dir>/SKILL.md` (case-insensitive) as text. Reads raw bytes and
/// decodes to UTF-8 (BOM/UTF-8 fast path → chardetng + encoding_rs) so a
/// non-UTF-8 frontmatter still parses. Only this parse step decodes; the
/// actual file copy (`copy_dir_recursive`) uses byte-level `fs::copy` and
/// preserves the original bytes unchanged.
fn read_skill_md(dir: &Path) -> Option<String> {
    for name in ["SKILL.md", "skill.md"] {
        let path = dir.join(name);
        if let Ok(bytes) = fs::read(&path) {
            let label = path.to_string_lossy().to_string();
            return Some(crate::rag::service::decode_text(&bytes, &label).0);
        }
    }
    None
}

/// Strip leading spaces + surrounding quotes from a YAML scalar value.
fn trim_yaml_value(v: &str) -> String {
    let v = v.trim();
    let v = v.trim_matches(['"', '\'']);
    v.to_string()
}

/// Whether `v` is a YAML block scalar indicator (`>`, `|`, with optional
/// digits and a `+`/`-` chomp) — e.g. `>`, `>-`, `>2-`, `|+`. Distinguishes
/// the block form from a literal value that merely starts with `>`/`|`.
fn is_block_indicator(v: &str) -> bool {
    let mut chars = v.chars();
    matches!(chars.next(), Some('>' | '|')) && chars.all(|c| c.is_ascii_digit() || c == '+' || c == '-')
}

// ── 2.3: scan ───────────────────────────────────────────────────────────────

/// Scan every configured agent's skillsPath for skill directories.
/// - Real dirs → importable (is_symlink=false).
/// - Symlinks pointing into our library (our own export artifacts) → skipped.
/// - Other symlinks (e.g. Claude Code's centrally-managed skills) → shown but
///   marked is_symlink=true (NOT importable — they're soft references; the
///   real content lives elsewhere). name/description read via the link.
pub async fn scan_for_import(app: &AppHandle) -> Result<Vec<ScannedSkill>> {
    // Library dir: `lib` for the already-imported FS check; `lib_canon`
    // (canonical) to detect symlinks that are our own exports (point into the
    // library) and skip them — avoid circular re-import.
    let lib = library_dir(app)?;
    let lib_canon = fs::canonicalize(&lib).ok();

    let agents = list_agents().await?;
    let mut out: Vec<ScannedSkill> = Vec::new();
    for agent in &agents {
        let root = match resolve_agent_path(&agent.skills_path) {
            Some(p) => p,
            None => continue, // ~ unresolvable → skip agent
        };
        let entries = match fs::read_dir(&root) {
            Ok(e) => e,
            Err(_) => continue, // path missing/unreadable → skip agent
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let meta = match fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let is_symlink = meta.file_type().is_symlink();

            if is_symlink {
                // Skip symlinks that point into our library (our own exports).
                if let Some(lib_c) = &lib_canon {
                    if let Ok(target_c) = fs::canonicalize(&path) {
                        if target_c.starts_with(lib_c) {
                            continue; // our export → skip
                        }
                    }
                }
                // Only include symlink skills whose target is a directory.
                if !fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false) {
                    continue;
                }
                // Shown but not importable (is_symlink=true; frontend disables).
            } else if !meta.is_dir() {
                continue; // non-symlink non-dir → not a skill
            }

            let dir_name = entry.file_name().to_string_lossy().to_string();
            // parse_skill_md reads SKILL.md through the symlink (follows it).
            let (name, description) = parse_skill_md(&path);
            // Filesystem check: already imported if the library dir has this
            // dir_name (NOT a DB lookup — external agent folders are
            // uncontrollable, so trust the actual library copy on disk).
            let already_imported = lib.join(&dir_name).exists();
            out.push(ScannedSkill {
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
                dir_name,
                name,
                description,
                path: path.to_string_lossy().to_string(),
                is_symlink,
                already_imported,
            });
        }
    }
    Ok(out)
}

// ── 2.3: list / get ─────────────────────────────────────────────────────────

/// DB-driven + FS-verified: returns `skill_id → [SkillExport]` for
/// skill_exports rows whose install STILL exists on disk at
/// `<agent_path>/<dir_name>`. Stale rows (user deleted the install) are
/// skipped (cleaned at startup by reconcile_pending).
///
/// Why DB + FS (not pure FS): a pure FS scan can't tell our copy install from
/// the agent's OWN same-named skill (both are real dirs) → over-reports. The
/// DB skill_exports tracks ONLY installs WE made (accurate); the FS check
/// guarantees freshness (no stale "installed" when the file's gone). This
/// satisfies "actual comparison" (FS verify) while being accurate.
async fn verified_exports_by_skill() -> Result<HashMap<String, Vec<SkillExport>>> {
    let agents = list_agents().await?;
    let agent_path: HashMap<String, Option<PathBuf>> =
        agents.iter().map(|a| (a.id.clone(), resolve_agent_path(&a.skills_path))).collect();
    let agent_name: HashMap<String, String> =
        agents.iter().map(|a| (a.id.clone(), a.name.clone())).collect();

    let rows = sqlx::query(
        "SELECT se.skill_id, se.agent_id, se.method, se.created_at, s.dir_name \
         FROM skill_exports se JOIN skills s ON s.id = se.skill_id",
    )
    .fetch_all(db::pool())
    .await?;

    let mut map: HashMap<String, Vec<SkillExport>> = HashMap::new();
    for r in &rows {
        let skill_id: String = r.try_get("skill_id").unwrap_or_default();
        let agent_id: String = r.try_get("agent_id").unwrap_or_default();
        let method: String = r.try_get("method").unwrap_or_default();
        let dir_name: String = r.try_get("dir_name").unwrap_or_default();
        let created_at: Option<String> = r.try_get("created_at").ok().flatten();
        // FS verify: the install must still exist at the agent's path.
        let exists = agent_path
            .get(&agent_id)
            .and_then(|ap| ap.as_ref())
            .map(|ap| ap.join(&dir_name).exists())
            .unwrap_or(false);
        if !exists {
            continue; // stale (user deleted) → skip; cleaned by reconcile_pending
        }
        map.entry(skill_id).or_default().push(SkillExport {
            agent_id: agent_id.clone(),
            agent_name: agent_name.get(&agent_id).cloned().unwrap_or_default(),
            method,
            created_at,
        });
    }
    Ok(map)
}

/// List library skills ordered by dir_name. A skill is returned ONLY if its
/// library copy actually exists on disk (`<library_dir>/<dir_name>`) — this is
/// a filesystem check, not DB state, so a skill whose file the user deleted
/// no longer shows. exports come from verified_exports_by_skill (DB-tracked
/// installs, FS-verified), keyed by skill id.
pub async fn list_library(app: &AppHandle) -> Result<Vec<Skill>> {
    let lib = library_dir(app)?;
    let exports_by_skill = verified_exports_by_skill().await?;
    let rows = sqlx::query(
        "SELECT id, dir_name, name, description, source_agent, source_path, created_at \
         FROM skills WHERE status='ok' ORDER BY dir_name",
    )
    .fetch_all(db::pool())
    .await?;

    rows.iter()
        .filter_map(|r| {
            let dir_name: String = r.try_get("dir_name").ok()?;
            // FS truth: skip skills whose library copy is gone.
            if !lib.join(&dir_name).exists() {
                return None;
            }
            let id: String = r.try_get("id").ok()?;
            let exports = exports_by_skill.get(&id).cloned().unwrap_or_default();
            Some(Ok(Skill {
                exports,
                id,
                dir_name,
                name: r.try_get("name").ok().flatten(),
                description: r.try_get("description").ok().flatten(),
                source_agent: r.try_get("source_agent").ok().flatten(),
                source_path: r.try_get("source_path").ok().flatten(),
                created_at: r.try_get("created_at").ok().flatten(),
            }))
        })
        .collect()
}

/// Get a single skill; returns None if the DB row is gone OR its library copy
/// no longer exists on disk (filesystem check). exports from
/// verified_exports_by_skill (DB-tracked installs, FS-verified).
pub async fn get_skill(app: &AppHandle, id: &str) -> Result<Option<Skill>> {
    let row = sqlx::query(
        "SELECT id, dir_name, name, description, source_agent, source_path, created_at \
         FROM skills WHERE id=? AND status='ok'",
    )
    .bind(id)
    .fetch_optional(db::pool())
    .await?;
    let Some(r) = row else { return Ok(None) };
    let lib = library_dir(app)?;
    let dir_name: String = r.try_get("dir_name")?;
    // FS truth: the library copy must actually exist.
    if !lib.join(&dir_name).exists() {
        return Ok(None);
    }
    let exports_by_skill = verified_exports_by_skill().await?;
    let skill_id: String = r.try_get("id")?;
    let exports = exports_by_skill.get(&skill_id).cloned().unwrap_or_default();
    Ok(Some(Skill {
        id: skill_id,
        dir_name,
        name: r.try_get("name").ok().flatten(),
        description: r.try_get("description").ok().flatten(),
        source_agent: r.try_get("source_agent").ok().flatten(),
        source_path: r.try_get("source_path").ok().flatten(),
        created_at: r.try_get("created_at").ok().flatten(),
        exports,
    }))
}

// ── 2.3: import ─────────────────────────────────────────────────────────────

/// Import selected skills into the library. Each item is either:
/// - agent-grouped (agent_id set, path None): source = <agent.skillsPath>/<dir_name>,
///   and a source-agent install record (method=copy) is written so the skill
///   shows where it came from.
/// - manual (path Some, agent_id ignored): source = the given path (a folder
///   the user picked manually); NO source-agent record (no default origin).
/// State machine: INSERT pending → copy_dir_recursive → UPDATE ok on success;
/// on failure the partial dir + row are removed. Crash mid-copy → status=pending
/// → reconcile_pending cleans at next startup.
pub async fn import_skills(app: &AppHandle, items: Vec<ImportItem>) -> Result<ImportSummary> {
    let lib = library_dir(app)?;
    fs::create_dir_all(&lib).ok();
    let agents = list_agents().await?;
    let agent_by_id: HashMap<String, &SkillAgent> = agents.iter().map(|a| (a.id.clone(), a)).collect();

    let mut results: Vec<ImportResultItem> = Vec::new();
    let mut success_count = 0usize;
    let mut failure_count = 0usize;

    for item in items {
        let dir_name = item.dir_name.trim().to_string();
        if dir_name.is_empty() {
            failure_count += 1;
            results.push(ImportResultItem { dir_name: String::new(), success: false, message: Some("empty dir name".into()) });
            continue;
        }
        // Filesystem check (not DB): if the library already has this dir, skip.
        if lib.join(&dir_name).exists() {
            failure_count += 1;
            results.push(ImportResultItem { dir_name, success: false, message: Some("already imported".into()) });
            continue;
        }

        // Resolve source: manual (path) or agent-grouped.
        let (src, source_agent_id): (PathBuf, Option<String>) = match &item.path {
            Some(p) if !p.trim().is_empty() => (PathBuf::from(p), None),
            _ => {
                let agent = match agent_by_id.get(&item.agent_id) {
                    Some(a) => *a,
                    None => {
                        failure_count += 1;
                        results.push(ImportResultItem { dir_name, success: false, message: Some("agent not found".into()) });
                        continue;
                    }
                };
                let p = match resolve_agent_path(&agent.skills_path) {
                    Some(p) => p,
                    None => {
                        failure_count += 1;
                        results.push(ImportResultItem { dir_name, success: false, message: Some("agent path unresolvable".into()) });
                        continue;
                    }
                };
                (p.join(&dir_name), Some(agent.id.clone()))
            }
        };

        let (name, description) = parse_skill_md(&src);
        let id = Uuid::new_v4().to_string();
        let dst = lib.join(&dir_name);

        // 1. pending insert (placeholder; counts as not-imported until ok)
        if let Err(e) = sqlx::query(
            "INSERT INTO skills (id, dir_name, name, description, source_agent, source_path, status) \
             VALUES (?, ?, ?, ?, ?, ?, 'pending')",
        )
        .bind(&id)
        .bind(&dir_name)
        .bind(&name)
        .bind(&description)
        .bind(&source_agent_id)
        .bind(src.to_string_lossy().to_string())
        .execute(db::pool())
        .await
        {
            failure_count += 1;
            results.push(ImportResultItem { dir_name, success: false, message: Some(format!("db: {}", e)) });
            continue;
        }

        // 2. copy (delete-then-copy)
        match copy_dir_recursive(&src, &dst) {
            Ok(()) => {
                // 3. mark ok
                let _ = sqlx::query("UPDATE skills SET status='ok' WHERE id=?")
                    .bind(&id)
                    .execute(db::pool())
                    .await;
                // Source-agent install record (copy) — ONLY for agent-grouped
                // imports (manual imports have no default source agent).
                if let Some(sa) = &source_agent_id {
                    let _ = sqlx::query(
                        "INSERT OR IGNORE INTO skill_exports (id, skill_id, agent_id, method, status) \
                         VALUES (?, ?, ?, 'copy', 'ok')",
                    )
                    .bind(Uuid::new_v4().to_string())
                    .bind(&id)
                    .bind(sa)
                    .execute(db::pool())
                    .await;
                }
                success_count += 1;
                results.push(ImportResultItem { dir_name, success: true, message: None });
            }
            Err(e) => {
                let _ = remove_link(&dst);
                let _ = sqlx::query("DELETE FROM skills WHERE id=?")
                    .bind(&id)
                    .execute(db::pool())
                    .await;
                failure_count += 1;
                results.push(ImportResultItem { dir_name, success: false, message: Some(format!("copy: {}", e)) });
            }
        }
    }

    Ok(ImportSummary { success_count, failure_count, results })
}

// ── 2.3: manual folder scan (2-layer skill detection) ───────────────────────

/// Scan a manually-selected folder for skills (max 2 layers of nesting):
/// - Layer 1: if the folder itself has a SKILL.md → it's a single skill.
/// - Layer 2: else, scan the folder's DIRECT children for SKILL.md.
/// - No 3rd+ layer (grandchildren not scanned).
/// Returns skills with agent_id="__manual__" (no source agent). Empty if none.
pub async fn scan_folder_for_skills(app: &AppHandle, folder: &str) -> Result<Vec<ScannedSkill>> {
    let lib = library_dir(app)?;
    let root = PathBuf::from(folder);
    let mut out: Vec<ScannedSkill> = Vec::new();

    let push = |out: &mut Vec<ScannedSkill>, dir: &Path, dir_name: &str| {
        let (name, description) = parse_skill_md(dir);
        let already_imported = lib.join(dir_name).exists();
        // Detect symlinks so the frontend excludes them from importability
        // (软引用 ignored) — same rule as the agent scan.
        let is_symlink = fs::symlink_metadata(dir)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        out.push(ScannedSkill {
            agent_id: "__manual__".into(),
            agent_name: String::new(),
            dir_name: dir_name.to_string(),
            name,
            description,
            path: dir.to_string_lossy().to_string(),
            is_symlink,
            already_imported,
        });
    };

    // Layer 1: the folder itself is a skill.
    if read_skill_md(&root).is_some() {
        if let Some(name) = root.file_name() {
            let dir_name = name.to_string_lossy().to_string();
            push(&mut out, &root, &dir_name);
        }
        return Ok(out);
    }

    // Layer 2: direct children with SKILL.md.
    if let Ok(entries) = fs::read_dir(&root) {
        for entry in entries.flatten() {
            let child = entry.path();
            if read_skill_md(&child).is_some() {
                let dir_name = entry.file_name().to_string_lossy().to_string();
                push(&mut out, &child, &dir_name);
            }
        }
    }
    Ok(out)
}

// ── 2.3: reconcile (startup cleanup of crashed-pending) ─────────────────────

/// Remove `pending` skills/exports left by a crashed import/export:
/// - pending skills: delete partial library dir + row.
/// - pending exports: delete partial target at <agent_path>/<dir_name> + row.
/// - ok skills whose library copy is gone (user deleted the file): delete the
///   row (+ exports via the orphan cleanup below) — DB stays in sync with FS.
/// Only `ok` is treated as installed; pending never shows in list_library.
pub async fn reconcile_pending(app: &AppHandle) -> Result<()> {
    let lib = library_dir(app)?;

    // Crashed-pending skills: delete partial library dir + row.
    let pending_skills = sqlx::query("SELECT id, dir_name FROM skills WHERE status='pending'")
        .fetch_all(db::pool())
        .await?;
    for r in &pending_skills {
        let id: String = r.try_get("id").unwrap_or_default();
        let dir_name: String = r.try_get("dir_name").unwrap_or_default();
        if !dir_name.is_empty() {
            let _ = remove_link(&lib.join(&dir_name));
        }
        let _ = sqlx::query("DELETE FROM skills WHERE id=?").bind(&id).execute(db::pool()).await;
    }

    let agents = list_agents().await?;
    let agent_path: HashMap<String, Option<PathBuf>> =
        agents.iter().map(|a| (a.id.clone(), resolve_agent_path(&a.skills_path))).collect();

    let pending_exports = sqlx::query("SELECT id, skill_id, agent_id FROM skill_exports WHERE status='pending'")
        .fetch_all(db::pool())
        .await?;
    // dir_name per skill_id (status-agnostic; skill may be gone already)
    let skill_rows = sqlx::query("SELECT id, dir_name FROM skills").fetch_all(db::pool()).await?;
    let dir_by_skill: HashMap<String, String> = skill_rows
        .iter()
        .filter_map(|r| {
            let id: String = r.try_get("id").ok()?;
            let dn: String = r.try_get("dir_name").ok()?;
            Some((id, dn))
        })
        .collect();

    for r in &pending_exports {
        let id: String = r.try_get("id").unwrap_or_default();
        let skill_id: String = r.try_get("skill_id").unwrap_or_default();
        let agent_id: String = r.try_get("agent_id").unwrap_or_default();
        if let (Some(Some(ap)), Some(dn)) = (agent_path.get(&agent_id), dir_by_skill.get(&skill_id)) {
            let _ = remove_link(&ap.join(dn));
        }
        let _ = sqlx::query("DELETE FROM skill_exports WHERE id=?").bind(&id).execute(db::pool()).await;
    }

    // Clean orphaned skill_exports: rows whose skill_id no longer exists (from
    // deletes that ran before the explicit transactional cleanup was added).
    let orphan_exports = sqlx::query(
        "DELETE FROM skill_exports WHERE skill_id NOT IN (SELECT id FROM skills)",
    )
    .execute(db::pool())
    .await?
    .rows_affected();

    // FS reconcile: delete 'ok' skill rows whose library copy is gone (the
    // user deleted the actual file). list_library already filters these out,
    // but this keeps the DB in sync so the row/exports don't linger.
    let gone_skills = sqlx::query("SELECT id, dir_name FROM skills WHERE status='ok'")
        .fetch_all(db::pool())
        .await?;
    let mut gone_count = 0usize;
    for r in &gone_skills {
        let id: String = r.try_get("id").unwrap_or_default();
        let dir_name: String = r.try_get("dir_name").unwrap_or_default();
        if !dir_name.is_empty() && !lib.join(&dir_name).exists() {
            let mut tx = db::pool().begin().await?;
            let _ = sqlx::query("DELETE FROM skill_exports WHERE skill_id=?")
                .bind(&id).execute(&mut *tx).await?;
            let _ = sqlx::query("DELETE FROM skills WHERE id=?")
                .bind(&id).execute(&mut *tx).await?;
            tx.commit().await?;
            gone_count += 1;
        }
    }

    // FS reconcile for exports: delete skill_exports rows whose install is gone
    // from the agent's path (user deleted the symlink/copy). Keeps the DB in
    // sync so verified_exports_by_skill doesn't carry stale rows.
    let stale_exports = sqlx::query(
        "SELECT se.id, se.agent_id, s.dir_name \
         FROM skill_exports se JOIN skills s ON s.id = se.skill_id",
    )
    .fetch_all(db::pool())
    .await?;
    let mut stale_count = 0usize;
    for r in &stale_exports {
        let id: String = r.try_get("id").unwrap_or_default();
        let agent_id: String = r.try_get("agent_id").unwrap_or_default();
        let dir_name: String = r.try_get("dir_name").unwrap_or_default();
        let exists = agent_path
            .get(&agent_id)
            .and_then(|ap| ap.as_ref())
            .map(|ap| ap.join(&dir_name).exists())
            .unwrap_or(false);
        if !exists {
            let _ = sqlx::query("DELETE FROM skill_exports WHERE id=?")
                .bind(&id)
                .execute(db::pool())
                .await;
            stale_count += 1;
        }
    }

    if !pending_skills.is_empty() || !pending_exports.is_empty() || orphan_exports > 0 || gone_count > 0 || stale_count > 0 {
        log::info!(
            "[skills] reconcile: cleaned {} pending skills, {} pending exports, {} orphaned exports, {} gone library copies, {} stale exports",
            pending_skills.len(),
            pending_exports.len(),
            orphan_exports,
            gone_count,
            stale_count,
        );
    }
    Ok(())
}

// ── 2.4: export/install (idempotent rebuild + Windows on-demand elevation) ──

// The manifest structs are only used by the Windows elevation helper
// (create_symlinks_elevated / run_helper_inner), so gate them to windows to
// avoid dead-code warnings on mac/linux.
#[cfg(windows)]
#[derive(Serialize, Deserialize)]
struct ManifestItem {
    link: String,
    target: String,
}

#[cfg(windows)]
#[derive(Serialize, Deserialize)]
struct Manifest {
    items: Vec<ManifestItem>,
}

#[cfg(windows)]
#[derive(Serialize, Deserialize, Clone)]
struct ManifestResultItem {
    link: String,
    success: bool,
    message: Option<String>,
}

#[cfg(windows)]
#[derive(Serialize, Deserialize)]
struct ManifestResults {
    results: Vec<ManifestResultItem>,
}

/// Whether an io::Error is a Windows symlink-privilege error. Non-Windows
/// always returns false (symlinks need no privilege there).
#[cfg(windows)]
fn is_privilege_error(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(1314) | Some(5)) // ERROR_PRIVILEGE_NOT_HELD | ERROR_ACCESS_DENIED
}
#[cfg(not(windows))]
#[allow(dead_code)]
fn is_privilege_error(_e: &std::io::Error) -> bool {
    false
}

/// Export/install skills to agents. State machine per (skill, agent):
/// INSERT pending → remove old install → create (symlink/copy) → UPDATE ok on
/// success (or DELETE row on failure). For Windows symlink with insufficient
/// privilege, items are batched and created via an elevated self-relaunch
/// (one UAC for the whole batch). Idempotent rebuild: always delete-then-build
/// regardless of old/new method, so updated library files are re-distributed.
pub async fn export_to_agents(
    app: &AppHandle,
    skill_ids: Vec<String>,
    agent_ids: Vec<String>,
    method: String,
) -> Result<Vec<ExportResultItem>> {
    let lib = library_dir(app)?;
    let agents = list_agents().await?;
    let agent_by_id: HashMap<String, &SkillAgent> = agents.iter().map(|a| (a.id.clone(), a)).collect();

    // Resolve dir_name per skill_id (status='ok' only).
    let mut dir_by_skill: HashMap<String, String> = HashMap::new();
    for sid in &skill_ids {
        if let Some(r) = sqlx::query("SELECT dir_name FROM skills WHERE id=? AND status='ok'")
            .bind(sid)
            .fetch_optional(db::pool())
            .await?
        {
            let dn: String = r.try_get("dir_name")?;
            dir_by_skill.insert(sid.clone(), dn);
        }
    }

    let mut results: Vec<ExportResultItem> = Vec::new();
    // Windows symlink items that need elevation (deferred to one batch call).
    #[cfg(windows)]
    let mut elevate: Vec<ElevatePlan> = Vec::new();

    for sid in &skill_ids {
        let Some(dir_name) = dir_by_skill.get(sid) else { continue };
        let target_lib = lib.join(dir_name); // library copy (symlink target / copy source)
        for aid in &agent_ids {
            let Some(agent) = agent_by_id.get(aid) else { continue };
            let Some(agent_path) = resolve_agent_path(&agent.skills_path) else {
                results.push(ExportResultItem { skill_id: sid.clone(), agent_id: aid.clone(), success: false, message: Some("agent path unresolvable".into()) });
                continue;
            };
            let link = agent_path.join(dir_name);
            fs::create_dir_all(&agent_path).ok();

            // pending placeholder. INSERT OR REPLACE: re-installing / switching
            // method for the same (skill, agent) replaces the existing row
            // (UNIQUE(skill_id, agent_id)) with a fresh pending row + new method.
            let row_id = Uuid::new_v4().to_string();
            if let Err(e) = sqlx::query(
                "INSERT OR REPLACE INTO skill_exports (id, skill_id, agent_id, method, status) VALUES (?, ?, ?, ?, 'pending')",
            )
            .bind(&row_id)
            .bind(sid)
            .bind(aid)
            .bind(&method)
            .execute(db::pool())
            .await
            {
                results.push(ExportResultItem { skill_id: sid.clone(), agent_id: aid.clone(), success: false, message: Some(format!("db: {}", e)) });
                continue;
            }

            // delete-then-build: remove any existing install first
            let _ = remove_link(&link);

            let outcome: CreateOutcome = if method == "copy" {
                match copy_dir_recursive(&target_lib, &link) {
                    Ok(()) => CreateOutcome::Success,
                    Err(e) => CreateOutcome::Failed(format!("copy: {}", e)),
                }
            } else if method == "symlink" {
                #[cfg(not(windows))]
                {
                    match std::os::unix::fs::symlink(&target_lib, &link) {
                        Ok(()) => CreateOutcome::Success,
                        Err(e) => CreateOutcome::Failed(format!("symlink: {}", e)),
                    }
                }
                #[cfg(windows)]
                {
                    match std::os::windows::fs::symlink_dir(&target_lib, &link) {
                        Ok(()) => CreateOutcome::Success,
                        Err(e) if is_privilege_error(&e) => {
                            elevate.push(ElevatePlan {
                                skill_id: sid.clone(),
                                agent_id: aid.clone(),
                                row_id: row_id.clone(),
                                link: link.clone(),
                                target: target_lib.clone(),
                            });
                            CreateOutcome::Deferred
                        }
                        Err(e) => CreateOutcome::Failed(format!("symlink: {}", e)),
                    }
                }
            } else {
                CreateOutcome::Failed("unknown method".into())
            };

            match outcome {
                CreateOutcome::Success => {
                    let _ = sqlx::query("UPDATE skill_exports SET status='ok' WHERE id=?")
                        .bind(&row_id)
                        .execute(db::pool())
                        .await;
                    results.push(ExportResultItem { skill_id: sid.clone(), agent_id: aid.clone(), success: true, message: None });
                }
                CreateOutcome::Failed(msg) => {
                    let _ = sqlx::query("DELETE FROM skill_exports WHERE id=?")
                        .bind(&row_id)
                        .execute(db::pool())
                        .await;
                    results.push(ExportResultItem { skill_id: sid.clone(), agent_id: aid.clone(), success: false, message: Some(msg) });
                }
                #[cfg(windows)]
                CreateOutcome::Deferred => {
                    // Left pending; the elevation batch below finalizes status.
                }
            }
        }
    }

    // Run the Windows elevation batch (one UAC) for deferred symlinks.
    #[cfg(windows)]
    {
        if !elevate.is_empty() {
            let items: Vec<(PathBuf, PathBuf)> = elevate.iter().map(|p| (p.link.clone(), p.target.clone())).collect();
            let batch_results = create_symlinks_elevated(&items);
            for (plan, res) in elevate.into_iter().zip(batch_results.into_iter()) {
                if res.success {
                    let _ = sqlx::query("UPDATE skill_exports SET status='ok' WHERE id=?")
                        .bind(&plan.row_id)
                        .execute(db::pool())
                        .await;
                    results.push(ExportResultItem { skill_id: plan.skill_id, agent_id: plan.agent_id, success: true, message: None });
                } else {
                    let _ = sqlx::query("DELETE FROM skill_exports WHERE id=?")
                        .bind(&plan.row_id)
                        .execute(db::pool())
                        .await;
                    results.push(ExportResultItem { skill_id: plan.skill_id, agent_id: plan.agent_id, success: false, message: res.message });
                }
            }
        }
    }

    Ok(results)
}

#[cfg(windows)]
struct ElevatePlan {
    skill_id: String,
    agent_id: String,
    row_id: String,
    link: PathBuf,
    target: PathBuf,
}

/// Outcome of creating an install for one (skill, agent). `Deferred` (Windows
/// only) means the symlink needs elevation and was pushed to the batch; the
/// caller leaves status='pending' and the elevation run finalizes it.
enum CreateOutcome {
    Success,
    Failed(String),
    #[cfg(windows)]
    Deferred,
}

// ── 2.4 (Windows): on-demand elevation via self-relaunch ─────────────────────
//
// On Windows, creating a symlink needs SeCreateSymbolicLinkPrivilege. The main
// app runs unprivileged; when `symlink_dir` fails with a privilege error, the
// deferred items are written to a manifest and the app relaunches ITSELF
// elevated (ShellExecuteExW "runas") with `--symlink-helper --manifest <tmp>`.
// The elevated copy creates the symlinks, writes results, exits. One UAC per
// batch. The main app never stays elevated, and MCP child processes inherit no
// admin rights. See `doc/agent_20260724.md` §3.8.5.

#[cfg(windows)]
pub fn run_helper_mode() -> ! {
    let args: Vec<String> = std::env::args().collect();
    let mut manifest_path: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--manifest" && i + 1 < args.len() {
            manifest_path = Some(args[i + 1].clone());
            i += 2;
        } else {
            i += 1;
        }
    }
    let results = match manifest_path.as_deref() {
        Some(mp) => run_helper_inner(mp),
        None => ManifestResults { results: vec![] },
    };
    let results_path = format!("{}.results.json", manifest_path.unwrap_or_default());
    let _ = std::fs::write(&results_path, serde_json::to_string(&results).unwrap_or_default());
    std::process::exit(0);
}

#[cfg(windows)]
fn run_helper_inner(manifest_path: &str) -> ManifestResults {
    let content = match std::fs::read_to_string(manifest_path) {
        Ok(c) => c,
        Err(_) => return ManifestResults { results: vec![] },
    };
    let manifest: Manifest = serde_json::from_str(&content).unwrap_or(Manifest { items: vec![] });
    let mut results = Vec::with_capacity(manifest.items.len());
    for item in &manifest.items {
        let link = PathBuf::from(&item.link);
        let target = PathBuf::from(&item.target);
        // delete-then-build (the elevated copy has privilege to remove + create)
        let _ = remove_link(&link);
        let r = match std::os::windows::fs::symlink_dir(&target, &link) {
            Ok(()) => ManifestResultItem { link: item.link.clone(), success: true, message: None },
            Err(e) => ManifestResultItem { link: item.link.clone(), success: false, message: Some(e.to_string()) },
        };
        results.push(r);
    }
    ManifestResults { results }
}

#[cfg(windows)]
pub fn create_symlinks_elevated(items: &[(PathBuf, PathBuf)]) -> Vec<ManifestResultItem> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, GetLastError};
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    use windows::Win32::UI::Shell::{ShellExecuteExW, SHELLEXECUTEINFOW, SEE_MASK_NOCLOSEPROCESS};
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

    fn to_wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }

    let fail_all = |msg: &str| -> Vec<ManifestResultItem> {
        items
            .iter()
            .map(|(l, _)| ManifestResultItem {
                link: l.to_string_lossy().to_string(),
                success: false,
                message: Some(msg.to_string()),
            })
            .collect()
    };

    let manifest = Manifest {
        items: items
            .iter()
            .map(|(l, t)| ManifestItem { link: l.to_string_lossy().to_string(), target: t.to_string_lossy().to_string() })
            .collect(),
    };
    let manifest_path = std::env::temp_dir().join(format!("mcphub-symlink-{}.json", Uuid::new_v4()));
    let results_path = std::path::PathBuf::from(format!("{}.results.json", manifest_path.to_string_lossy()));
    if std::fs::write(&manifest_path, serde_json::to_string(&manifest).unwrap_or_default()).is_err() {
        return fail_all("manifest write failed");
    }

    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(_) => {
            let _ = std::fs::remove_file(&manifest_path);
            return fail_all("current_exe failed");
        }
    };
    let params = format!("--symlink-helper --manifest \"{}\"", manifest_path.to_string_lossy());

    let verb = to_wide("runas");
    let exe_w = to_wide(&exe.to_string_lossy());
    let params_w = to_wide(&params);

    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = PCWSTR(verb.as_ptr());
    info.lpFile = PCWSTR(exe_w.as_ptr());
    info.lpParameters = PCWSTR(params_w.as_ptr());
    info.nShow = SW_HIDE.0 as u32;

    let launched = unsafe { ShellExecuteExW(&mut info) };
    if launched.is_err() {
        let err = unsafe { GetLastError().0 };
        let _ = std::fs::remove_file(&manifest_path);
        return fail_all(&format!(
            "提权被拒绝或失败 (WinError {}): 请改选文件拷贝",
            err
        ));
    }

    // Wait for the elevated copy to finish.
    if !info.hProcess.is_invalid() {
        unsafe {
            let _ = WaitForSingleObject(info.hProcess, INFINITE);
            let mut code: u32 = 0;
            let _ = GetExitCodeProcess(info.hProcess, &mut code);
            let _ = CloseHandle(info.hProcess);
        }
    }

    // Read per-item results written by the elevated copy.
    let results: Vec<ManifestResultItem> = match std::fs::read_to_string(&results_path) {
        Ok(c) => serde_json::from_str::<ManifestResults>(&c)
            .map(|r| r.results)
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    };

    // Map results back to the input items (by link path).
    let mapped: Vec<ManifestResultItem> = items
        .iter()
        .map(|(l, _)| {
            let key = l.to_string_lossy().to_string();
            results
                .iter()
                .find(|r| r.link == key)
                .cloned()
                .unwrap_or(ManifestResultItem { link: key, success: false, message: Some("no result from helper".into()) })
        })
        .collect();

    let _ = std::fs::remove_file(&manifest_path);
    let _ = std::fs::remove_file(&results_path);
    mapped
}

// ── 2.5: uninstall + delete (cleanup) ────────────────────────────────────────

/// Uninstall a skill from a single agent: remove the install at the agent's
/// path (symlink or copy) and delete the skill_exports row. The source-agent
/// record is treated like any other install (deletable) — the user can
/// remove the skill from the source agent too. Removing a link needs no
/// privilege (user-writable agent dir).
pub async fn uninstall_skill(skill_id: &str, agent_id: &str) -> Result<bool> {
    let skill_row = sqlx::query("SELECT dir_name FROM skills WHERE id=?")
        .bind(skill_id)
        .fetch_optional(db::pool())
        .await?;
    let Some(sr) = skill_row else { return Ok(false); };
    let dir_name: String = sr.try_get("dir_name")?;

    let agents = list_agents().await?;
    if let Some(agent) = agents.iter().find(|a| a.id == agent_id) {
        if let Some(ap) = resolve_agent_path(&agent.skills_path) {
            let _ = remove_link(&ap.join(&dir_name));
        }
    }

    let affected = sqlx::query("DELETE FROM skill_exports WHERE skill_id=? AND agent_id=?")
        .bind(skill_id)
        .bind(agent_id)
        .execute(db::pool())
        .await?
        .rows_affected();
    Ok(affected > 0)
}

/// Delete a skill from the library. Symlink exports are removed mandatorily
/// (dangling symlinks would point at the deleted library); copy exports are
/// removed only when their agent_id is in `cleanup_agent_ids` (the import-
/// source record — agent_id == source_agent — is NOT a real copy and is never
/// file-cleaned; it just cascade-deletes with the skill row). The library copy
/// is always removed. Errors if the skill isn't found or the delete matched 0
/// rows (so the frontend can't mistake a no-op for success).
pub async fn delete_skill(app: &AppHandle, id: &str, cleanup_agent_ids: Vec<String>) -> Result<()> {
    let row = sqlx::query("SELECT dir_name FROM skills WHERE id=?")
        .bind(id)
        .fetch_optional(db::pool())
        .await?;
    let Some(r) = row else {
        return Err(anyhow::anyhow!("skill not found: {}", id));
    };
    let dir_name: String = r.try_get("dir_name")?;

    let agents = list_agents().await?;
    let agent_path: HashMap<String, Option<PathBuf>> =
        agents.iter().map(|a| (a.id.clone(), resolve_agent_path(&a.skills_path))).collect();

    // Clean up exported installs (including the source-agent record — it's
    // deletable like any other install).
    let exports = sqlx::query("SELECT agent_id, method FROM skill_exports WHERE skill_id=?")
        .bind(id)
        .fetch_all(db::pool())
        .await?;
    for r in &exports {
        let agent_id: String = r.try_get("agent_id")?;
        let method: String = r.try_get("method")?;
        // symlink: mandatory; copy: only when chosen for cleanup.
        let clean = method == "symlink" || cleanup_agent_ids.iter().any(|a| a == &agent_id);
        if clean {
            if let Some(Some(ap)) = agent_path.get(&agent_id) {
                let _ = remove_link(&ap.join(&dir_name));
            }
        }
    }

    // Remove the library copy (real dir → remove_dir_all via remove_link).
    let lib = library_dir(app)?;
    let _ = remove_link(&lib.join(&dir_name));

    // Delete the skill row AND its skill_exports rows in one transaction
    // (no foreign-key cascade — done explicitly in code for flexibility).
    let mut tx = db::pool().begin().await?;
    let _export_rows = sqlx::query("DELETE FROM skill_exports WHERE skill_id=?")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    let affected = sqlx::query("DELETE FROM skills WHERE id=?")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    if affected == 0 {
        return Err(anyhow::anyhow!("skill not deleted (0 rows affected): {}", id));
    }
    Ok(())
}

// ── 2.6: system integration (open folder / pick folder) ─────────────────────
//
// 2.6 uses the dialog plugin (pick_folder) directly from Rust, and a raw
// platform process for "reveal folder" (the shell plugin's `open` is
// deprecated in favor of tauri-plugin-opener, which isn't a dep; spawning the
// OS opener is simpler and adds no plugin). Capability permissions gate the JS
// plugin API, not Rust-side calls, so no capability change is needed.

/// Reveal a path (e.g. an agent's skillsPath) in the OS file manager. `~` is
/// expanded via resolve_agent_path. Errors if the path can't be resolved or
/// doesn't exist.
pub async fn open_path_in_explorer(path: &str) -> Result<()> {
    let p = resolve_agent_path(path).ok_or_else(|| anyhow::anyhow!("path unresolvable: {}", path))?;
    if !p.exists() {
        return Err(anyhow::anyhow!("path does not exist: {}", p.display()));
    }
    spawn_file_manager(&p).map_err(|e| anyhow::anyhow!("open failed: {}", e))?;
    Ok(())
}

/// Open a skill's library folder (`<library_dir>/<dir_name>`) in the OS file
/// manager. Used by the main list row "open folder" button.
pub async fn open_skill_library(app: &AppHandle, id: &str) -> Result<()> {
    let row = sqlx::query("SELECT dir_name FROM skills WHERE id=?")
        .bind(id)
        .fetch_optional(db::pool())
        .await?;
    let Some(r) = row else {
        return Err(anyhow::anyhow!("skill not found: {}", id));
    };
    let dir_name: String = r.try_get("dir_name")?;
    let p = library_dir(app)?.join(&dir_name);
    if !p.exists() {
        return Err(anyhow::anyhow!("library copy not found: {}", p.display()));
    }
    spawn_file_manager(&p).map_err(|e| anyhow::anyhow!("open failed: {}", e))?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn spawn_file_manager(p: &Path) -> std::io::Result<()> {
    std::process::Command::new("open").arg(p).spawn()?.wait()?;
    Ok(())
}
#[cfg(target_os = "windows")]
fn spawn_file_manager(p: &Path) -> std::io::Result<()> {
    // explorer returns immediately; don't wait (odd exit codes otherwise).
    std::process::Command::new("explorer").arg(p).spawn()?;
    Ok(())
}
#[cfg(all(unix, not(target_os = "macos")))]
fn spawn_file_manager(p: &Path) -> std::io::Result<()> {
    std::process::Command::new("xdg-open").arg(p).spawn()?;
    Ok(())
}

/// Open the OS folder picker and return the chosen absolute path, or None if
/// cancelled. Blocking (runs in the command's async context).
pub async fn pick_directory(app: &AppHandle) -> Result<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let folder = app
        .dialog()
        .file()
        .set_title("Select skills folder")
        .blocking_pick_folder();
    Ok(match folder {
        Some(fp) => Some(fp.into_path().map_err(|e| anyhow::anyhow!("{}", e))?.to_string_lossy().into_owned()),
        None => None,
    })
}

