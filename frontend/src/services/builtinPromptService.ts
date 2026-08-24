import { BuiltinPrompt, ApiResponse, PromptPage } from '@/types';
import { apiGet, apiPost, apiPut, apiDelete } from '../utils/fetchInterceptor';

/**
 * Get all built-in prompts
 */
export const getBuiltinPrompts = async (): Promise<BuiltinPrompt[]> => {
  const response: ApiResponse<BuiltinPrompt[]> = await apiGet('/prompts');
  if (!response.success) {
    throw new Error(response.message || 'Failed to fetch built-in prompts');
  }
  return response.data || [];
};

/**
 * Get a single built-in prompt by ID
 */
export const getBuiltinPromptById = async (id: string): Promise<BuiltinPrompt> => {
  const response: ApiResponse<BuiltinPrompt> = await apiGet(`/prompts/${id}`);
  if (!response.success) {
    throw new Error(response.message || 'Failed to fetch built-in prompt');
  }
  return response.data!;
};

/**
 * Create a new built-in prompt
 */
export const createBuiltinPrompt = async (
  prompt: Omit<BuiltinPrompt, 'id'>,
): Promise<BuiltinPrompt> => {
  const response: ApiResponse<BuiltinPrompt> = await apiPost('/prompts', prompt);
  if (!response.success) {
    throw new Error(response.message || 'Failed to create built-in prompt');
  }
  return response.data!;
};

/**
 * Update an existing built-in prompt
 */
export const updateBuiltinPrompt = async (
  id: string,
  prompt: Partial<BuiltinPrompt>,
): Promise<BuiltinPrompt> => {
  const response: ApiResponse<BuiltinPrompt> = await apiPut(`/prompts/${id}`, prompt);
  if (!response.success) {
    throw new Error(response.message || 'Failed to update built-in prompt');
  }
  return response.data!;
};

/**
 * Delete a built-in prompt
 */
export const deleteBuiltinPrompt = async (id: string): Promise<void> => {
  const response: ApiResponse = await apiDelete(`/prompts/${id}`);
  if (!response.success) {
    throw new Error(response.message || 'Failed to delete built-in prompt');
  }
};

/**
 * Paginated builtin-prompt search (SQL-level LIKE + LIMIT/OFFSET). `page` is
 * 0-based. `filter`: 'all' | 'active' | 'inactive'.
 */
export const searchBuiltinPrompts = async (
  searchKey: string,
  filter: string,
  page: number,
  pageSize: number,
): Promise<PromptPage> => {
  const response: ApiResponse<PromptPage> = await apiPost('/prompts/search', {
    searchKey,
    filter,
    page,
    pageSize,
  });
  if (!response.success) throw new Error(response.message || 'Failed to search prompts');
  return response.data ?? { items: [], total: 0, page, pageSize };
};
