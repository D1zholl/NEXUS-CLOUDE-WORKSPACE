/**
 * Системные настройки, мониторинг и задачи.
 *
 * Сгенерировано при разделении services/api.ts; правки — в этом модуле.
 */

import type {
  AuditRow,
  BackgroundJob,
  BackgroundSettings,
  JobsResponse,
  ListAuditParams,
  ModuleStateRow,
  MonitoringCurrent,
  MonitoringHistory,
  MonitoringModules,
  MyUsage,
  Overview,
  Page,
  SystemSettingSpec,
  SystemSettingsResponse,
} from './types';
import { request, queryString } from './http';

export const systemApi = {
  getMySettings: () => request<Record<string, unknown>>('/me/settings'),
  saveMySetting: <T>(key: string, value: T) =>
    request<T>(`/me/settings/${encodeURIComponent(key)}`, {
      method: 'PUT',
      body: value,
    }),
  getPublicBackground: () => request<BackgroundSettings>('/public/appearance/background'),
  myUsage: () => request<MyUsage>('/me/usage'),

  /* --- Система --- */
  overview: () => request<Overview>('/system/overview'),
  listModules: () => request<ModuleStateRow[]>('/system/modules'),
  setModule: (key: string, enabled: boolean) =>
    request<ModuleStateRow>(`/system/modules/${encodeURIComponent(key)}`, {
      method: 'PUT',
      body: { enabled },
    }),
  listAudit: (params: ListAuditParams = {}) =>
    request<Page<AuditRow>>(
      `/system/audit${queryString({
        page: params.page,
        perPage: params.perPage,
        eventType: params.eventType,
      })}`,
    ),
  getSystemSettings: () => request<SystemSettingsResponse>('/system/settings'),
  getSettingsCatalog: () => request<SystemSettingSpec[]>('/system/settings/catalog'),
  saveSystemSetting: <T>(key: string, value: T) =>
    request<T>(`/system/settings/${encodeURIComponent(key)}`, {
      method: 'PUT',
      body: value,
    }),
};

export const monitoringApi = {
  monitoringCurrent: () => request<MonitoringCurrent>('/monitoring/current'),
  monitoringHistory: (window: string) =>
    request<MonitoringHistory>(`/monitoring/history${queryString({ window })}`),
  monitoringModules: () => request<MonitoringModules>('/monitoring/modules'),
  listJobs: (params: { status?: string; jobType?: string; limit?: number } = {}) =>
    request<JobsResponse>(`/jobs${queryString(params)}`),
  getJob: (id: string) => request<BackgroundJob>(`/jobs/${encodeURIComponent(id)}`),
  pauseJob: (id: string) =>
    request<BackgroundJob>(`/jobs/${encodeURIComponent(id)}/pause`, {
      method: 'POST',
      body: {},
    }),
  resumeJob: (id: string) =>
    request<BackgroundJob>(`/jobs/${encodeURIComponent(id)}/resume`, {
      method: 'POST',
      body: {},
    }),
  cancelJob: (id: string) =>
    request<BackgroundJob>(`/jobs/${encodeURIComponent(id)}/cancel`, {
      method: 'POST',
      body: {},
    }),
  runMaintenance: (kind: string) =>
    request<{ id: string; jobType: string }>('/jobs/maintenance', {
      method: 'POST',
      body: { kind },
    }),
};
