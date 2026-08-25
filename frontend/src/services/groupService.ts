import { Group, GroupPage, ApiResponse } from '@/types';
import { apiPost } from '../utils/fetchInterceptor';

/**
 * Paginated group search (SQL-level LIKE + LIMIT/OFFSET), backing the
 * ServerForm group dropdown. `page` is 0-based.
 */
export const searchGroups = async (
  searchKey: string,
  page: number,
  pageSize: number,
): Promise<GroupPage> => {
  const response: ApiResponse<GroupPage> = await apiPost('/groups/search', {
    searchKey,
    page,
    pageSize,
  });
  if (!response.success) throw new Error(response.message || 'Failed to search groups');
  return response.data ?? { items: [] as Group[], total: 0, page, pageSize };
};
