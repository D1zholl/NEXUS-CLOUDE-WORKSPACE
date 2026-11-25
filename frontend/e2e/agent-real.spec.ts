/**
 * Интеграционная проверка с **реальной** PolyStrata и реальным AI Runtime.
 *
 * Включается только при наличии внешнего OpenAI-compatible endpoint:
 *
 *   E2E_POLYSTRATA_URL=http://127.0.0.1:8080/v1
 *   E2E_POLYSTRATA_MODEL=<имя модели>
 *   E2E_POLYSTRATA_API_KEY=<ключ, если нужен>
 *
 * Без этих переменных тест помечается skipped, а не подменяется mock-ответом.
 * Это единственная проверка, которую нельзя выполнить в CI без установленной
 * PolyStrata; она не заменяет прогон на двух физических ПК.
 */

import fs from 'node:fs';
import path from 'node:path';

import { ADMIN_STATE } from './fixtures/config';
import {
  RUNTIME_BIN,
  startAgentHarness,
  type AgentHarness,
} from './fixtures/agentRuntime';
import { apiData, uniqueTag } from './fixtures/api';
import { expect, test } from './fixtures/test';

interface TaskRow {
  id: string;
  status: string;
  errorCode?: string | null;
}

interface MessageRow {
  role: string;
  content: string;
  status: string;
}

const polyUrl = process.env.E2E_POLYSTRATA_URL;
const polyModel = process.env.E2E_POLYSTRATA_MODEL ?? 'default';
const polyKey = process.env.E2E_POLYSTRATA_API_KEY ?? '';
const runTag = uniqueTag('real-');

let harness: AgentHarness;
let providerName: string;

test.describe.configure({ mode: 'serial' });
test.use({ storageState: ADMIN_STATE });

test.describe('Agent Mode с реальной PolyStrata', () => {
  test.skip(!RUNTIME_BIN || !fs.existsSync(RUNTIME_BIN), 'нет собранного AI Runtime');
  test.skip(!polyUrl, 'E2E_POLYSTRATA_URL не задан — реальная PolyStrata недоступна');
  test.setTimeout(240_000);

  test.beforeAll(async () => {
    harness = await startAgentHarness(runTag);
    providerName = `Real PolyStrata ${runTag}`;
    await apiData(harness.api, 'POST', '/ai/admin/providers', {
      json: {
        name: providerName,
        baseUrl: polyUrl,
        apiKey: polyKey || undefined,
        defaultModel: polyModel,
        runtimeNodeId: harness.nodeId,
        enabled: true,
      },
    });
    fs.writeFileSync(path.join(harness.workspaceRoot, 'probe.txt'), 'NEXUS-REAL-POLYSTRATA\n', 'utf8');
  });

  test.afterAll(async () => {
    if (harness) await harness.stop();
  });

  test('выполняет Agent-задачу на реальной модели и реальном Runtime', async ({ page }) => {
    await page.goto('/workspace/ai');
    await expect(page.getByRole('heading', { level: 1, name: 'AI Workspace' })).toBeVisible();
    await page.getByRole('button', { name: 'Новый диалог', exact: true }).click();
    await page
      .getByRole('group', { name: 'Режим AI' })
      .getByRole('button', { name: 'Agent' })
      .click();
    const selects = page.locator('.ai-selectors select');
    await selects.nth(0).selectOption({ label: providerName });
    await page.locator('.ai-selectors input').fill(polyModel);
    await selects.nth(1).selectOption(harness.workspaceId);

    await page
      .locator('textarea.ai-textarea')
      .fill('Прочитай файл probe.txt через fs_read и кратко перескажи его содержимое.');
    await page.locator('textarea.ai-textarea').press('Enter');
    await expect(page).toHaveURL(/[?&]c=/, { timeout: 30_000 });
    const conversationId = new URL(page.url()).searchParams.get('c')!;

    const deadline = Date.now() + 180_000;
    let task: TaskRow | undefined;
    while (Date.now() < deadline) {
      const pageData = await apiData<{ items: TaskRow[] }>(
        harness.api,
        'GET',
        `/ai/tasks?conversationId=${conversationId}&perPage=5`,
      );
      task = pageData.items[0];
      if (task && ['completed', 'failed', 'cancelled'].includes(task.status)) break;
      await new Promise((resolve) => setTimeout(resolve, 500));
    }
    expect(task, 'задача должна появиться').toBeTruthy();
    // Реальная модель обязана завершить задачу без ошибок протокола.
    expect(task?.status, JSON.stringify(task)).toBe('completed');
    expect(task?.errorCode ?? null).toBeNull();

    const messages = await apiData<{ items: MessageRow[] }>(
      harness.api,
      'GET',
      `/ai/conversations/${conversationId}/messages`,
    );
    const assistant = messages.items.filter((item) => item.role === 'assistant');
    expect(assistant.length).toBeGreaterThan(0);
    expect(assistant.some((item) => item.content.trim().length > 0)).toBe(true);
  });
});
