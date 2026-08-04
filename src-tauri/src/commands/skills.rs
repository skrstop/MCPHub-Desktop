//! Tauri commands for the Skills feature.
//!
//! - 2.2: `list_skill_agents` / `save_skill_agents`
//! - 2.3: `scan_skills_for_import` / `list_skills` / `get_skill` / `import_skills`
//! - 2.4–2.6 commands added later; see `doc/agent_20260724.md` §3.8.

use crate::{
    models::skill::{ExportResultItem, ImportItem, ImportSummary, Skill, SkillAgent, ScannedSkill},
    services::skill_service,
};
use tauri::AppHandle;
/// List configured AI agents and their skills install paths.
#[tauri::command]
pub async fn list_skill_agents() -> Result<Vec<SkillAgent>, String> {
    skill_service::list_agents().await.map_err(|e| e.to_string())
}

/// Persist the full agent list (add/edit/delete).
#[tauri::command]
pub async fn save_skill_agents(agents: Vec<SkillAgent>) -> Result<(), String> {
    skill_service::save_agents(agents).await.map_err(|e| e.to_string())
}

/// Create a new custom (user-added) agent. Refuses built-in names.
#[tauri::command]
pub async fn create_skill_agent(name: String, skills_path: String) -> Result<SkillAgent, String> {
    skill_service::create_custom_agent(&name, &skills_path)
        .await
        .map_err(|e| e.to_string())
}

/// Delete a custom agent by id. Refuses to delete built-in agents.
#[tauri::command]
pub async fn delete_skill_agent(id: String) -> Result<(), String> {
    skill_service::delete_custom_agent(&id).await.map_err(|e| e.to_string())
}

/// Scan all configured agents' skills paths for importable skills
/// (symlinks/shortcuts skipped; SKILL.md frontmatter parsed for name/desc).
#[tauri::command]
pub async fn scan_skills_for_import(app: AppHandle) -> Result<Vec<ScannedSkill>, String> {
    skill_service::scan_for_import(&app).await.map_err(|e| e.to_string())
}

/// List all skills in the library (status='ok' only, ordered by dir_name,
/// each with its status='ok' exports).
#[tauri::command]
pub async fn list_skills(app: AppHandle) -> Result<Vec<Skill>, String> {
    skill_service::list_library(&app).await.map_err(|e| e.to_string())
}

/// Get a single skill (with its exports) by id. Errors if not found or its
/// library copy is gone (frontend always calls with an id from the list).
#[tauri::command]
pub async fn get_skill(app: AppHandle, id: String) -> Result<Skill, String> {
    skill_service::get_skill(&app, &id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("skill '{}' not found", id))
}

/// Import selected skills (agentId + dirName) into the library.
/// State machine: pending insert → copy → ok on success (see skill_service).
#[tauri::command]
pub async fn import_skills(app: AppHandle, items: Vec<ImportItem>) -> Result<ImportSummary, String> {
    skill_service::import_skills(&app, items).await.map_err(|e| e.to_string())
}

/// Scan a manually-selected folder for skills (max 2-layer nesting: the folder
/// itself if it has SKILL.md, else its direct children with SKILL.md). Returns
/// skills with agent_id="__manual__" (no source agent).
#[tauri::command]
pub async fn scan_folder_for_skills(app: AppHandle, path: String) -> Result<Vec<ScannedSkill>, String> {
    skill_service::scan_folder_for_skills(&app, &path).await.map_err(|e| e.to_string())
}

/// Export/install skills to agents (symlink or copy). Idempotent rebuild:
/// always delete-then-build per (skill, agent). On Windows symlink without
/// privilege, items are batched through one elevated self-relaunch (one UAC).
#[tauri::command]
pub async fn export_skills_to_agents(
    app: AppHandle,
    skill_ids: Vec<String>,
    agent_ids: Vec<String>,
    method: String,
) -> Result<Vec<ExportResultItem>, String> {
    skill_service::export_to_agents(&app, skill_ids, agent_ids, method)
        .await
        .map_err(|e| e.to_string())
}

/// Uninstall a skill from a single agent (removes the install + export row).
#[tauri::command]
pub async fn uninstall_skill(skill_id: String, agent_id: String) -> Result<bool, String> {
    skill_service::uninstall_skill(&skill_id, &agent_id)
        .await
        .map_err(|e| e.to_string())
}

/// Delete a skill from the library. Symlink exports removed mandatorily;
/// copy exports removed only for agent_ids in `cleanup_agent_ids`. Errors if
/// the skill isn't found (so a no-op can't be mistaken for success).
#[tauri::command]
pub async fn delete_skill(
    app: AppHandle,
    id: String,
    cleanup_agent_ids: Vec<String>,
) -> Result<(), String> {
    skill_service::delete_skill(&app, &id, cleanup_agent_ids)
        .await
        .map_err(|e| e.to_string())
}

/// Reveal a path in the OS file manager (expands ~; errors if missing).
#[tauri::command]
pub async fn open_path_in_explorer(path: String) -> Result<(), String> {
    skill_service::open_path_in_explorer(&path)
        .await
        .map_err(|e| e.to_string())
}

/// Open a skill's library folder in the OS file manager.
#[tauri::command]
pub async fn open_skill_library_dir(app: AppHandle, id: String) -> Result<(), String> {
    skill_service::open_skill_library(&app, &id)
        .await
        .map_err(|e| e.to_string())
}

/// Open the OS folder picker; returns the chosen absolute path or null.
#[tauri::command]
pub async fn pick_directory(app: AppHandle) -> Result<Option<String>, String> {
    skill_service::pick_directory(&app)
        .await
        .map_err(|e| e.to_string())
}
