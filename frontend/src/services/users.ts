/**
 * Пользователи.
 *
 * Сгенерировано при разделении services/api.ts; правки — в этом модуле.
 */

import type {
  CreateUserInput,
  ListUsersParams,
  Page,
  UpdateUserInput,
  UserDetail,
  UserSummary,
} from './types';
import { request, queryString } from './http';

export const usersApi = {
  listUsers: (params: ListUsersParams = {}) =>
    request<Page<UserSummary>>(
      `/users${queryString({
        page: params.page,
        perPage: params.perPage,
        q: params.q,
        status: params.status,
      })}`,
    ),
  getUser: (id: string) => request<UserDetail>(`/users/${encodeURIComponent(id)}`),
  createUser: (input: CreateUserInput) =>
    request<UserDetail>('/users', { method: 'POST', body: input }),
  updateUser: (id: string, input: UpdateUserInput) =>
    request<UserDetail>(`/users/${encodeURIComponent(id)}`, {
      method: 'PATCH',
      body: input,
    }),
  deleteUser: (id: string) =>
    request<{ deleted: boolean }>(`/users/${encodeURIComponent(id)}`, { method: 'DELETE' }),
  resetUserPassword: (id: string, password: string) =>
    request<{ ok: boolean }>(`/users/${encodeURIComponent(id)}/password`, {
      method: 'POST',
      body: { password },
    }),
  unlockUser: (id: string) =>
    request<{ ok: boolean }>(`/users/${encodeURIComponent(id)}/unlock`, {
      method: 'POST',
      body: {},
    }),
  revokeUserSessions: (id: string) =>
    request<{ revoked: number }>(`/users/${encodeURIComponent(id)}/sessions/revoke`, {
      method: 'POST',
      body: {},
    }),
};
