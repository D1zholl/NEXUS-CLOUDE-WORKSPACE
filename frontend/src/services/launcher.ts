/**
 * Launcher API.
 *
 * Сгенерировано при разделении services/api.ts; правки — в этом модуле.
 */

import type {
  CreateLauncherTokenInput,
  LauncherRepositoryInfo,
  LauncherToken,
  LauncherTokenIssued,
} from './types';
import { ApiRequestError, request } from './http';

/** Запрос к Launcher API от имени лаунчера: токен передаётся в заголовке. */
export async function launcherRequest<T>(token: string, path: string): Promise<T> {
  const response = await fetch(`/api/v1${path}`, {
    headers: { Accept: 'application/json', Authorization: `Bearer ${token}` },
  });
  const payload = (await response.json()) as { success?: boolean; data?: T; error?: unknown };
  if (!response.ok || !payload.success || payload.data === undefined) {
    const error = payload.error as { code?: string; message?: string } | undefined;
    throw new ApiRequestError(
      response.status,
      error?.code ?? 'LAUNCHER_ERROR',
      error?.message ?? 'Запрос к Launcher API не выполнен.',
    );
  }
  return payload.data;
}

export const launcherApi = {
  listLauncherTokens: () => request<LauncherToken[]>('/admin/launcher/tokens'),
  repositoryInfo: () => request<LauncherRepositoryInfo>('/admin/launcher/repository'),
  issueLauncherToken: (input: CreateLauncherTokenInput) =>
    request<LauncherTokenIssued>('/launcher/auth/token', { method: 'POST', body: input }),
  revokeLauncherToken: (id: string) =>
    request<{ revoked: boolean }>(`/admin/launcher/tokens/${encodeURIComponent(id)}`, {
      method: 'DELETE',
    }),
  revokeMyLauncherToken: () =>
    request<{ revoked: boolean }>('/launcher/auth/revoke', { method: 'POST', body: {} }),
};

/** Проверка публичного каталога и первого манифеста тем же путём, что MyLauncher. */
export async function probeRepository(
  channel: 'stable' | 'beta',
): Promise<{
  packCount: number;
  names: string[];
  manifestVersion: number | null;
  fileCount: number;
}> {
  const response = await fetch(`/api/packs?channel=${channel}`, {
    headers: { Accept: 'application/json' },
  });
  if (!response.ok) {
    throw new ApiRequestError(
      response.status,
      'REPOSITORY_UNAVAILABLE',
      `Каталог вернул ${response.status}.`,
    );
  }
  const payload = (await response.json()) as {
    packs?: Array<{ id?: string; name?: string; latestVersion?: string; channel?: string }>;
  };
  const packs = Array.isArray(payload.packs) ? payload.packs : [];
  const names = packs.map((pack) => pack.name ?? 'Без названия');
  const first = packs.find((pack) => pack.id && pack.latestVersion);
  if (!first?.id || !first.latestVersion) {
    return { packCount: packs.length, names, manifestVersion: null, fileCount: 0 };
  }
  const manifestResponse = await fetch(
    `/api/packs/${encodeURIComponent(first.id)}/versions/${encodeURIComponent(first.latestVersion)}/manifest?channel=${first.channel === 'beta' ? 'beta' : 'stable'}`,
    { headers: { Accept: 'application/json' } },
  );
  if (!manifestResponse.ok) {
    throw new ApiRequestError(
      manifestResponse.status,
      'MANIFEST_UNAVAILABLE',
      `Манифест вернул ${manifestResponse.status}.`,
    );
  }
  const manifest = (await manifestResponse.json()) as {
    manifestVersion?: number;
    files?: unknown[];
  };
  return {
    packCount: packs.length,
    names,
    manifestVersion: manifest.manifestVersion ?? null,
    fileCount: Array.isArray(manifest.files) ? manifest.files.length : 0,
  };
}
