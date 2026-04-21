import { useState, useEffect, useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { User, ApiResponse, UserFormData, UserUpdateData } from '@/types';
import { apiDelete, apiGet, apiPost, apiPut } from '../utils/fetchInterceptor';

export const useUserData = () => {
  const { t } = useTranslation();
  const [users, setUsers] = useState<User[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);

  const fetchUsers = useCallback(async () => {
    try {
      setLoading(true);
      const data: ApiResponse<User[]> = await apiGet('/users');
      if (!data.success) {
        setError(data.message || t('users.fetchError'));
        return;
      }

      if (data && data.success && Array.isArray(data.data)) {
        setUsers(data.data);
      } else {
        console.error('Invalid user data format:', data);
        setUsers([]);
      }

      setError(null);
    } catch (err) {
      console.error('Error fetching users:', err);
      setError(err instanceof Error ? err.message : 'Failed to fetch users');
      setUsers([]);
    } finally {
      setLoading(false);
    }
  }, []);

  // Trigger a refresh of the users data
  const triggerRefresh = useCallback(() => {
    setRefreshKey((prev) => prev + 1);
  }, []);

  // Create a new user
  const createUser = async (userData: UserFormData) => {
    try {
      const result: ApiResponse<User> = await apiPost('/users', userData);
      triggerRefresh();
      return result;
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to create user');
      return null;
    }
  };

  // Update an existing user
  const updateUser = async (username: string, data: UserUpdateData) => {
    try {
      const result: ApiResponse<User> = await apiPut(`/users/${username}`, data);
      triggerRefresh();
      return result || null;
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to update user');
      return null;
    }
  };

  // Delete a user
  const deleteUser = async (username: string) => {
    try {
      const result = await apiDelete(`/users/${username}`);
      if (!result?.success) {
        setError(result?.message || t('users.deleteError'));
        return result;
      }

      triggerRefresh();
      return result;
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to delete user');
      return false;
    }
  };

  // Fetch users when the component mounts or refreshKey changes
  useEffect(() => {
    fetchUsers();
  }, [fetchUsers, refreshKey]);

  return {
    users,
    loading,
    error,
    setError,
    triggerRefresh,
    createUser,
    updateUser,
    deleteUser,
  };
};
