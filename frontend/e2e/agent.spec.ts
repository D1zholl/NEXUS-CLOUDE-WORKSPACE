/**
 * Полный E2E Agent Mode: UI + настоящий backend + настоящий
 * `nexus-ai-runtime.exe` + контролируемый OpenAI-compatible stub PolyStrata.
 *
 * Сценарии соответствуют ТЗ §5: создание диалога, Agent Mode, streaming,
 * tool calling, approval (approve/reject), результат инструмента, продолжение
 * цикла, файлы и diff, отмена, F5, восстановление истории, отключение Runtime.
 *
 * Реальная PolyStrata проверяется отдельным файлом `agent-real.spec.ts`.
 */

import fs from 'node:fs';
import path from 'node:path';

import { ADMIN_STATE } from './fixtures/config';
import {
  E2E_MODEL,
  RUNTIME_BIN,
  startAgentHarness,
  startRuntime,
  stopRuntime,
  type AgentHarness,
} from './fixtures/agentRuntime';
import { apiData, apiFetch, uniqueTag } from './fixtures/api';
import { expect, test } from './fixtures/test';

interface TaskRow {
  id: string;
  status: string;
  cancelStatus?: string | null;
  errorCode?: string | null;
  iterations: number;
}

interface ExecutionRow {
  id: string;
  toolId: string;
  status: string;
  resultText?: string | null;
  changedFiles?: Array<{ path: string; action: string; additions: number; deletions: number }>;
  diff?: string | null;
}

interface EventRow {
  id: number;
  kind: string;
}

let harness: AgentHarness;
const runTag = uniqueTag('agent-');
const runtimeMissing = !fs.existsSync(RUNTIME_BIN);

test.describe.configure({ mode: 'serial' });
test.use({ storageState: ADMIN_STATE });

async function tasks(conversationId: string): Promise<TaskRow[]> {
  const page = await apiData<{ items: TaskRow[] }>(
    harness.api,
    'GET',
    `/ai/tasks?conversationId=${conversationId}&perPage=20`,
  );
  return page.items;
}

async function waitTask(
  conversationId: string,
  predicate: (task: TaskRow) => boolean,
  timeoutMs = 60_000,
): Promise<TaskRow> {
  const deadline = Date.now() + timeoutMs;
  let last: TaskRow | undefined;
  while (Date.now() < deadline) {
    const items = await tasks(conversationId);
    last = items[0];
    if (last && predicate(last)) return last;
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(
    `задача не достигла ожидаемого состояния; последняя: ${JSON.stringify(last)}`,
  );
}

async function executions(taskId: string): Promise<ExecutionRow[]> {
  const page = await apiData<{ items: ExecutionRow[] }>(
    harness.api,
    'GET',
    `/ai/tasks/${taskId}/executions`,
  );
  return page.items;
}

async function events(taskId: string): Promise<EventRow[]> {
  const page = await apiData<{ items: EventRow[] }>(
    harness.api,
    'GET',
    `/ai/tasks/${taskId}/events?perPage=200`,
  );
  return page.items;
}

function conversationId(pageUrl: string): string {
  const id = new URL(pageUrl).searchParams.get('c');
  if (!id) throw new Error(`в URL нет ?c=: ${pageUrl}`);
  return id;
}

async function prepareAgent(page: import('@playwright/test').Page): Promise<void> {
  await page.goto('/workspace/ai');
  await expect(page.getByRole('heading', { level: 1, name: 'AI Workspace' })).toBeVisible();
  await page.getByRole('button', { name: 'Новый диалог', exact: true }).click();
  await page.getByRole('group', { name: 'Режим AI' }).getByRole('button', { name: 'Agent' }).click();
  await expect(page.locator('textarea.ai-textarea')).toBeVisible();
  const selects = page.locator('.ai-selectors select');
  await selects.nth(0).selectOption({ label: harness.providerName });
  await page.locator('.ai-selectors input').fill(E2E_MODEL);
  await selects.nth(1).selectOption(harness.workspaceId);
}

async function send(page: import('@playwright/test').Page, prompt: string): Promise<string> {
  await page.locator('textarea.ai-textarea').fill(prompt);
  await page.locator('textarea.ai-textarea').press('Enter');
  await expect(page).toHaveURL(/[?&]c=/, { timeout: 20_000 });
  return conversationId(page.url());
}

test.describe('Agent Mode E2E (реальный Runtime)', () => {
  test.skip(runtimeMissing, `нет собранного Runtime: ${RUNTIME_BIN}`);
  test.setTimeout(180_000);

  test.beforeAll(async () => {
    harness = await startAgentHarness(runTag);
  });

  test.afterAll(async () => {
    if (harness) await harness.stop();
  });

  test('создаёт диалог, включает Agent Mode и показывает streaming + tool call', async ({
    page,
  }) => {
    await prepareAgent(page);
    const id = await send(page, 'E2E_STREAM создай файл notes.md');

    // Промежуточный текст появляется до завершения задачи (streaming).
    await expect(page.getByText('Промежуточный шаг', { exact: false })).toBeVisible({
      timeout: 30_000,
    });

    const task = await waitTask(id, (item) => item.status === 'completed');
    expect(task.errorCode).toBeNull();
    expect(task.iterations).toBeGreaterThanOrEqual(2);

    const file = path.join(harness.workspaceRoot, 'notes.md');
    expect(fs.existsSync(file)).toBe(true);
    expect(fs.readFileSync(file, 'utf8')).toBe('line-1\n');

    const calls = await executions(task.id);
    const write = calls.find((item) => item.toolId === 'fs.write');
    expect(write?.status).toBe('completed');
    expect(write?.resultText?.length ?? 0).toBeGreaterThan(0);

    await expect(page.getByText('Файл создан и проверен.')).toBeVisible({ timeout: 20_000 });
    await expect(page.getByText('fs.write', { exact: false }).first()).toBeVisible();
    await expect(page.locator('.ai-task__status')).toContainText('Завершена');
  });

  test('показывает approval, применяет патч после подтверждения и отображает diff', async ({
    page,
  }) => {
    await prepareAgent(page);
    const id = await send(page, 'E2E_APPROVAL измени notes.md');

    const approval = page.locator('.ai-approval').filter({ hasText: 'fs.apply_patch' });
    await expect(approval).toBeVisible({ timeout: 30_000 });
    await expect(approval).toContainText('Требуется подтверждение');

    // До подтверждения side effect не выполнен.
    const file = path.join(harness.workspaceRoot, 'notes.md');
    expect(fs.readFileSync(file, 'utf8')).toBe('line-1\n');

    await approval.getByRole('button', { name: 'Подтвердить' }).click();
    const task = await waitTask(id, (item) => item.status === 'completed');
    expect(fs.readFileSync(file, 'utf8')).toContain('line-2');

    const calls = await executions(task.id);
    const patch = calls.find((item) => item.toolId === 'fs.apply_patch');
    expect(patch?.status).toBe('completed');
    expect(patch?.diff).toContain('+line-2');
    expect(patch?.changedFiles?.some((entry) => entry.path === 'notes.md')).toBe(true);

    await expect(page.getByText('Изменённые файлы')).toBeVisible({ timeout: 20_000 });
    await expect(page.locator('.ai-changed-file').filter({ hasText: 'notes.md' })).toBeVisible();
    await page.locator('.ai-diff summary').click();
    await expect(page.locator('.ai-diff')).toContainText('+line-2');
  });

  test('отклонение approval отменяет задачу и не выполняет действие', async ({ page }) => {
    await prepareAgent(page);
    const file = path.join(harness.workspaceRoot, 'notes.md');
    const before = fs.readFileSync(file, 'utf8');
    const id = await send(page, 'E2E_REJECT попробуй изменить notes.md');

    const approval = page.locator('.ai-approval').filter({ hasText: 'fs.apply_patch' });
    await expect(approval).toBeVisible({ timeout: 30_000 });
    await approval.getByRole('button', { name: 'Отклонить' }).click();

    const task = await waitTask(id, (item) => item.status === 'cancelled');
    expect(task.cancelStatus === 'confirmed' || task.cancelStatus === 'none').toBe(true);
    expect(fs.readFileSync(file, 'utf8')).toBe(before);

    const calls = await executions(task.id);
    expect(calls.every((item) => item.status !== 'completed')).toBe(true);

    await expect(page.locator('.ai-task__status')).toContainText('Отменена', {
      timeout: 20_000,
    });
    await page.getByRole('button', { name: /События/ }).click();
    await expect(page.getByText('ai.task.cancelled').first()).toBeVisible();
  });

  test('отменяет выполняющуюся задачу', async ({ page }) => {
    await prepareAgent(page);
    const id = await send(page, 'E2E_CANCEL длинная операция');
    await waitTask(id, (item) => item.status === 'running' || item.status === 'queued');
    await expect(page.locator('.ai-task__status')).toContainText('Выполняется', {
      timeout: 20_000,
    });

    await page.getByRole('button', { name: 'Отменить' }).click();
    const task = await waitTask(id, (item) => item.status === 'cancelled');
    expect(task.cancelStatus).toBe('confirmed');
    await expect(page.locator('.ai-task__status')).toContainText('Отменена', {
      timeout: 20_000,
    });
  });

  test('переживает F5 во время выполнения и восстанавливает историю и события', async ({
    page,
  }) => {
    await prepareAgent(page);
    const id = await send(page, 'E2E_RELOAD создай reload.txt');

    // Обновляем страницу, пока задача ещё выполняется (stub задерживает ответ).
    await page.reload();
    await expect(page.getByRole('heading', { level: 1, name: 'AI Workspace' })).toBeVisible();
    await expect(page).toHaveURL(new RegExp(`c=${id}`));
    await expect(page.locator('.ai-task')).toBeVisible({ timeout: 30_000 });

    const task = await waitTask(id, (item) => item.status === 'completed');
    expect(fs.readFileSync(path.join(harness.workspaceRoot, 'reload.txt'), 'utf8')).toBe(
      'reload-ok\n',
    );

    await expect(page.getByText('Файл после перезагрузки создан.')).toBeVisible({
      timeout: 30_000,
    });
    await page.getByRole('button', { name: /События/ }).click();
    await expect(page.getByText('ai.task.started').first()).toBeVisible();
    await expect(page.getByText('ai.tool.completed').first()).toBeVisible();

    const stored = await events(task.id);
    const ids = stored.map((event) => event.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect([...ids].sort((left, right) => left - right)).toEqual(ids);
  });

  test('выполняет несколько последовательных инструментов в одном запуске', async ({ page }) => {
    await prepareAgent(page);
    const id = await send(page, 'E2E_MULTI выполни два шага');
    const task = await waitTask(id, (item) => item.status === 'completed');
    expect(task.iterations).toBeGreaterThanOrEqual(3);

    const calls = await executions(task.id);
    expect(calls.map((item) => item.toolId)).toEqual(['fs.write', 'fs.read']);
    expect(calls.every((item) => item.status === 'completed')).toBe(true);
    expect(fs.readFileSync(path.join(harness.workspaceRoot, 'multi.txt'), 'utf8')).toBe(
      'multi-step\n',
    );
    await expect(page.getByText('Шаги выполнены последовательно.')).toBeVisible({
      timeout: 20_000,
    });
  });

  test('контролируемо завершает задачу при отключённом Runtime и восстанавливает соединение', async ({
    page,
  }) => {
    await stopRuntime(harness);
    // Backend замечает разрыв по heartbeat/сокету.
    await expect
      .poll(
        async () => {
          const pageData = await apiData<{ items: Array<{ id: string; status: string }> }>(
            harness.api,
            'GET',
            '/ai/admin/runtime-nodes',
          );
          return pageData.items.find((item) => item.id === harness.nodeId)?.status;
        },
        { timeout: 30_000 },
      )
      .not.toBe('online');

    await page.goto('/workspace/ai');
    await expect(page.getByRole('heading', { level: 1, name: 'AI Workspace' })).toBeVisible();
    // Backend обязан отказать контролируемо и НЕ создавать осиротевшую задачу.
    const conversation = await apiData<{ id: string }>(harness.api, 'POST', '/ai/conversations', {
      json: {
        mode: 'agent',
        model: E2E_MODEL,
        workspaceId: harness.workspaceId,
        providerId: harness.providerId,
      },
    });
    const rejected = await apiFetch(
      harness.api,
      'POST',
      `/ai/conversations/${conversation.id}/messages`,
      { json: { content: 'E2E_DISCONNECT проверка отказа' } },
    );
    expect(rejected.status()).toBe(503);
    const rejectedBody = (await rejected.json()) as {
      error: { code: string; message: string };
    };
    expect(rejectedBody.error.code).toBe('AI_RUNTIME_OFFLINE');
    expect(await tasks(conversation.id)).toHaveLength(0);

    // Повторный запуск процесса использует существующий device.json.
    await startRuntime(harness, runTag);
    await expect
      .poll(
        async () => {
          const pageData = await apiData<{ items: Array<{ id: string; status: string }> }>(
            harness.api,
            'GET',
            '/ai/admin/runtime-nodes',
          );
          return pageData.items.find((item) => item.id === harness.nodeId)?.status;
        },
        { timeout: 40_000 },
      )
      .toBe('online');

    // После reconnect новая задача снова проходит полный цикл.
    await prepareAgent(page);
    const nextId = await send(page, 'E2E_MULTI после reconnect');
    const recovered = await waitTask(nextId, (item) => item.status === 'completed');
    expect(recovered.errorCode ?? null).toBeNull();
  });
});
