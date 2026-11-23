/**
 * Роли и права.
 *
 * Сгенерировано при разделении services/api.ts; правки — в этом модуле.
 */

import type { CreateRoleInput, Permission, Role, UpdateRoleInput } from './types';
import { request } from './http';

export const rolesApi = {
  listRoles: () => request<Role[]>('/roles'),
  createRole: (input: CreateRoleInput) => request<Role>('/roles', { method: 'POST', body: input }),
  updateRole: (id: string, input: UpdateRoleInput) =>
    request<Role>(`/roles/${encodeURIComponent(id)}`, { method: 'PATCH', body: input }),
  deleteRole: (id: string) =>
    request<{ deleted: boolean }>(`/roles/${encodeURIComponent(id)}`, { method: 'DELETE' }),
  listPermissions: () => request<Permission[]>('/permissions'),
};
