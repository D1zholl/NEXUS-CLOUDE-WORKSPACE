import path from 'node:path';

import { expect, test } from './fixtures/test';
import { BASE_URL, USER_STATE, readMeta, type E2EMeta } from './fixtures/config';
import {
  cardEntryId,
  cloudCard,
  gotoSignedInCloud,
  openCloudCard,
  purgeCard,
  uploadBuffer,
  uploadFile,
  uploadFolder,
  waitForCloudCard,
  waitForQueueDone,
} from './fixtures/cloud';

test.use({ storageState: USER_STATE });

let meta: E2EMeta;
let runId = '';

test.beforeAll(() => {
  meta = readMeta();
  runId = meta.runId;
});

function fileMeta(metaData: E2EMeta, file: string): string {
  return path.basename(file);
}

test.describe('I1: Cloud Storage', () => {
  test('загрузка файла и предпросмотр текста с подсветкой', async ({ page }) => {
    const name = fileMeta(meta, meta.files.highlight);
    await gotoSignedInCloud(page);
    await uploadFile(page, meta.files.highlight);
    await waitForCloudCard(page, name);

    await openCloudCard(page, name);
    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog.getByText(name, { exact: false }).first()).toBeVisible();

    const pre = dialog.locator('pre.cloud-preview-code');
    await expect(pre).toBeVisible({ timeout: 30_000 });
    await expect(pre).toHaveAttribute('data-language', 'javascript');
    await expect(pre).toContainText(`E2E-HIGHLIGHT-${runId}`);
    await expect(pre.locator('span[class*="hljs-"]').first()).toBeVisible({ timeout: 30_000 });

    await dialog.getByRole('button', { name: 'Закрыть' }).last().click();
    await purgeCard(page, name);
  });

  test('предпросмотр PDF в canvas', async ({ page }) => {
    const name = fileMeta(meta, meta.files.pdf);
    await gotoSignedInCloud(page);
    await uploadFile(page, meta.files.pdf);
    await waitForCloudCard(page, name);

    await openCloudCard(page, name);
    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog.getByText(/Страница 1 из 1/)).toBeVisible({ timeout: 30_000 });

    const canvas = dialog.locator('.cloud-preview-pdf canvas');
    await expect(canvas).toBeVisible({ timeout: 30_000 });
    const rendered = await canvas.evaluate((element) => {
      const node = element as HTMLCanvasElement;
      return { width: node.width, height: node.height };
    });
    expect(rendered.width).toBeGreaterThan(100);
    expect(rendered.height).toBeGreaterThan(100);

    await dialog.getByRole('button', { name: 'Закрыть' }).last().click();
    await purgeCard(page, name);
  });

  test('папка: 2000 файлов и корректный счётчик', async ({ page }) => {
    test.setTimeout(900_000);
    await gotoSignedInCloud(page);
    await uploadFolder(page, meta.files.bulkDir);
    await waitForQueueDone(page, 2000, 720_000);

    await waitForCloudCard(page, meta.files.bulkRootName);
    await openCloudCard(page, meta.files.bulkRootName);
    await expect(page.getByText(/Всего:\s*2000/)).toBeVisible({ timeout: 30_000 });
    await expect(page.getByTestId('cloud-card')).toHaveCount(48);

    await page.goto('/workspace/storage');
    await expect(page.getByRole('heading', { name: 'Cloud Storage' })).toBeVisible();
    await purgeCard(page, meta.files.bulkRootName);
  });

  test('пауза и продолжение загрузки папки', async ({ page }) => {
    test.setTimeout(300_000);
    const rootName = path.basename(meta.files.pauseDir);
    const fileName = path.basename(meta.files.pauseFile);

    let delayed = false;
    await page.route('**/api/v1/cloud/uploads/*', async (route) => {
      if (!delayed && route.request().method() === 'PATCH') {
        delayed = true;
        await new Promise((resolve) => setTimeout(resolve, 1500));
      }
      try {
        await route.continue();
      } catch {
        // Навигация/unroute могли уже обработать маршрут — для теста это не ошибка.
      }
    });

    await gotoSignedInCloud(page);
    await uploadFolder(page, meta.files.pauseDir);

    const row = page.getByTestId('upload-queue-item').filter({ hasText: fileName });
    await expect(row).toBeVisible({ timeout: 30_000 });
    await expect(row).toContainText(/%/, { timeout: 30_000 });
    await row.getByTitle('Пауза').click();
    await expect(row.getByTitle('Продолжить')).toBeVisible({ timeout: 30_000 });

    await row.getByTitle('Продолжить').click();
    await waitForQueueDone(page, 1, 180_000);

    await waitForCloudCard(page, rootName);
    await openCloudCard(page, rootName);
    await waitForCloudCard(page, fileName);
    await page.goto('/workspace/storage');
    await expect(page.getByRole('heading', { name: 'Cloud Storage' })).toBeVisible();
    await purgeCard(page, rootName);
  });

  test('замена файла при конфликте имени', async ({ page }) => {
    const name = fileMeta(meta, meta.files.replace);
    await gotoSignedInCloud(page);
    await uploadFile(page, meta.files.replace);
    await waitForCloudCard(page, name);

    await uploadBuffer(page, name, Buffer.from(`replace-v2 ${runId}\n`, 'utf8'));
    const uploadItem = page
      .locator('.transfer-item')
      .filter({ has: page.getByRole('button', { name: 'Заменить' }) });
    await expect(uploadItem).toContainText('Имя уже занято — выберите действие', {
      timeout: 30_000,
    });
    await uploadItem.getByRole('button', { name: 'Заменить' }).click();
    await expect(page.locator('.transfer-item--done').filter({ hasText: name }).last()).toBeVisible(
      {
        timeout: 30_000,
      },
    );

    const id = await cardEntryId(page, name);
    const response = await page.request.get(
      `${BASE_URL}/api/v1/cloud/entries/${encodeURIComponent(id)}/download`,
    );
    expect(response.status()).toBe(200);
    expect(await response.text()).toContain(`replace-v2 ${runId}`);

    await purgeCard(page, name);
  });

  test('публичная ссылка открывается без сессии и скачивает файл', async ({
    page,
    browser,
    consoleGuard,
  }) => {
    const name = fileMeta(meta, meta.files.share);
    await gotoSignedInCloud(page);
    await uploadFile(page, meta.files.share);
    await waitForCloudCard(page, name);

    await cloudCard(page, name).getByRole('button', { name: 'Создать ссылку' }).click();
    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog.getByText(name, { exact: false }).first()).toBeVisible();
    await dialog.getByRole('button', { name: 'Создать ссылку' }).click();

    const urlInput = dialog.locator('input[readonly]');
    await expect(urlInput).toBeVisible({ timeout: 30_000 });
    const shareUrl = await urlInput.inputValue();
    expect(shareUrl).toContain('/s/');
    await dialog.getByRole('button', { name: 'Готово' }).click();

    const anonymous = await browser.newContext({
      baseURL: BASE_URL,
      locale: 'ru-RU',
    });
    await consoleGuard.attachContext(anonymous);
    try {
      const anonPage = await anonymous.newPage();
      await anonPage.goto(shareUrl);
      await expect(anonPage.getByRole('heading', { name })).toBeVisible({ timeout: 30_000 });
      const downloadPromise = anonPage.waitForEvent('download');
      await anonPage.getByRole('button', { name: /Скачать/ }).click();
      const download = await downloadPromise;
      expect(download.suggestedFilename()).toBe(name);
    } finally {
      await anonymous.close();
    }

    await purgeCard(page, name);
  });

  test('корзина: перемещение и восстановление', async ({ page }) => {
    const name = fileMeta(meta, meta.files.trash);
    await gotoSignedInCloud(page);
    await uploadFile(page, meta.files.trash);
    await waitForCloudCard(page, name);

    await cloudCard(page, name).getByRole('button', { name: 'В корзину' }).click();
    await expect(page.locator('.toast').filter({ hasText: 'Перемещено в корзину' })).toBeVisible({
      timeout: 30_000,
    });
    await expect(cloudCard(page, name)).toHaveCount(0);

    await page.getByRole('tab', { name: 'Корзина' }).click();
    await waitForCloudCard(page, name);
    await cloudCard(page, name).getByRole('button', { name: 'Восстановить' }).click();
    await expect(page.locator('.toast').filter({ hasText: 'Запись восстановлена' })).toBeVisible({
      timeout: 30_000,
    });

    await page.getByRole('tab', { name: 'Мои файлы' }).click();
    await waitForCloudCard(page, name);
    await purgeCard(page, name);
  });
});
