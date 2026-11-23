/**
 * Публичные ссылки Cloud Storage.
 *
 * Сгенерировано при разделении services/api.ts; правки — в этом модуле.
 */

import { ApiRequestError } from './http';

/** Публичный доступ к ссылке: не использует CSRF и cookie-сессию панели. */
export interface PublicShareInfo {
  name: string;
  size: number;
  contentType: string;
  hasPassword: boolean;
  authOnly: boolean;
  downloadCount: number;
  maxDownloads: number | null;
  expiresAt: string | null;
}

interface PublicShareFailure {
  success?: boolean;
  data?: PublicShareInfo;
  error?: { code?: string; message?: string };
}

export interface PublicShareUnlock {
  ticket: string;
  expiresAt: string;
}

export async function fetchPublicShare(token: string): Promise<PublicShareInfo> {
  const response = await fetch(`/api/v1/public/shares/${encodeURIComponent(token)}`);
  const payload = (await response.json().catch(() => null)) as PublicShareFailure | null;
  if (!response.ok || !payload?.success || !payload.data) {
    throw new ApiRequestError(
      response.status,
      payload?.error?.code ?? 'SHARE_UNAVAILABLE',
      payload?.error?.message ?? 'Ссылка не найдена или срок её действия истёк.',
    );
  }
  return payload.data;
}

/**
 * Проверяет пароль ссылки и получает короткоживущий тикет скачивания.
 * Пароль передаётся только в теле запроса и не попадает в URL.
 */
export async function unlockPublicShare(
  token: string,
  password?: string,
): Promise<PublicShareUnlock> {
  const response = await fetch(`/api/v1/public/shares/${encodeURIComponent(token)}/unlock`, {
    method: 'POST',
    credentials: 'same-origin',
    headers: { Accept: 'application/json', 'Content-Type': 'application/json' },
    body: JSON.stringify({ password: password ?? '' }),
  });
  const payload = (await response.json().catch(() => null)) as {
    success?: boolean;
    data?: PublicShareUnlock;
    error?: { code?: string; message?: string };
  } | null;
  if (!response.ok || !payload?.success || !payload.data) {
    throw new ApiRequestError(
      response.status,
      payload?.error?.code ?? 'SHARE_UNAVAILABLE',
      payload?.error?.message ?? 'Не удалось получить доступ к файлу.',
    );
  }
  return payload.data;
}

export function publicShareDownloadUrl(token: string, ticket?: string): string {
  const suffix = ticket ? `?ticket=${encodeURIComponent(ticket)}` : '';
  return `/api/v1/public/shares/${encodeURIComponent(token)}/download${suffix}`;
}
