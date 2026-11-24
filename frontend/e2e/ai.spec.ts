import { ADMIN_STATE } from './fixtures/config';
import { expect, test } from './fixtures/test';

test.use({ storageState: ADMIN_STATE });

test.describe('AI Workspace UI', () => {
  test('открывает AI Workspace и Management без ошибок рендера', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));

    await page.goto('/workspace/ai');
    await expect(page.getByRole('heading', { level: 1, name: 'AI Workspace' })).toBeVisible({
      timeout: 30_000,
    });
    await expect(page.getByText('AI ещё не настроен')).toBeVisible({ timeout: 30_000 });

    await page.goto('/system/ai');
    await expect(page.getByRole('heading', { level: 1, name: 'AI Management' })).toBeVisible({
      timeout: 30_000,
    });
    await page.getByRole('tab', { name: 'MCP' }).click();
    await expect(page.getByText('MCP Connections')).toBeVisible({ timeout: 30_000 });

    expect(errors).toEqual([]);
  });
});
