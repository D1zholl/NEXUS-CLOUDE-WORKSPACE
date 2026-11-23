import { request, type APIRequestContext, type APIResponse } from '@playwright/test';

import { BASE_URL } from './config';

/**
 * API-сессия Playwright для сценариев, которым не нужен браузерный UI:
 * Windows-инварианты, подготовка данных для Minecraft/Launcher.
 *
 * Cookie берутся из storageState, CSRF — из `/auth/me`, как в самом приложении.
 */
export interface ApiSession {
  ctx: APIRequestContext;
  csrf: string;
}

export async function openApiSession(storageState: string): Promise<ApiSession> {
  const ctx = await request.newContext({
    baseURL: BASE_URL,
    storageState,
    extraHTTPHeaders: { Accept: 'application/json' },
  });
  const response = await ctx.get('/api/v1/auth/me');
  if (!response.ok()) {
    const text = await response.text().catch(() => '');
    await ctx.dispose();
    throw new Error(`GET /auth/me: HTTP ${response.status()} ${text}`);
  }
  const payload = (await response.json()) as { data: { csrfToken: string } };
  return { ctx, csrf: payload.data.csrfToken };
}

export interface FetchOptions {
  json?: unknown;
  /** Сырое тело: чанк загрузки, байты файла и т. п. */
  body?: Buffer | string;
  headers?: Record<string, string>;
  timeout?: number;
}

/** HTTP-запрос к API с cookie и CSRF текущей сессии. */
export async function apiFetch(
  session: ApiSession,
  method: 'GET' | 'POST' | 'PATCH' | 'PUT' | 'DELETE',
  path: string,
  options: FetchOptions = {},
): Promise<APIResponse> {
  const headers: Record<string, string> = {
    Accept: 'application/json',
    ...options.headers,
  };
  if (method !== 'GET' && method !== 'HEAD') {
    headers['X-CSRF-Token'] = session.csrf;
  }
  let data: Buffer | string | undefined = options.body;
  if (options.json !== undefined) {
    headers['Content-Type'] = 'application/json';
    data = JSON.stringify(options.json);
  }
  return session.ctx.fetch(`/api/v1${path}`, {
    method,
    headers,
    data,
    timeout: options.timeout ?? 120_000,
  });
}

/** Разворачивает конверт `{ success, data }` и падает с телом ошибки. */
export async function apiData<T>(
  session: ApiSession,
  method: 'GET' | 'POST' | 'PATCH' | 'PUT' | 'DELETE',
  path: string,
  options: FetchOptions = {},
): Promise<T> {
  const response = await apiFetch(session, method, path, options);
  const text = await response.text();
  if (!response.ok()) {
    throw new Error(`${method} ${path}: HTTP ${response.status()} ${text}`);
  }
  if (!text) return undefined as T;
  return (JSON.parse(text) as { data: T }).data;
}

export interface ApiFailure {
  status: number;
  body: {
    success: false;
    error: {
      code: string;
      message: string;
      details?: unknown;
    };
    request_id?: string;
  };
}

/** Ожидает ошибочный ответ и возвращает разобранный конверт. */
export async function expectFailure(response: APIResponse, status: number): Promise<ApiFailure> {
  const text = await response.text();
  if (response.status() !== status) {
    throw new Error(`ожидался HTTP ${status}, получен ${response.status()}: ${text}`);
  }
  return {
    status: response.status(),
    body: JSON.parse(text) as ApiFailure['body'],
  };
}

/** Короткий уникальный суффикс: не конфликтует с прошлыми прогонами. */
export function uniqueTag(prefix = ''): string {
  const stamp = Date.now().toString(36);
  const random = Math.random().toString(36).slice(2, 6);
  return `${prefix}${stamp}-${random}`;
}

/** Декодирует RFC 5987 `filename*=UTF-8''...` из Content-Disposition. */
export function decodeFilenameStar(header: string): string | null {
  const marker = "filename*=UTF-8''";
  const index = header.indexOf(marker);
  if (index < 0) return null;
  const encoded = header.slice(index + marker.length);
  const bytes: number[] = [];
  for (let cursor = 0; cursor < encoded.length; cursor += 1) {
    const char = encoded[cursor];
    if (char === '%') {
      bytes.push(Number.parseInt(encoded.slice(cursor + 1, cursor + 3), 16));
      cursor += 2;
    } else {
      bytes.push(char.charCodeAt(0));
    }
  }
  return Buffer.from(bytes).toString('utf8');
}

/** Удаляет Cloud-запись: корзина → безвозвратная очистка. */
export async function apiPurgeEntry(session: ApiSession, id: string): Promise<void> {
  await apiFetch(session, 'DELETE', `/cloud/entries/${encodeURIComponent(id)}`);
  await apiFetch(session, 'DELETE', `/cloud/entries/${encodeURIComponent(id)}/purge`);
}
