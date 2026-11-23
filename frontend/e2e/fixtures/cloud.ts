import path from 'node:path';

import { expect, type Page } from '@playwright/test';

import { BASE_URL } from './config';

const csrfCache = new WeakMap<Page, string>();

export async function gotoSignedInCloud(page: Page): Promise<void> {
  await page.goto('/workspace/storage');
  await expect(page.getByRole('heading', { name: 'Cloud Storage' })).toBeVisible();
  await expect(page.getByTestId('cloud-file-input')).toBeAttached();
}

export async function csrfToken(page: Page): Promise<string> {
  const cached = csrfCache.get(page);
  if (cached) return cached;
  const response = await page.request.get(`${BASE_URL}/api/v1/auth/me`);
  if (!response.ok()) {
    throw new Error(`GET /auth/me: HTTP ${response.status()} ${await response.text()}`);
  }
  const payload = (await response.json()) as { data: { csrfToken: string } };
  csrfCache.set(page, payload.data.csrfToken);
  return payload.data.csrfToken;
}

export async function apiCall<T>(
  page: Page,
  method: 'GET' | 'POST' | 'PATCH' | 'DELETE',
  url: string,
  data?: unknown,
): Promise<T> {
  const headers: Record<string, string> = { Accept: 'application/json' };
  if (method !== 'GET') headers['X-CSRF-Token'] = await csrfToken(page);
  const response = await page.request.fetch(`${BASE_URL}/api/v1${url}`, {
    method,
    headers,
    data,
    timeout: /\/purge$/.test(url) ? 180_000 : 60_000,
  });
  const text = await response.text();
  if (!response.ok()) {
    throw new Error(`${method} ${url}: HTTP ${response.status()} ${text}`);
  }
  return (text ? (JSON.parse(text) as { data: T }) : { data: undefined as T }).data;
}

export async function uploadFile(page: Page, file: string): Promise<void> {
  await page.getByTestId('cloud-file-input').setInputFiles(file);
}

export async function uploadBuffer(
  page: Page,
  name: string,
  buffer: Buffer,
  mimeType = 'text/plain',
): Promise<void> {
  await page.getByTestId('cloud-file-input').setInputFiles({ name, mimeType, buffer });
}

export async function uploadFolder(page: Page, directory: string): Promise<void> {
  if (!path.isAbsolute(directory)) throw new Error(`Ожидался абсолютный путь: ${directory}`);
  await page.getByTestId('cloud-folder-input').setInputFiles(directory);
}

export function cloudCard(page: Page, name: string) {
  return page.getByTestId('cloud-card').filter({ hasText: name });
}

export async function waitForCloudCard(page: Page, name: string): Promise<void> {
  await expect(cloudCard(page, name)).toBeVisible({ timeout: 30_000 });
}

export async function openCloudCard(page: Page, name: string): Promise<void> {
  const card = cloudCard(page, name);
  await expect(card).toBeVisible({ timeout: 30_000 });
  await card.locator('.cloud-card__name').click();
}

export async function cardEntryId(page: Page, name: string): Promise<string> {
  const card = cloudCard(page, name);
  await expect(card).toBeVisible({ timeout: 30_000 });
  const id = await card.getAttribute('data-entry-id');
  if (!id) throw new Error(`У карточки «${name}» нет data-entry-id`);
  return id;
}

/**
 * Полная очистка тестовой записи: корзина → безвозвратное удаление.
 * Для папки сервер удаляет вложенные записи рекурсивно.
 */
export async function purgeEntry(page: Page, id: string): Promise<void> {
  await apiCall(page, 'DELETE', `/cloud/entries/${encodeURIComponent(id)}`);
  await apiCall(page, 'DELETE', `/cloud/entries/${encodeURIComponent(id)}/purge`);
}

export async function purgeCard(page: Page, name: string): Promise<void> {
  const id = await cardEntryId(page, name);
  await purgeEntry(page, id);
}

export async function findEntryId(page: Page, name: string): Promise<string | null> {
  for (let pageNumber = 1; pageNumber <= 25; pageNumber += 1) {
    const data = await apiCall<{ items: Array<{ id: string; name: string }> }>(
      page,
      'GET',
      `/cloud/entries?view=folder&perPage=100&page=${pageNumber}&q=${encodeURIComponent(name)}`,
    );
    const found = data.items.find((item) => item.name === name);
    if (found) return found.id;
    if (data.items.length < 100) return null;
  }
  return null;
}

export async function purgeByName(page: Page, name: string): Promise<void> {
  const id = await findEntryId(page, name);
  if (id) await purgeEntry(page, id);
}

export async function waitForQueueDone(
  page: Page,
  expectedDone: number,
  timeout = 300_000,
): Promise<void> {
  await expect(page.getByTestId('upload-queue')).toBeVisible({ timeout: 30_000 });
  await expect(
    page.getByTestId('upload-queue').getByText(`${expectedDone} готово`, { exact: false }),
  ).toBeVisible({ timeout });
  await expect(page.getByTestId('upload-queue').getByText(/с ошибкой/)).toHaveCount(0);
}
