import { ADMIN_STATE, USER_STATE } from './fixtures/config';
import { apiData, apiFetch, expectFailure, openApiSession, type ApiSession } from './fixtures/api';
import { expect, test } from './fixtures/test';

interface ModuleRow {
  key: string;
  enabled: boolean;
}

async function restoreModules(admin: ApiSession, previous: ModuleRow[]): Promise<void> {
  for (const row of previous) {
    await apiData(admin, 'PUT', `/system/modules/${row.key}`, {
      json: { enabled: row.enabled },
    });
  }
}

test.describe('I1: модули и права', () => {
  test.use({ storageState: USER_STATE });

  test('отключение модуля блокирует UI и API', async ({ page }) => {
    test.setTimeout(120_000);
    const admin = await openApiSession(ADMIN_STATE);
    const user = await openApiSession(USER_STATE);
    const previous = await apiData<ModuleRow[]>(admin, 'GET', '/system/modules');

    try {
      // Cloud: серверный гейт отвечает MODULE_DISABLED, навигация и маршрут скрыты.
      await apiData(admin, 'PUT', '/system/modules/cloud', { json: { enabled: false } });
      const cloudResponse = await apiFetch(user, 'GET', '/cloud/entries?view=folder&perPage=1');
      const cloudFailure = await expectFailure(cloudResponse, 403);
      expect(cloudFailure.body.error.code).toBe('MODULE_DISABLED');
      expect(cloudFailure.body.error.details).toMatchObject({ module: 'cloud' });

      await page.goto('/');
      await expect(page.getByRole('link', { name: 'Cloud Storage' })).toHaveCount(0);
      await expect(page.getByText('Отключён администратором')).toBeVisible();

      await page.goto('/workspace/storage');
      await expect(page.getByTestId('module-disabled')).toBeVisible();
      await expect(page.getByRole('heading', { name: 'Модуль отключён' })).toBeVisible();
      await expect(
        page.getByText(/Администратор отключил модуль «NEXUS Cloud Storage»/),
      ).toBeVisible();

      // Minecraft: блокируются и сборки, и внешний Launcher API.
      await apiData(admin, 'PUT', '/system/modules/minecraft', { json: { enabled: false } });
      const buildsResponse = await apiFetch(user, 'GET', '/minecraft/builds?perPage=1');
      const buildsFailure = await expectFailure(buildsResponse, 403);
      expect(buildsFailure.body.error.code).toBe('MODULE_DISABLED');

      const launcherResponse = await apiFetch(user, 'GET', '/launcher/catalog', {
        headers: { Authorization: 'Bearer nxl_invalid' },
      });
      const launcherFailure = await expectFailure(launcherResponse, 404);
      expect(launcherFailure.body.error.code).toBe('NOT_FOUND');

      await page.goto('/');
      await expect(page.getByRole('link', { name: 'Minecraft Builds' })).toHaveCount(0);
      await expect(page.getByRole('link', { name: 'Launcher API' })).toHaveCount(0);
      await expect(page.getByText('Отключён администратором')).toHaveCount(2);

      await page.goto('/workspace/builds');
      await expect(page.getByTestId('module-disabled')).toBeVisible();
      await expect(
        page.getByText(/Администратор отключил модуль «Minecraft Build Manager»/),
      ).toBeVisible();
    } finally {
      await restoreModules(admin, previous);
      await user.ctx.dispose();
      await admin.ctx.dispose();
    }
  });

  test('пользователь без system.view не видит системные разделы', async ({ page }) => {
    test.setTimeout(90_000);
    const user = await openApiSession(USER_STATE);
    try {
      const me = await user.ctx.get('/api/v1/auth/me');
      expect(me.ok()).toBe(true);
      const session = (await me.json()) as {
        data: {
          user: { isSuperadmin: boolean; permissions: string[] };
        };
      };
      expect(session.data.user.isSuperadmin).toBe(false);
      expect(session.data.user.permissions).not.toContain('system.view');

      await page.goto('/');
      await expect(page.getByRole('link', { name: 'Cloud Storage' }).first()).toBeVisible();

      for (const label of [
        'Состояние',
        'Пользователи',
        'Роли и права',
        'Журнал событий',
        'Мониторинг',
        'Фоновые задачи',
        'Хранилища',
        'Общие настройки',
        'Ограничения',
        'Сеть',
        'Безопасность',
        'Обслуживание',
      ]) {
        await expect(page.getByRole('link', { name: label })).toHaveCount(0);
      }

      await page.goto('/system');
      await expect(page.getByRole('heading', { name: 'Недостаточно прав' })).toBeVisible();

      const overview = await apiFetch(user, 'GET', '/system/overview');
      const failure = await expectFailure(overview, 403);
      expect(failure.body.error.code).toBe('FORBIDDEN');
      expect(failure.body.error.details).toMatchObject({
        required_permission: 'system.view',
      });
    } finally {
      await user.ctx.dispose();
    }
  });
});
