import { ADMIN_STATE } from './fixtures/config';
import { apiData, openApiSession } from './fixtures/api';
import { expect, test } from './fixtures/test';

test.use({ storageState: ADMIN_STATE });

const SYSTEM_PAGES: Array<{ path: string; title: string }> = [
  { path: '/system', title: 'Состояние системы' },
  { path: '/system/users', title: 'Пользователи' },
  { path: '/system/roles', title: 'Роли и права' },
  { path: '/system/audit', title: 'Журнал событий' },
  { path: '/system/monitoring', title: 'Мониторинг' },
  { path: '/system/jobs', title: 'Фоновые задачи' },
  { path: '/system/storage', title: 'Хранилища и диски' },
  { path: '/system/general', title: 'Общие настройки' },
  { path: '/system/limits', title: 'Ограничения ресурсов' },
  { path: '/system/network', title: 'Сетевые настройки' },
  { path: '/system/security', title: 'Безопасность' },
  { path: '/system/maintenance', title: 'Обслуживание' },
  { path: '/system/ai', title: 'AI Management' },
  { path: '/system/appearance', title: 'Внешний вид' },
  { path: '/system/background', title: '3D-фон' },
];

test.describe('I1: System', () => {
  test('15 страниц открываются без ошибок консоли', async ({ page }) => {
    test.setTimeout(180_000);

    for (const entry of SYSTEM_PAGES) {
      await page.goto(entry.path);
      await expect(
        page.getByRole('heading', { level: 1, name: entry.title }),
        `Страница ${entry.path}`,
      ).toBeVisible({ timeout: 30_000 });
    }
  });

  test('системная настройка применяется и отражается в effective', async ({ page }) => {
    test.setTimeout(120_000);
    const admin = await openApiSession(ADMIN_STATE);
    let previous: number | null = null;
    try {
      await page.goto('/system/limits');
      const card = page.getByTestId('system-setting-limits.max_upload_mb');
      await expect(card).toBeVisible();

      const input = card.locator('input[type="number"]');
      const current = Number(await input.inputValue());
      expect(Number.isFinite(current)).toBe(true);
      previous = current;
      const next = current === 10240 ? 10241 : 10240;

      await input.fill(String(next));
      const save = card.getByRole('button', { name: 'Сохранить' });
      await expect(save).toBeEnabled();
      await save.click();
      await expect(page.locator('.toast').filter({ hasText: 'Настройка сохранена.' })).toBeVisible({
        timeout: 30_000,
      });

      const settings = await apiData<{
        values: Record<string, unknown>;
        effective: Record<string, unknown>;
      }>(admin, 'GET', '/system/settings');
      expect(Number(settings.values['limits.max_upload_mb'])).toBe(next);
      expect(Number(settings.effective['limits.max_upload_mb'])).toBe(next);

      // UI перечитывает настройки и показывает сохранённое значение.
      await expect(input).toHaveValue(String(next), { timeout: 30_000 });
    } finally {
      if (previous !== null) {
        await apiData(admin, 'PUT', '/system/settings/limits.max_upload_mb', {
          json: previous,
        });
      }
      await admin.ctx.dispose();
    }
  });

  test('K4: reduced-motion не отключает анимацию интерфейса', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.goto('/system/background');
    await expect(page.getByRole('heading', { level: 1, name: '3D-фон' })).toBeVisible({
      timeout: 30_000,
    });
    await expect(page.locator('.bg-preview canvas, .bg-preview .bg-fallback')).toHaveCount(1);
    const animationDuration = await page.locator('.bg-preview').evaluate((preview) => {
      const probe = document.createElement('div');
      probe.className = 'loading-cube__inner';
      preview.append(probe);
      const duration = getComputedStyle(probe).animationDuration;
      probe.remove();
      return duration;
    });
    expect(animationDuration).toBe('3.4s');

    await page.goto('/system/appearance');
    await expect(page.getByTestId('motion-state')).toContainText('Анимация включена');
  });

  test('K4: акцент внешнего вида сохраняется в профиле', async ({ page }) => {
    const admin = await openApiSession(ADMIN_STATE);
    try {
      await page.goto('/system/appearance');
      await page.getByRole('button', { name: 'Магента' }).click();
      await expect(page.locator('html')).toHaveAttribute('data-accent', 'magenta');
      await page.getByRole('button', { name: 'Сохранить', exact: true }).click();
      await expect(page.locator('.toast').filter({ hasText: 'Внешний вид сохранён.' })).toBeVisible(
        { timeout: 30_000 },
      );

      await page.reload();
      await expect(page.locator('html')).toHaveAttribute('data-accent', 'magenta', {
        timeout: 30_000,
      });

      const values = await apiData<Record<string, { accent?: string }>>(
        admin,
        'GET',
        '/me/settings',
      );
      expect(values['appearance.interface']?.accent).toBe('magenta');
    } finally {
      // Возвращаем базовый акцент, чтобы тест не влиял на другие сценарии.
      await apiData(admin, 'PUT', '/me/settings/appearance.interface', {
        json: { accent: 'cyan' },
      });
      await admin.ctx.dispose();
    }
  });
});

test.describe('K4: узкий экран', () => {
  test.use({ storageState: ADMIN_STATE, viewport: { width: 360, height: 740 } });

  test('навигация сворачивается на 360 px и ведёт в оба раздела внешнего вида', async ({
    page,
  }) => {
    await page.goto('/system');
    await expect(page.getByRole('heading', { level: 1, name: 'Состояние системы' })).toBeVisible();
    const toggle = page.getByRole('button', { name: 'Открыть навигацию' });
    await expect(toggle).toBeVisible();
    await expect(page.locator('.sidebar')).not.toHaveClass(/sidebar--open/);

    await toggle.click();
    await expect(page.locator('.sidebar')).toHaveClass(/sidebar--open/);
    await page.getByRole('link', { name: 'Внешний вид' }).click();
    await expect(page).toHaveURL(/\/system\/appearance$/);
    await expect(page.getByRole('heading', { level: 1, name: 'Внешний вид' })).toBeVisible();

    await toggle.click();
    await page.getByRole('link', { name: '3D-фон' }).click();
    await expect(page).toHaveURL(/\/system\/background$/);
    await expect(page.getByRole('heading', { level: 1, name: '3D-фон' })).toBeVisible();

    const overflow = await page.evaluate(
      () => document.documentElement.scrollWidth - window.innerWidth,
    );
    expect(overflow).toBeLessThanOrEqual(1);
  });
});
