import React, { createContext, useState, useEffect, useCallback, useContext } from 'react';
import { useTranslation } from 'react-i18next';
import { BuiltinPrompt, BuiltinResource } from '@/types';
import { useAuth } from '@/contexts/AuthContext';
import {
  getBuiltinPrompts,
  createBuiltinPrompt,
  updateBuiltinPrompt,
  deleteBuiltinPrompt,
} from '@/services/builtinPromptService';
import {
  getBuiltinResources,
  createBuiltinResource,
  updateBuiltinResource,
  deleteBuiltinResource,
} from '@/services/builtinResourceService';

// Lifts builtin prompts/resources into a single shared store so the Sidebar
// badges and the Prompts/Resources pages stay in sync without duplicate state.
// Mutations bump a refresh key that re-runs the fetch, so every consumer
// (including the Sidebar count) updates immediately after an add/edit/delete.

interface BuiltinDataContextType {
  prompts: BuiltinPrompt[];
  resources: BuiltinResource[];
  promptsLoading: boolean;
  resourcesLoading: boolean;
  promptsError: string | null;
  resourcesError: string | null;
  setPromptsError: (error: string | null) => void;
  setResourcesError: (error: string | null) => void;
  refreshPrompts: () => void;
  refreshResources: () => void;
  addPrompt: (prompt: Omit<BuiltinPrompt, 'id'>) => Promise<{ success: boolean; message?: string; data?: BuiltinPrompt }>;
  editPrompt: (id: string, prompt: Partial<BuiltinPrompt>) => Promise<{ success: boolean; message?: string; data?: BuiltinPrompt }>;
  removePrompt: (id: string) => Promise<{ success: boolean; message?: string }>;
  addResource: (resource: Omit<BuiltinResource, 'id'>) => Promise<{ success: boolean; message?: string; data?: BuiltinResource }>;
  editResource: (id: string, resource: Partial<BuiltinResource>) => Promise<{ success: boolean; message?: string; data?: BuiltinResource }>;
  removeResource: (id: string) => Promise<{ success: boolean; message?: string }>;
}

const BuiltinDataContext = createContext<BuiltinDataContextType | undefined>(undefined);

export const BuiltinDataProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const { t } = useTranslation();
  const { auth } = useAuth();

  const [prompts, setPrompts] = useState<BuiltinPrompt[]>([]);
  const [resources, setResources] = useState<BuiltinResource[]>([]);
  const [promptsLoading, setPromptsLoading] = useState(true);
  const [resourcesLoading, setResourcesLoading] = useState(true);
  const [promptsError, setPromptsError] = useState<string | null>(null);
  const [resourcesError, setResourcesError] = useState<string | null>(null);
  const [promptsRefreshKey, setPromptsRefreshKey] = useState(0);
  const [resourcesRefreshKey, setResourcesRefreshKey] = useState(0);

  const fetchPrompts = useCallback(async () => {
    try {
      setPromptsLoading(true);
      const data = await getBuiltinPrompts();
      setPrompts(data);
      setPromptsError(null);
    } catch (err) {
      console.error('Error fetching built-in prompts:', err);
      setPromptsError(err instanceof Error ? err.message : t('builtinPrompts.fetchError'));
      setPrompts([]);
    } finally {
      setPromptsLoading(false);
    }
  }, [t]);

  const fetchResources = useCallback(async () => {
    try {
      setResourcesLoading(true);
      const data = await getBuiltinResources();
      setResources(data);
      setResourcesError(null);
    } catch (err) {
      console.error('Error fetching built-in resources:', err);
      setResourcesError(err instanceof Error ? err.message : t('builtinResources.fetchError'));
      setResources([]);
    } finally {
      setResourcesLoading(false);
    }
  }, [t]);

  // Clear data + stop fetching while unauthenticated; the fetch effects gate on
  // auth.isAuthenticated so they re-run once login resolves. The Provider sits at
  // the app root, before loadUser() completes, so an ungated mount-time fetch
  // returns an empty/forbidden result (the "counts are 0" bug).
  useEffect(() => {
    if (auth.loading || auth.isAuthenticated) return;
    setPrompts([]);
    setResources([]);
    setPromptsLoading(false);
    setResourcesLoading(false);
  }, [auth.loading, auth.isAuthenticated]);

  useEffect(() => {
    if (auth.loading || !auth.isAuthenticated) return;
    fetchPrompts();
  }, [fetchPrompts, promptsRefreshKey, auth.isAuthenticated, auth.loading]);

  useEffect(() => {
    if (auth.loading || !auth.isAuthenticated) return;
    fetchResources();
  }, [fetchResources, resourcesRefreshKey, auth.isAuthenticated, auth.loading]);

  const refreshPrompts = useCallback(() => {
    setPromptsRefreshKey((prev) => prev + 1);
  }, []);

  const refreshResources = useCallback(() => {
    setResourcesRefreshKey((prev) => prev + 1);
  }, []);

  const addPrompt = useCallback(
    async (prompt: Omit<BuiltinPrompt, 'id'>) => {
      try {
        const result = await createBuiltinPrompt(prompt);
        refreshPrompts();
        return { success: true, data: result };
      } catch (err) {
        const message = err instanceof Error ? err.message : t('builtinPrompts.createError');
        setPromptsError(message);
        return { success: false, message };
      }
    },
    [t, refreshPrompts],
  );

  const editPrompt = useCallback(
    async (id: string, prompt: Partial<BuiltinPrompt>) => {
      try {
        const result = await updateBuiltinPrompt(id, prompt);
        refreshPrompts();
        return { success: true, data: result };
      } catch (err) {
        const message = err instanceof Error ? err.message : t('builtinPrompts.updateError');
        setPromptsError(message);
        return { success: false, message };
      }
    },
    [t, refreshPrompts],
  );

  const removePrompt = useCallback(
    async (id: string) => {
      try {
        await deleteBuiltinPrompt(id);
        refreshPrompts();
        return { success: true };
      } catch (err) {
        const message = err instanceof Error ? err.message : t('builtinPrompts.deleteError');
        setPromptsError(message);
        return { success: false, message };
      }
    },
    [t, refreshPrompts],
  );

  const addResource = useCallback(
    async (resource: Omit<BuiltinResource, 'id'>) => {
      try {
        const result = await createBuiltinResource(resource);
        refreshResources();
        return { success: true, data: result };
      } catch (err) {
        const message = err instanceof Error ? err.message : t('builtinResources.createError');
        setResourcesError(message);
        return { success: false, message };
      }
    },
    [t, refreshResources],
  );

  const editResource = useCallback(
    async (id: string, resource: Partial<BuiltinResource>) => {
      try {
        const result = await updateBuiltinResource(id, resource);
        refreshResources();
        return { success: true, data: result };
      } catch (err) {
        const message = err instanceof Error ? err.message : t('builtinResources.updateError');
        setResourcesError(message);
        return { success: false, message };
      }
    },
    [t, refreshResources],
  );

  const removeResource = useCallback(
    async (id: string) => {
      try {
        await deleteBuiltinResource(id);
        refreshResources();
        return { success: true };
      } catch (err) {
        const message = err instanceof Error ? err.message : t('builtinResources.deleteError');
        setResourcesError(message);
        return { success: false, message };
      }
    },
    [t, refreshResources],
  );

  const value: BuiltinDataContextType = {
    prompts,
    resources,
    promptsLoading,
    resourcesLoading,
    promptsError,
    resourcesError,
    setPromptsError,
    setResourcesError,
    refreshPrompts,
    refreshResources,
    addPrompt,
    editPrompt,
    removePrompt,
    addResource,
    editResource,
    removeResource,
  };

  return <BuiltinDataContext.Provider value={value}>{children}</BuiltinDataContext.Provider>;
};

export const useBuiltinData = () => {
  const context = useContext(BuiltinDataContext);
  if (context === undefined) {
    throw new Error('useBuiltinData must be used within a BuiltinDataProvider');
  }
  return context;
};
