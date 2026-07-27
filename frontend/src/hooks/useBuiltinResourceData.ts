// Thin wrapper over the shared BuiltinDataContext so ResourcesPage keeps its
// existing call site shape (resources/loading/error/triggerRefresh/CRUD).
// The shared store keeps the Sidebar badge in sync with page mutations.
import { useBuiltinData } from '@/contexts/BuiltinDataContext';

export const useBuiltinResourceData = () => {
  const {
    resources,
    resourcesLoading,
    resourcesError,
    setResourcesError,
    refreshResources,
    addResource,
    editResource,
    removeResource,
  } = useBuiltinData();

  return {
    resources,
    loading: resourcesLoading,
    error: resourcesError,
    setError: setResourcesError,
    triggerRefresh: refreshResources,
    addResource,
    editResource,
    removeResource,
  };
};
