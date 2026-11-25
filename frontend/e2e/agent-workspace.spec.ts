/**
 * E2E Agent Workspace (Agent IDE): реальный Runtime + backend + контролируемый
 * stub PolyStrata. Проверяет Explorer/Editor, сохранение с проверкой версии,
 * конфликт, изменения агента, diff, безопасный откат, Task Manager и
 * мобильное переключение панелей.
 */

import fs from 'node:fs';
import path from 'node:path';

import { ADMIN_STATE } from './fixtures/config';
import {
  E2E_MODEL,
  RUNTIME_BIN,
  startAgentHarness,
  type AgentHarness,
} from './fixtures/agentRuntime';
import { uniqueTag } from './fixtures/api';
import { expect, test } from './fixtures/test';

const runTag = uniqueTag('agentide-');
const runtimeMissing = !fs.existsSync(RUNTIME_BIN);
const PNG_1X1 =
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=';

let harness: AgentHarness;

test.describe.configure({ mode: 'serial' });
test.use({ storageState: ADMIN_STATE });

test.describe('AI Agent Workspace E2E', () => {
  test.skip(runtimeMissing, `нет собранного Runtime: ${RUNTIME_BIN}`);
  test.setTimeout(180_000);

  test.beforeAll(async () => {
    harness = await startAgentHarness(runTag);
    fs.mkdirSync(path.join(harness.workspaceRoot, 'src'), { recursive: true });
    fs.writeFileSync(path.join(harness.workspaceRoot, 'README.md'), '# Seed\n\nhello\n');
    fs.writeFileSync(
      path.join(harness.workspaceRoot, 'src', 'app.ts'),
      'export const x = 1;\n',
    );
    fs.writeFileSync(
      path.join(harness.workspaceRoot, 'logo.png'),
      Buffer.from(PNG_1X1, 'base64'),
    );
  });

  test.afterAll(async () => {
    if (harness) await harness.stop();
  });

  async function openWorkspace(
    page: import('@playwright/test').Page,
    expectTree = true,
  ) {
    // Предварительно выбираем workspace harness, чтобы страница не делала
    // запрос к первому (возможно offline) workspace из общего E2E DB.
    await page.addInitScript((id) => {
      window.localStorage.setItem('nexus.ai.agent.workspace', id);
    }, harness.workspaceId);
    await page.goto('/workspace/ai/agent');
    const switcher = page.locator('.ws-switcher__button');
    await expect(switcher).toBeVisible({ timeout: 30_000 });
    await expect(switcher).toContainText(harness.workspaceName, { timeout: 30_000 });
    if (!expectTree) return;
    await expect(page.locator('.ws-node', { hasText: 'README.md' })).toBeVisible({
      timeout: 30_000,
    });
  }

  async function selectProvider(page: import('@playwright/test').Page) {
    await page.getByRole('button', { name: 'Панели' }).click();
    const dialog = page.locator('.modal');
    await expect(dialog).toBeVisible();
    await dialog.locator('select').nth(0).selectOption({ label: harness.providerName });
    await dialog.locator('select').nth(1).selectOption({ label: E2E_MODEL });
    await dialog.getByRole('button', { name: 'Готово' }).click();
    await expect(dialog).toBeHidden();
  }

  test('Explorer, Editor, сохранение и preview изображения', async ({ page }) => {
    await openWorkspace(page);
    await expect(page.locator('.ai-agent-layout__explorer')).toBeVisible();
    await expect(page.locator('.editor-pane')).toBeVisible();
    await expect(page.locator('.agent-pane')).toBeVisible();

    await page.locator('.ws-node', { hasText: 'README.md' }).click();
    const editor = page.locator('.code-editor__input');
    await expect(editor).toBeVisible();
    await expect(editor).toHaveValue(/# Seed/);

    await editor.fill('# Saved by E2E\n\neditor works\n');
    await page.locator('.editor-pane__actions').getByRole('button', { name: 'Сохранить' }).click();
    await expect
      .poll(
        () => fs.readFileSync(path.join(harness.workspaceRoot, 'README.md'), 'utf8'),
        { timeout: 20_000 },
      )
      .toContain('editor works');

    // Каталог и вложенный файл открываются по клику.
    await page.locator('.ws-node', { hasText: 'src' }).first().click();
    await expect(page.locator('.ws-node', { hasText: 'app.ts' })).toBeVisible();
    await page.locator('.ws-node', { hasText: 'app.ts' }).click();
    await expect(page.locator('.code-editor__input')).toHaveValue('export const x = 1;\n');

    // Изображение читается через /files/raw.
    await page.locator('.ws-node', { hasText: 'logo.png' }).click();
    await expect(page.locator('.media-viewer__image img')).toBeVisible({ timeout: 20_000 });
  });

  test('конфликт версий не перезаписывает внешние изменения', async ({ page }) => {
    await openWorkspace(page);
    await page.locator('.ws-node', { hasText: 'README.md' }).click();
    const editor = page.locator('.code-editor__input');
    await expect(editor).toHaveValue(/# Saved by E2E/);

    await editor.fill('# My local edit\n');
    fs.writeFileSync(
      path.join(harness.workspaceRoot, 'README.md'),
      '# External change\n',
      'utf8',
    );
    await page.locator('.editor-pane__actions').getByRole('button', { name: 'Сохранить' }).click();

    await expect(page.locator('.editor-conflict')).toBeVisible({ timeout: 20_000 });
    await expect(page.locator('.editor-conflict')).toContainText('Файл изменён');
    expect(fs.readFileSync(path.join(harness.workspaceRoot, 'README.md'), 'utf8')).toBe(
      '# External change\n',
    );

    // Принимаем версию Runtime — буфер обновляется без потери данных на диске.
    await page.locator('.editor-conflict').getByRole('button', { name: 'Принять версию Runtime' }).click();
    await expect(page.locator('.code-editor__input')).toHaveValue('# External change\n');
  });

  test('изменения агента, diff, безопасный откат и Task Manager', async ({ page }) => {
    await openWorkspace(page);
    await selectProvider(page);

    await page.locator('.agent-composer textarea').fill('E2E_RELOAD');
    await page.locator('.agent-composer textarea').press('Enter');
    await expect(page.locator('.agent-status').getByText('Завершена')).toBeVisible({
      timeout: 90_000,
    });
    await expect
      .poll(() => fs.existsSync(path.join(harness.workspaceRoot, 'reload.txt')), {
        timeout: 20_000,
      })
      .toBe(true);

    // Снимок изменения доступен в нижней панели.
    await page
      .locator('.bottom-panel__tabs')
      .getByRole('button', { name: /Changed Files/ })
      .click();
    const row = page.locator('.changed-file', { hasText: 'reload.txt' });
    await expect(row).toBeVisible({ timeout: 20_000 });
    await expect(row.locator('.changed-file__add')).toContainText('+');

    await row.getByTitle('Показать diff').click();
    const modal = page.locator('.modal');
    await expect(modal.locator('.diff-viewer')).toContainText('reload-ok');
    await modal.locator('.modal__foot').getByRole('button', { name: 'Закрыть' }).click();
    await expect(modal).toBeHidden();

    // Откат: файл был создан агентом, поэтому восстановление удаляет его.
    await row.getByTitle('Восстановить версию до изменения').click();
    await page.locator('.modal').getByRole('button', { name: 'Восстановить' }).click();
    await expect
      .poll(() => fs.existsSync(path.join(harness.workspaceRoot, 'reload.txt')), {
        timeout: 30_000,
      })
      .toBe(false);

    // Task Manager: старая задача открывается без повторного запуска.
    await page.getByRole('button', { name: 'Задачи', exact: true }).click();
    const manager = page.locator('.modal');
    await expect(manager.locator('.task-manager')).toBeVisible();
    const item = manager.locator('.task-item', { hasText: 'E2E_RELOAD' }).first();
    await expect(item).toBeVisible({ timeout: 20_000 });
    await item.click();
    await expect(manager.locator('.task-details')).toContainText('Обзор');
    await manager.getByRole('button', { name: 'Открыть в панели' }).click();
    await expect(manager).toBeHidden();
    await expect(page.locator('.agent-pane')).toContainText('история');
  });

  test('пользователь создаёт workspace через UI (Runtime выделяет каталог)', async ({ page }) => {
    await openWorkspace(page);
    await page.locator('.ws-switcher__button').click();
    await page.getByRole('button', { name: 'Новый workspace' }).click();
    const dialog = page.getByRole('dialog', { name: 'Новый workspace' });
    const createdName = `E2E Created ${runTag}`;
    await dialog.locator('input').first().fill(createdName);
    await dialog.locator('textarea').fill('workspace из E2E');
    await dialog.getByRole('button', { name: 'Создать' }).click();
    await expect(page.getByRole('dialog', { name: 'Новый workspace' })).toBeHidden({
      timeout: 30_000,
    });
    await expect(page.locator('.ws-switcher__button')).toContainText(createdName, {
      timeout: 30_000,
    });
    await expect(page.locator('.ws-tree__empty')).toBeVisible({ timeout: 30_000 });

    await page.getByTitle('Новый файл').click();
    const createDialog = page.getByRole('dialog', { name: 'Новый файл' });
    await expect(createDialog).toBeVisible();
    await createDialog.locator('input').nth(1).fill('hello.txt');
    await createDialog.getByRole('button', { name: 'Создать' }).click();
    await expect(page.locator('.ws-node', { hasText: 'hello.txt' })).toBeVisible({
      timeout: 20_000,
    });

    // Каталог создан Runtime: provisioning root = родитель workspace harness.
    const provisioningRoot = path.dirname(harness.workspaceRoot);
    await expect
      .poll(
        () =>
          fs.readdirSync(provisioningRoot).some((name) => {
            if (!/^[0-9a-f-]{36}$/.test(name)) return false;
            return fs.existsSync(path.join(provisioningRoot, name, 'hello.txt'));
          }),
        { timeout: 20_000 },
      )
      .toBe(true);
  });

  test('мобильное переключение Explorer/Editor/Agent', async ({ page }) => {
    await page.setViewportSize({ width: 820, height: 900 });
    await openWorkspace(page, false);
    const tabs = page.locator('.ai-agent-mobile-tabs');
    await expect(tabs).toBeVisible();
    await tabs.getByRole('button', { name: 'Explorer' }).click();
    await expect(page.locator('.ai-agent-layout__explorer')).toBeVisible();
    await expect(page.locator('.ws-node', { hasText: 'README.md' })).toBeVisible({
      timeout: 30_000,
    });
    await tabs.getByRole('button', { name: 'Agent' }).click();
    await expect(page.locator('.ai-agent-layout__agent')).toBeVisible();
    await tabs.getByRole('button', { name: 'Editor' }).click();
    await expect(page.locator('.editor-pane')).toBeVisible();
  });
});
