/**
 * Авторизация, сессия и профиль.
 *
 * Сгенерировано при разделении services/api.ts; правки — в этом модуле.
 */

import type { HealthResponse, MeResponse, SetupStatus } from './types';
import { request } from './http';

export const authApi = {
  health: () => request<HealthResponse>('/health'),
  setupStatus: () => request<SetupStatus>('/auth/setup-status'),

  /* --- Авторизация --- */
  login: (username: string, password: string) =>
    request<MeResponse>('/auth/login', {
      method: 'POST',
      body: { username, password },
    }),
  me: () => request<MeResponse>('/auth/me'),
  logout: () => request<{ ok: boolean }>('/auth/logout', { method: 'POST', body: {} }),
  changePassword: (currentPassword: string, newPassword: string) =>
    request<{ ok: boolean }>('/auth/password', {
      method: 'POST',
      body: { currentPassword, newPassword },
    }),
  revokeAllSessions: () =>
    request<{ ok: boolean }>('/auth/sessions/revoke-all', { method: 'POST', body: {} }),
};
