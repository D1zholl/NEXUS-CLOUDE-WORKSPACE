import { request, type APIRequestContext } from '@playwright/test';

import {
  ADMIN_STATE,
  BASE_URL,
  LOCK_PASSWORD,
  LOCK_USERNAME,
  USER_PASSWORD,
  USER_USERNAME,
  readMeta,
} from './fixtures/config';
import { expect, test } from './fixtures/test';

async function adminContext(): Promise<{ ctx: APIRequestContext; csrf: string }> {
  const ctx = await request.newContext({
    baseURL: BASE_URL,
    storageState: ADMIN_STATE,
    extraHTTPHeaders: { Accept: 'application/json' },
  });
  const me = await ctx.get('/api/v1/auth/me');
  if (!me.ok()) throw new Error(`admin /auth/me: HTTP ${me.status()} ${await me.text()}`);
  const payload = (await me.json()) as { data: { csrfToken: string } };
  return { ctx, csrf: payload.data.csrfToken };
}

async function adminSend(
  ctx: APIRequestContext,
  csrf: string,
  method: 'POST' | 'PUT',
  url: string,
  data: unknown,
): Promise<void> {
  const response = await ctx.fetch(url, {
    method,
    headers: {
      Accept: 'application/json',
      'Content-Type': 'application/json',
      'X-CSRF-Token': csrf,
    },
    data: JSON.stringify(data),
  });
  if (!response.ok()) {
    throw new Error(`${method} ${url}: HTTP ${response.status()} ${await response.text()}`);
  }
}

async function submitLogin(
  page: import('@playwright/test').Page,
  username: string,
  password: string,
) {
  await page.locator('#login-username').fill(username);
  await page.locator('#login-password').fill(password);
  await page.getByTestId('login-form').getByRole('button', { name: /Войти/ }).click();
}

async function readLoginMaxFailures(ctx: APIRequestContext): Promise<number> {
  const response = await ctx.get('/api/v1/system/settings');
  if (!response.ok()) return 10;
  const payload = (await response.json()) as {
    data: { values?: Record<string, unknown> };
  };
  const value = Number(payload.data.values?.['security.login_max_failures']);
  return Number.isFinite(value) && value >= 3 ? value : 10;
}

test.describe('I1: вход и сессии', () => {
  test('вход через UI, переход в Cloud и выход', async ({ page }) => {
    await page.goto('/login');
    await expect(page.getByTestId('login-form')).toBeVisible();
    await submitLogin(page, USER_USERNAME, USER_PASSWORD);

    await expect(page).toHaveURL(/\/$/);
    await expect(page.getByRole('link', { name: 'Cloud Storage' }).first()).toBeVisible();
    await page.getByRole('link', { name: 'Cloud Storage' }).first().click();
    await expect(page).toHaveURL(/\/workspace\/storage/);
    await expect(page.getByRole('heading', { name: 'Cloud Storage' })).toBeVisible();

    await page.getByTitle('Меню пользователя').click();
    await page.getByRole('menuitem', { name: 'Выйти' }).click();
    await expect(page).toHaveURL(/\/login$/);
    await expect(page.getByTestId('login-form')).toBeVisible();

    // Защищённый маршрут после выхода возвращает на форму входа.
    await page.goto('/workspace/storage');
    await expect(page).toHaveURL(/\/login$/);
    await expect(page.getByTestId('login-form')).toBeVisible();
  });

  test('неверный пароль: понятная ошибка и успешный вход после неё', async ({ page }) => {
    await page.goto('/login');
    await submitLogin(page, USER_USERNAME, `${USER_PASSWORD}-wrong`);
    await expect(page.getByRole('alert')).toContainText(/Неверн.*пароль/i);

    await submitLogin(page, USER_USERNAME, USER_PASSWORD);
    await expect(page).toHaveURL(/\/$/);
    await expect(page.getByRole('link', { name: 'Cloud Storage' }).first()).toBeVisible();
  });

  test('блокировка после серии неверных попыток', async ({ page }) => {
    const admin = await adminContext();
    const previousSetting = await readLoginMaxFailures(admin.ctx);
    try {
      // Минимум каталога — 3. Уменьшаем порог, чтобы не ждать 10 попыток;
      // login-limiter (5) при этом ещё не успевает сработать.
      await adminSend(
        admin.ctx,
        admin.csrf,
        'PUT',
        '/api/v1/system/settings/security.login_max_failures',
        3,
      );
      const meta = readMeta();
      await adminSend(
        admin.ctx,
        admin.csrf,
        'POST',
        `/api/v1/users/${meta.lockUser.id}/unlock`,
        {},
      );

      await page.goto('/login');
      for (let attempt = 1; attempt <= 3; attempt += 1) {
        await submitLogin(page, LOCK_USERNAME, `${LOCK_PASSWORD}-wrong-${attempt}`);
        const alert = page.getByRole('alert');
        await expect(alert).toBeVisible();
        if (attempt < 3) {
          await expect(alert).toContainText(/Неверн.*пароль/i);
        } else {
          await expect(alert).toContainText(/заблокирована/i);
        }
      }

      // Даже верный пароль не пускает заблокированную запись.
      await submitLogin(page, LOCK_USERNAME, LOCK_PASSWORD);
      await expect(page.getByRole('alert')).toContainText(/заблокирована/i);
      await expect(page).toHaveURL(/\/login$/);
    } finally {
      await adminSend(
        admin.ctx,
        admin.csrf,
        'PUT',
        '/api/v1/system/settings/security.login_max_failures',
        previousSetting,
      );
      const meta = readMeta();
      await adminSend(
        admin.ctx,
        admin.csrf,
        'POST',
        `/api/v1/users/${meta.lockUser.id}/unlock`,
        {},
      );
      await admin.ctx.dispose();
    }
  });
});
