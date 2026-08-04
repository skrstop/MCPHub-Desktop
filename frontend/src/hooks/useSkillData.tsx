import React, { createContext, useContext, useState, useEffect, useCallback } from 'react';
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
 * Skills library store, shared app-wide via context so the sidebar badge and
 * the Skills page stay in sync (e.g. import/delete updates both). Mounted
 * once at the app root (see SkillDataProvider in App.tsx).
 */
type SkillDataValue = ReturnType<typeof useSkillDataState>;
const SkillDataContext = createContext<SkillDataValue | undefined>(undefined);

const useSkillDataState = () => {
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

/** Provider that mounts the shared skills store once at the app root. */
export const SkillDataProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const value = useSkillDataState();
  return <SkillDataContext.Provider value={value}>{children}</SkillDataContext.Provider>;
};

/** Consume the shared skills store. Must be used inside <SkillDataProvider>. */
export const useSkillData = () => {
  const ctx = useContext(SkillDataContext);
  if (!ctx) throw new Error('useSkillData must be used within a SkillDataProvider');
  return ctx;
};
