use serde::{Deserialize, Serialize};

/// A configured AI agent and its skills install path. Stored in
/// `system_config.config_json.skills.agents`. `custom=false` marks agents that
/// come from the bundled `install.json` catalog (built-in, read-only in the
/// agent-management UI); `custom=true` marks user-added agents (creatable /
/// deletable). `custom` is recomputed from the catalog on every `list_agents`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillAgent {
    pub id: String,
    pub name: String,
    pub skills_path: String,
    #[serde(default)]
    pub custom: bool,
}

/// A skill directory discovered under an agent's skillsPath during import
/// scanning. name/description come from the skill's SKILL.md frontmatter.
/// `is_symlink` marks soft-referenced skills (the entry is a symlink) which
/// are shown but not importable. `already_imported` is a filesystem check
/// (the library already has this dir_name), NOT a DB lookup.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScannedSkill {
    pub agent_id: String,
    pub agent_name: String,
    pub dir_name: String,
    pub name: String,
    pub description: String,
    pub path: String,
    pub is_symlink: bool,
    pub already_imported: bool,
}

/// A single export of a library skill to an agent, with the method used.
/// (status is DB-internal; only `ok` rows are surfaced to the frontend.)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillExport {
    pub agent_id: String,
    pub agent_name: String,
    pub method: String, // "symlink" | "copy"
    pub created_at: Option<String>,
}

/// A skill stored in the app's managed skills library. `exports` lists the
/// agents this skill has been installed to (status='ok' only).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub id: String,
    pub dir_name: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub source_agent: Option<String>,
    pub source_path: Option<String>,
    pub created_at: Option<String>,
    pub exports: Vec<SkillExport>,
}

/// Per (skill, agent) result of an export/install operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResultItem {
    pub skill_id: String,
    pub agent_id: String,
    pub success: bool,
    pub message: Option<String>,
}

/// Per-item result of an import operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResultItem {
    pub dir_name: String,
    pub success: bool,
    pub message: Option<String>,
}

/// Summary of an import operation (counts + per-item results).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub success_count: usize,
    pub failure_count: usize,
    pub results: Vec<ImportResultItem>,
}

/// Payload item for `import_skills`. Agent-grouped imports set `agent_id`
/// (source = <agent.skillsPath>/<dirName>, a source-agent record is written).
/// Manual imports set `path` (source = that folder, NO source-agent record,
/// agent_id ignored).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportItem {
    pub agent_id: String,
    pub dir_name: String,
    pub path: Option<String>,
}
