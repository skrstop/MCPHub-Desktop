import { useState, useEffect, useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { Skill, ExportResultItem } from '@/types';
import { useAuth } from '@/contexts/AuthContext';
import {
  listSkills,
  importSkills as importSkillsApi,
  exportSkills as exportSkillsApi,
  deleteSkill as deleteSkillApi,
  uninstallSkill as uninstallSkillApi,
} from '@/services/skillService';

/**
 * Self-managed store for the Skills library page. Kept independent of
 * BuiltinDataContext on purpose — skills are a separate concern and the
 * sidebar badge is left empty.
 */
export const useSkillData = () => {
  const { t } = useTranslation();
  const { auth } = useAuth();

  const [skills, setSkills] = useState<Skill[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);

  const fetchSkills = useCallback(async () => {
    try {
      setLoading(true);
      const data = await listSkills();
      setSkills(data);
      setError(null);
    } catch (err) {
      console.error('Error fetching skills:', err);
      setError(err instanceof Error ? err.message : t('skills.fetchError'));
      setSkills([]);
    } finally {
      setLoading(false);
    }
  }, [t]);

  // Clear data + stop fetching while unauthenticated.
  useEffect(() => {
    if (auth.loading || auth.isAuthenticated) return;
    setSkills([]);
    setLoading(false);
  }, [auth.loading, auth.isAuthenticated]);

  useEffect(() => {
    if (auth.loading || !auth.isAuthenticated) return;
    fetchSkills();
  }, [fetchSkills, refreshKey, auth.isAuthenticated, auth.loading]);

  const refresh = useCallback(() => setRefreshKey((k) => k + 1), []);

  const importSkills = useCallback(
    async (items: Array<{ agentId: string; dirName: string; path?: string }>) => {
      try {
        const result = await importSkillsApi(items);
        refresh();
        return { success: true, data: result };
      } catch (err) {
        const message = err instanceof Error ? err.message : t('skills.importError');
        setError(message);
        return { success: false, message };
      }
    },
    [t, refresh],
  );

  const exportSkills = useCallback(
    async (skillIds: string[], agentIds: string[], method: 'symlink' | 'copy') => {
      try {
        const results: ExportResultItem[] = await exportSkillsApi(skillIds, agentIds, method);
        refresh();
        return { success: true, data: results };
      } catch (err) {
        const message = err instanceof Error ? err.message : t('skills.exportError');
        setError(message);
        return { success: false, message };
      }
    },
    [t, refresh],
  );

  const removeSkill = useCallback(
    async (id: string, cleanupAgentIds: string[] = []) => {
      try {
        await deleteSkillApi(id, cleanupAgentIds);
        refresh();
        return { success: true };
      } catch (err) {
        const message = err instanceof Error ? err.message : t('skills.deleteError');
        setError(message);
        return { success: false, message };
      }
    },
    [t, refresh],
  );

  const uninstallSkill = useCallback(
    async (skillId: string, agentId: string) => {
      try {
        await uninstallSkillApi(skillId, agentId);
        refresh();
        return { success: true };
      } catch (err) {
        const message = err instanceof Error ? err.message : t('skills.uninstallError');
        setError(message);
        return { success: false, message };
      }
    },
    [t, refresh],
  );

  return {
    skills,
    loading,
    error,
    setError,
    triggerRefresh: refresh,
    importSkills,
    exportSkills,
    removeSkill,
    uninstallSkill,
  };
};
