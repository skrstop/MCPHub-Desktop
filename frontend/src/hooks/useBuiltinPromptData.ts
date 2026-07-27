// Thin wrapper over the shared BuiltinDataContext so PromptsPage keeps its
// existing call site shape (prompts/loading/error/triggerRefresh/CRUD).
// The shared store keeps the Sidebar badge in sync with page mutations.
import { useBuiltinData } from '@/contexts/BuiltinDataContext';

export const useBuiltinPromptData = () => {
  const {
    prompts,
    promptsLoading,
    promptsError,
    setPromptsError,
    refreshPrompts,
    addPrompt,
    editPrompt,
    removePrompt,
  } = useBuiltinData();

  return {
    prompts,
    loading: promptsLoading,
    error: promptsError,
    setError: setPromptsError,
    triggerRefresh: refreshPrompts,
    addPrompt,
    editPrompt,
    removePrompt,
  };
};
