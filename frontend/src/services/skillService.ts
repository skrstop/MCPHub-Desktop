import {
  Skill,
  SkillAgent,
  ScannedSkill,
  ExportResultItem,
  ApiResponse,
} from '@/types';
import { apiGet, apiPost, apiPut } from '../utils/fetchInterceptor';

/**
 * Get the configured AI agents and their skills install paths.
 */
export const listSkillAgents = async (): Promise<SkillAgent[]> => {
  const response: ApiResponse<SkillAgent[]> = await apiGet('/skills/agents');
  if (!response.success) {
    throw new Error(response.message || 'Failed to fetch skill agents');
  }
  return response.data || [];
};

/**
 * Persist the full list of agents (add/edit/delete).
 */
export const saveSkillAgents = async (agents: SkillAgent[]): Promise<void> => {
  const response: ApiResponse = await apiPut('/skills/agents', agents);
  if (!response.success) {
    throw new Error(response.message || 'Failed to save skill agents');
  }
};

/** Alias used by the settings card's per-section save helper. */
export const updateSkillsAgents = saveSkillAgents;

/** Create a new custom (user-added) agent. Refuses built-in names. */
export const createSkillAgent = async (
  name: string,
  skillsPath: string,
): Promise<SkillAgent> => {
  const response: ApiResponse<SkillAgent> = await apiPost('/skills/agents/create', { name, skillsPath });
  if (!response.success) {
    throw new Error(response.message || 'Failed to create agent');
  }
  return response.data!;
};

/** Delete a custom agent by id. Refuses to delete built-in agents. */
export const deleteSkillAgent = async (id: string): Promise<void> => {
  const response: ApiResponse = await apiPost('/skills/agents/delete', { id });
  if (!response.success) {
    throw new Error(response.message || 'Failed to delete agent');
  }
};

/**
 * Scan all configured agent skills paths for importable skills.
 * Symlinks/shortcuts are skipped server-side.
 */
export const scanSkillsForImport = async (): Promise<ScannedSkill[]> => {
  const response: ApiResponse<ScannedSkill[]> = await apiGet('/skills/scan');
  if (!response.success) {
    throw new Error(response.message || 'Failed to scan skills');
  }
  return response.data || [];
};

/**
 * List all skills in the app's managed library.
 */
export const listSkills = async (): Promise<Skill[]> => {
  const response: ApiResponse<Skill[]> = await apiGet('/skills');
  if (!response.success) {
    throw new Error(response.message || 'Failed to fetch skills');
  }
  return response.data || [];
};

/**
 * Get a single skill (with its exports) by id.
 */
export const getSkill = async (id: string): Promise<Skill> => {
  const response: ApiResponse<Skill> = await apiGet(`/skills/${encodeURIComponent(id)}`);
  if (!response.success) {
    throw new Error(response.message || 'Failed to fetch skill');
  }
  return response.data!;
};

/**
 * Import selected skills into the app library. Agent-grouped items set `agentId`
 * (source = <agent.skillsPath>/<dirName>, a source-agent record is written);
 * manual items set `path` (source = that folder, no source-agent record).
 */
export const importSkills = async (
  items: Array<{ agentId: string; dirName: string; path?: string }>,
): Promise<{ successCount: number; failureCount: number; results: ExportResultItem[] }> => {
  const response: ApiResponse<{ successCount: number; failureCount: number; results: ExportResultItem[] }> =
    await apiPost('/skills/import', { items });
  if (!response.success) {
    throw new Error(response.message || 'Failed to import skills');
  }
  return response.data || { successCount: 0, failureCount: 0, results: [] };
};

/**
 * Scan a manually-selected folder for skills (max 2-layer SKILL.md detection).
 * Returns skills with agentId="__manual__" (no source agent).
 */
export const scanFolderForSkills = async (path: string): Promise<ScannedSkill[]> => {
  const response: ApiResponse<ScannedSkill[]> = await apiPost('/skills/scan-folder', { path });
  if (!response.success) {
    throw new Error(response.message || 'Failed to scan folder');
  }
  return response.data || [];
};

/**
 * Export the given skills to the given agents using symlink or copy.
 */
export const exportSkills = async (
  skillIds: string[],
  agentIds: string[],
  method: 'symlink' | 'copy',
): Promise<ExportResultItem[]> => {
  const response: ApiResponse<ExportResultItem[]> = await apiPost('/skills/export', {
    skillIds,
    agentIds,
    method,
  });
  if (!response.success) {
    throw new Error(response.message || 'Failed to export skills');
  }
  return response.data || [];
};

/**
 * Reveal an agent's skills path in the OS file manager. The path may start
 * with `~` (expanded server-side in Phase 2).
 */
export const openAgentPath = async (path: string): Promise<void> => {
  const response: ApiResponse = await apiPost('/skills/open-path', { path });
  if (!response.success) {
    throw new Error(response.message || 'Failed to open path');
  }
};

/**
 * Open a skill's library folder (the managed copy) in the OS file manager.
 */
export const openSkillLibrary = async (id: string): Promise<void> => {
  const response: ApiResponse = await apiPost('/skills/open-library', { id });
  if (!response.success) {
    throw new Error(response.message || 'Failed to open skill folder');
  }
};

/**
 * Open the OS folder picker and return the chosen absolute path, or null if
 * the user cancelled. Phase 1 returns a mock path.
 */
export const pickDirectory = async (): Promise<string | null> => {
  // The command returns Option<String> → data is the path string directly
  // (or absent when cancelled).
  const response: ApiResponse<string | null> = await apiPost('/skills/pick-directory', {});
  if (!response.success) {
    throw new Error(response.message || 'Failed to pick directory');
  }
  return response.data ?? null;
};

/**
 * Uninstall a skill from a single agent: remove the symlink/file copy at the
 * agent's skills path and delete the skill_exports record.
 */
export const uninstallSkill = async (skillId: string, agentId: string): Promise<void> => {
  const response: ApiResponse = await apiPost('/skills/uninstall', { skillId, agentId });
  if (!response.success) {
    throw new Error(response.message || 'Failed to uninstall skill');
  }
};

/**
 * Remove a skill from the library and optionally clean up its exported
 * copies/symlinks at the given agent paths. Symlink exports are always
 * removed (mandatory); copy exports are only removed when their agentId is
 * included in cleanupAgentIds.
 */
export const deleteSkill = async (id: string, cleanupAgentIds: string[] = []): Promise<void> => {
  const response: ApiResponse = await apiPost('/skills/delete', { id, cleanupAgentIds });
  if (!response.success) {
    throw new Error(response.message || 'Failed to delete skill');
  }
};
