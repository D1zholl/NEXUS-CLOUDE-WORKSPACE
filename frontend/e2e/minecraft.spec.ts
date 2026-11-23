import type { Page } from '@playwright/test';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { ADMIN_STATE, readMeta } from './fixtures/config';
import { apiData, apiFetch, openApiSession, uniqueTag } from './fixtures/api';
import { expect, test } from './fixtures/test';

test.use({ storageState: ADMIN_STATE });

/**
 * Если внешний источник метаданных Mojang недоступен (офлайн-стенд),
 * мастер получает детерминированный список версий. Проверяем UI-поток, а не
 * доступность launchermeta.mojang.com.
 */
async function metadataWithFallback(page: Page): Promise<void> {
  await page.route('**/api/v1/minecraft/metadata/minecraft', async (route) => {
    try {
      const response = await route.fetch({ timeout: 15_000 });
      if (response.ok()) {
        await route.fulfill({ response });
        return;
      }
    } catch {
      // Ниже отдаём стабильный ответ.
    }
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        success: true,
        data: {
          latest: { release: '1.20.1' },
          versions: [{ id: '1.20.1', releasedAt: '2023-06-12T12:00:00+00:00' }],
        },
      }),
    });
  });
}

test.describe('I1: Minecraft Build Manager', () => {
  let buildId = '';

  test.afterEach(async () => {
    if (!buildId) return;
    // Архивирование разрешает удалить сборку с опубликованным релизом.
    const admin = await openApiSession(ADMIN_STATE);
    try {
      await apiFetch(admin, 'POST', `/minecraft/builds/${buildId}/archive`, { json: {} });
      await apiFetch(admin, 'DELETE', `/minecraft/builds/${buildId}`);
    } finally {
      await admin.ctx.dispose();
      buildId = '';
    }
  });

  test('мастер из 5 шагов, файл, проверка, релиз и манифест', async ({ page }) => {
    test.setTimeout(300_000);
    const meta = readMeta();
    const tag = uniqueTag('mc-');
    const buildName = `E2E Build ${tag}`;
    const fileName = `e2e-artifact-${tag}.jar`;

    await metadataWithFallback(page);
    await page.goto('/workspace/builds');
    await expect(page.getByRole('heading', { name: 'Minecraft Builds' })).toBeVisible();

    await page.getByRole('button', { name: 'Новая сборка' }).click();
    const wizard = page.getByRole('dialog');
    await expect(wizard).toBeVisible();
    await expect(wizard.getByText('Основные данные')).toBeVisible();

    // Шаг 1: основные данные.
    await wizard.locator('#wizard-name').fill(buildName);
    await wizard.locator('#wizard-description').fill(`E2E сценарий ${meta.runId}`);
    await wizard.locator('#wizard-visibility').selectOption('public');

    // Регрессия фокуса: 12 символов подряд остаются в поле, а не на крестике.
    const nameInput = wizard.locator('#wizard-name');
    await nameInput.fill('');
    await nameInput.click();
    await page.keyboard.type('0123456789ab', { delay: 8 });
    await expect(nameInput).toBeFocused();
    await expect(nameInput).toHaveValue('0123456789ab');
    await nameInput.fill(buildName);

    await wizard.getByRole('button', { name: 'Далее' }).click();

    // Шаг 2: версия Minecraft.
    const versionSelect = wizard.locator('select').first();
    await expect(versionSelect.locator('option[value="1.20.1"]')).toBeAttached({
      timeout: 30_000,
    });
    await versionSelect.selectOption('1.20.1');
    await wizard.getByRole('button', { name: 'Далее' }).click();

    // Шаг 3: загрузчик (Vanilla не требует версии).
    await expect(wizard.locator('#wizard-loader')).toHaveValue('vanilla');
    await wizard.getByRole('button', { name: 'Далее' }).click();

    // Шаг 4: Java подставлена по версии игры.
    await expect(wizard.locator('#wizard-java-major')).toHaveValue('17');
    await wizard.getByRole('button', { name: 'Далее' }).click();

    // Шаг 5: итог и создание.
    await expect(wizard.getByText(buildName, { exact: false }).first()).toBeVisible();
    await wizard.getByRole('button', { name: 'Создать сборку' }).click();

    await expect(page).toHaveURL(/\/workspace\/builds\/[0-9a-f-]{36}$/);
    buildId = page.url().split('/').pop() ?? '';
    expect(buildId).toMatch(/^[0-9a-f-]{36}$/);
    await expect(page.getByRole('heading', { level: 1 })).toContainText(buildName);

    // Загрузка артефакта через штатный input страницы сборки.
    await page.getByTestId('build-file-input').setInputFiles({
      name: fileName,
      mimeType: 'application/java-archive',
      buffer: Buffer.from(`NEXUS E2E artifact ${meta.runId}\n`, 'utf8'),
    });
    const uploadItem = page.locator('.upload-item').filter({ hasText: fileName });
    await expect(uploadItem).toContainText('Готово', { timeout: 60_000 });
    await expect(page.getByText(fileName, { exact: false }).first()).toBeVisible();

    // Проверка сборки: файл обязательный, ошибок и предупреждений быть не должно.
    await page.getByRole('button', { name: 'Проверить' }).click();
    await expect(page.locator('.validation-report')).toContainText('Сборка готова к публикации.', {
      timeout: 30_000,
    });

    // Релиз и публикация.
    await page.getByRole('button', { name: 'Новый релиз' }).click();
    const releaseDialog = page.getByRole('dialog');
    await expect(releaseDialog).toBeVisible();
    await releaseDialog.locator('#release-version').fill('1.0.0');
    await releaseDialog.locator('#release-changelog').fill(`Релиз E2E ${meta.runId}`);
    await releaseDialog.getByRole('button', { name: 'Создать релиз' }).click();
    await expect(page.locator('.toast').filter({ hasText: 'Релиз 1.0.0 создан.' })).toBeVisible({
      timeout: 30_000,
    });

    await page.getByRole('button', { name: 'Релизы' }).click();
    const releaseCard = page.locator('.release-card').filter({ hasText: '1.0.0' });
    await expect(releaseCard).toBeVisible();
    await releaseCard.getByRole('button', { name: 'Опубликовать' }).click();
    await expect(
      page.locator('.toast').filter({ hasText: 'Релиз 1.0.0 опубликован.' }).first(),
    ).toBeVisible({ timeout: 30_000 });
    await expect(releaseCard.getByText('Опубликован', { exact: true })).toBeVisible();
    await expect(releaseCard.getByText('текущий', { exact: true })).toBeVisible();

    // Манифест опубликованного релиза: по умолчанию открыт MyLauncher.
    await releaseCard.getByRole('button', { name: 'Манифест' }).click();
    const manifestDialog = page.getByRole('dialog');
    await expect(manifestDialog.getByText(/Файлов:\s*1/)).toBeVisible({
      timeout: 30_000,
    });
    const launcherText = manifestDialog.locator('pre.cloud-preview-text');
    await expect(launcherText).toContainText('"manifestVersion": 1');
    await expect(launcherText).toContainText(fileName);
    await expect(launcherText).toContainText('/api/files/sha256:');

    await manifestDialog.getByRole('button', { name: /Внутренний \(schema_version 1\.0\)/ }).click();
    await expect(manifestDialog.getByText(/Файлов в манифесте:\s*1/)).toBeVisible();
    const manifestText = manifestDialog.locator('pre.cloud-preview-text');
    await expect(manifestText).toContainText('"schema_version": "1.0"');
    await expect(manifestText).toContainText(fileName);
    await expect(manifestText).toContainText('/api/v1/launcher/artifacts/');
  });

  test('каноническое дерево: папка config, drop в mods, options.txt и фокус', async ({
    page,
  }) => {
    test.setTimeout(300_000);
    const admin = await openApiSession(ADMIN_STATE);
    const tag = uniqueTag('mcf-');
    let tempRoot = '';
    try {
      const build = await apiData<{ id: string }>(admin, 'POST', '/minecraft/builds', {
        json: {
          name: `E2E Files ${tag}`,
          slug: `e2e-files-${tag}`,
          description: '',
          version: '1.0.0',
          category: 'tech',
          visibility: 'private',
          minecraftVersion: '1.20.1',
          loaderType: 'vanilla',
          loaderVersion: '',
          javaMajor: 17,
          javaMin: 17,
          javaNotes: '',
        },
      });
      buildId = build.id;

      // Папка config целиком: Playwright передаёт каталог в webkitdirectory.
      tempRoot = await mkdtemp(join(tmpdir(), 'nexus-e2e-'));
      const configDir = join(tempRoot, 'config');
      await mkdir(join(configDir, 'create', 'deep'), { recursive: true });
      await writeFile(join(configDir, 'create', 'deep', 'a.toml'), 'x = 1\n');
      await writeFile(join(configDir, 'create', 'b.json'), '{}\n');

      await page.goto(`/workspace/builds/${buildId}`);
      await expect(page.getByTestId('build-file-manager')).toBeVisible();
      await page.getByTestId('build-folder-input').setInputFiles(configDir);
      await expect(
        page.locator('.upload-item').filter({ hasText: 'config/create/deep/a.toml' }),
      ).toContainText('Готово', { timeout: 60_000 });
      await expect(
        page.locator('.upload-item').filter({ hasText: 'config/create/b.json' }),
      ).toContainText('Готово', { timeout: 60_000 });

      await page.getByTestId('build-tree-config').click();
      await expect(page.getByText('config/create/deep/a.toml').first()).toBeVisible();
      await expect(page.getByText('config/create/b.json').first()).toBeVisible();

      // Drop .jar на узел mods раскладывает файл в mods/ без подпапок.
      const dropJar = await page.evaluateHandle(({ name }) => {
        const transfer = new DataTransfer();
        transfer.items.add(
          new File(['NEXUS dropped jar'], name, { type: 'application/java-archive' }),
        );
        return transfer;
      }, { name: `e2e-drop-${tag}.jar` });
      await page.getByTestId('build-tree-mods').click();
      await page.dispatchEvent('[data-testid="build-file-manager"]', 'drop', {
        dataTransfer: dropJar,
      });
      await expect(
        page.locator('.upload-item').filter({ hasText: `mods/e2e-drop-${tag}.jar` }),
      ).toContainText('Готово', { timeout: 60_000 });
      await expect(page.getByText(`mods/e2e-drop-${tag}.jar`).first()).toBeVisible();

      // Drop options.txt в выделенный слот в корне.
      const dropOptions = await page.evaluateHandle(() => {
        const transfer = new DataTransfer();
        transfer.items.add(new File(['fov:0.5\n'], 'options.txt', { type: 'text/plain' }));
        return transfer;
      });
      await page.dispatchEvent('[data-testid="build-tree-options"]', 'drop', {
        dataTransfer: dropOptions,
      });
      await expect(
        page.locator('.upload-item').filter({ hasText: 'options.txt' }),
      ).toContainText('Готово', { timeout: 60_000 });
      await page.getByTestId('build-tree-root').click();
      await expect(page.getByText('options.txt').first()).toBeVisible();

      // Внутренний DnD: файл из mods в config.
      await page.getByTestId('build-tree-mods').click();
      const jarRow = page.locator('tr').filter({ hasText: `e2e-drop-${tag}.jar` });
      await jarRow.dragTo(page.getByTestId('build-tree-config'));
      await expect(
        page.locator('.toast').filter({ hasText: 'Путь обновлён.' }),
      ).toBeVisible({ timeout: 30_000 });
      await page.getByTestId('build-tree-config').click();
      await expect(page.getByText(`config/e2e-drop-${tag}.jar`).first()).toBeVisible();

      // Длинный ввод в модальной форме не теряет фокус.
      await page.getByRole('button', { name: 'Новый релиз' }).click();
      const dialog = page.getByRole('dialog');
      const version = dialog.locator('#release-version');
      await version.click();
      await page.keyboard.type('123456789012345', { delay: 8 });
      await expect(version).toBeFocused();
      await expect(version).toHaveValue('123456789012345');
      await dialog.getByRole('button', { name: 'Отмена' }).click();
    } finally {
      if (tempRoot) await rm(tempRoot, { recursive: true, force: true });
      await admin.ctx.dispose();
    }
  });
});
