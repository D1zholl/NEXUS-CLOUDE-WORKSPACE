/**
 * Управляемый стенд для E2E Agent Mode:
 * настоящий `nexus-ai-runtime.exe` + локальный OpenAI-compatible stub
 * PolyStrata + реальная БД/backend Playwright.
 *
 * Stub не подменяет интеграцию: WSS, enrollment, receipts, изоляция workspace,
 * approvals и запись файлов выполняются реальным Runtime. Модель — единственная
 * контролируемая часть, чтобы сценарии были детерминированными.
 *
 * Реальная PolyStrata проверяется отдельным сценарием (см. agent-real.spec.ts),
 * который включается переменными E2E_POLYSTRATA_URL/_MODEL и не является
 * mock-тестом.
 */

import { spawn, spawnSync, type ChildProcess } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import path from 'node:path';

import { apiData, openApiSession, type ApiSession } from './api';
import { ADMIN_STATE, BASE_URL, E2E_ROOT, LOGS_DIR, REPO_ROOT } from './config';

export const RUNTIME_BIN = path.join(
  REPO_ROOT,
  'runtime',
  'target',
  'release',
  process.platform === 'win32' ? 'nexus-ai-runtime.exe' : 'nexus-ai-runtime',
);

export const E2E_MODEL = 'e2e-agent-model';
export const E2E_PROVIDER_NAME = 'E2E Agent PolyStrata';

interface RuntimeNode {
  id: string;
  name: string;
  status: string;
}

interface MeResponse {
  user: { id: string; username: string };
  csrfToken: string;
}

export interface AgentHarness {
  api: ApiSession;
  workspaceId: string;
  workspaceName: string;
  workspaceRoot: string;
  nodeId: string;
  nodeName: string;
  providerId: string;
  providerName: string;
  stubUrl: string;
  runtime: ChildProcess;
  stop: () => Promise<void>;
}

interface StubRequest {
  messages?: Array<{ role?: string; content?: string | null }>;
  stream?: boolean;
  tools?: unknown[];
}

function sse(payload: unknown): string {
  return `data: ${JSON.stringify(payload)}\n\n`;
}

function textStream(chunks: string[], usage = 12): string {
  const events = chunks.map((chunk) =>
    sse({ choices: [{ delta: { content: chunk }, finish_reason: null }] }),
  );
  events.push(
    sse({ choices: [{ delta: {}, finish_reason: 'stop' }], usage: { total_tokens: usage } }),
  );
  events.push('data: [DONE]\n\n');
  return events.join('');
}

function toolCallStream(
  id: string,
  name: string,
  args: unknown,
  content: string[] = [],
): string {
  const events = content.map((chunk) =>
    sse({ choices: [{ delta: { content: chunk }, finish_reason: null }] }),
  );
  events.push(
    sse({
      choices: [
        {
          delta: {
            tool_calls: [
              {
                index: 0,
                id,
                type: 'function',
                function: { name, arguments: JSON.stringify(args) },
              },
            ],
          },
          finish_reason: null,
        },
      ],
    }),
  );
  events.push(
    sse({ choices: [{ delta: {}, finish_reason: 'tool_calls' }], usage: { total_tokens: 10 } }),
  );
  events.push('data: [DONE]\n\n');
  return events.join('');
}

const NOTES_PATCH = [
  '--- a/notes.md',
  '+++ b/notes.md',
  '@@ -1,1 +1,2 @@',
  ' line-1',
  '+line-2',
  '',
].join('\n');

interface Decision {
  content?: string[];
  tool?: { id: string; name: string; args: unknown };
  delayMs?: number;
  finishReason?: string;
}

function decide(body: StubRequest): Decision {
  const messages = body.messages ?? [];
  const lastUser = [...messages]
    .reverse()
    .find((message) => message.role === 'user')
    ?.content;
  const prompt = typeof lastUser === 'string' ? lastUser : '';
  const toolResults = messages.filter((message) => message.role === 'tool').length;

  if (prompt.includes('E2E_CANCEL') || prompt.includes('E2E_DISCONNECT')) {
    return { delayMs: 30_000, content: ['слишком поздно'] };
  }
  if (prompt.includes('E2E_RELOAD')) {
    if (toolResults === 0) {
      return {
        delayMs: 1_200,
        content: ['Готовлю файл…'],
        tool: {
          id: 'call_reload',
          name: 'fs_write',
          args: { path: 'reload.txt', content: 'reload-ok\n', overwrite: true },
        },
      };
    }
    return { content: ['Файл после перезагрузки создан.'] };
  }
  if (prompt.includes('E2E_MULTI')) {
    if (toolResults === 0) {
      return {
        content: ['Шаг 1: создаю файл.'],
        tool: {
          id: 'call_multi_1',
          name: 'fs_write',
          args: { path: 'multi.txt', content: 'multi-step\n', overwrite: true },
        },
      };
    }
    if (toolResults === 1) {
      return {
        content: ['Шаг 2: перечитываю файл.'],
        tool: { id: 'call_multi_2', name: 'fs_read', args: { path: 'multi.txt' } },
      };
    }
    return { content: ['Шаги выполнены последовательно.'] };
  }
  if (prompt.includes('E2E_APPROVAL') || prompt.includes('E2E_REJECT')) {
    if (toolResults === 0) {
      return {
        content: ['Нужно изменить notes.md.'],
        tool: { id: 'call_patch', name: 'fs_apply_patch', args: { patch: NOTES_PATCH } },
      };
    }
    return { content: ['Патч применён.'] };
  }
  if (prompt.includes('E2E_STREAM')) {
    if (toolResults === 0) {
      return {
        content: ['Промежуточный шаг: ', 'создаю файл.'],
        tool: {
          id: 'call_stream',
          name: 'fs_write',
          args: { path: 'notes.md', content: 'line-1\n', overwrite: true },
        },
      };
    }
    return { content: ['Файл создан и проверен.'] };
  }
  return { content: ['E2E: задача принята.'] };
}

function startPolyStrataStub(): Promise<{ url: string; close: () => Promise<void> }> {
  return new Promise((resolve, reject) => {
    const server = http.createServer((request, response) => {
      let raw = '';
      request.setEncoding('utf8');
      request.on('data', (chunk: string) => {
        raw += chunk;
      });
      request.on('end', () => {
        const url = request.url ?? '';
        if (url.endsWith('/models')) {
          response.writeHead(200, { 'content-type': 'application/json' });
          response.end(JSON.stringify({ data: [{ id: E2E_MODEL }] }));
          return;
        }
        if (url.endsWith('/health') || url.endsWith('/status')) {
          response.writeHead(200, { 'content-type': 'application/json' });
          response.end(JSON.stringify({ ok: true, version: 'e2e-stub' }));
          return;
        }
        if (!url.endsWith('/chat/completions')) {
          response.writeHead(404, { 'content-type': 'application/json' });
          response.end(JSON.stringify({ error: 'not found' }));
          return;
        }
        let body: StubRequest = {};
        try {
          body = JSON.parse(raw) as StubRequest;
        } catch {
          // Пустое/битое тело: обрабатываем как обычный запрос.
        }
        const decision = decide(body);
        const reply = (): void => {
          if (body.stream) {
            const payload =
              decision.tool !== undefined
                ? toolCallStream(
                    decision.tool.id,
                    decision.tool.name,
                    decision.tool.args,
                    decision.content,
                  )
                : textStream(decision.content ?? ['E2E ответ']);
            response.writeHead(200, {
              'content-type': 'text/event-stream',
              'cache-control': 'no-cache',
              connection: 'close',
            });
            response.end(payload);
            return;
          }
          response.writeHead(200, { 'content-type': 'application/json' });
          response.end(
            JSON.stringify({
              id: `chatcmpl-e2e-${Date.now()}`,
              object: 'chat.completion',
              choices: [
                {
                  index: 0,
                  message: {
                    role: 'assistant',
                    content: decision.tool ? null : (decision.content ?? []).join(''),
                    tool_calls: decision.tool
                      ? [
                          {
                            id: decision.tool.id,
                            type: 'function',
                            function: {
                              name: decision.tool.name,
                              arguments: JSON.stringify(decision.tool.args),
                            },
                          },
                        ]
                      : undefined,
                  },
                  finish_reason: decision.finishReason ?? (decision.tool ? 'tool_calls' : 'stop'),
                },
              ],
              usage: { total_tokens: 12 },
            }),
          );
        };
        if (decision.delayMs) {
          setTimeout(reply, decision.delayMs);
        } else {
          reply();
        }
      });
    });
    server.on('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      if (!address || typeof address === 'string') {
        reject(new Error('stub PolyStrata: некорректный адрес'));
        return;
      }
      resolve({
        url: `http://127.0.0.1:${address.port}/v1`,
        close: () =>
          new Promise<void>((done) => {
            server.close(() => done());
          }),
      });
    });
  });
}

async function waitForNode(api: ApiSession, name: string, timeoutMs = 40_000): Promise<RuntimeNode> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const page = await apiData<{ items: RuntimeNode[] }>(
      api,
      'GET',
      '/ai/admin/runtime-nodes',
    );
    const node = page.items.find((item) => item.name === name);
    if (node?.status === 'online') return node;
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`Runtime ${name} не подключился за ${timeoutMs} мс`);
}

export async function startAgentHarness(runTag: string): Promise<AgentHarness> {
  if (!fs.existsSync(RUNTIME_BIN)) {
    throw new Error(
      `Не найден ${RUNTIME_BIN}. Соберите Runtime: cd runtime; cargo build --release`,
    );
  }
  fs.mkdirSync(LOGS_DIR, { recursive: true });
  const root = path.join(E2E_ROOT, 'agent', runTag);
  const workspaceRoot = path.join(root, 'workspace');
  const stateDir = path.join(root, 'state');
  fs.mkdirSync(workspaceRoot, { recursive: true });
  fs.mkdirSync(stateDir, { recursive: true });

  const api = await openApiSession(ADMIN_STATE);
  const me = await apiData<MeResponse>(api, 'GET', '/auth/me');

  const stub = await startPolyStrataStub();
  const nodeName = `E2E-AGENT-${runTag}`;
  const configPath = path.join(root, 'runtime.toml');
  fs.writeFileSync(
    configPath,
    `[server]
url = "${BASE_URL.replace(/^http/, 'ws')}/api/v1/ai/runtime/connect"
enroll_url = "${BASE_URL}"
name = "${nodeName}"
allow_insecure_ws = true
heartbeat_seconds = 5
reconnect_min_seconds = 1
reconnect_max_seconds = 2

[storage]
state_dir = ${JSON.stringify(stateDir)}
allowed_roots = [${JSON.stringify(root)}]
receipts_max_bytes = 1048576
receipts_max_archives = 4

[provider]
base_url = "${stub.url}"
timeout_seconds = 30
connect_timeout_seconds = 5

[logging]
level = "info"
`,
    'utf8',
  );

  const enrollment = await apiData<{ code: string }>(api, 'POST', '/ai/admin/enrollment-codes', {
    json: { label: nodeName, ttlSeconds: 900 },
  });
  const enroll = spawnSync(
    RUNTIME_BIN,
    ['--config', configPath, 'enroll', '--code', enrollment.code, '--name', nodeName],
    { encoding: 'utf8', timeout: 120_000, windowsHide: true },
  );
  if (enroll.status !== 0) {
    await stub.close();
    throw new Error(
      `enroll не выполнен: ${enroll.status}\nstdout: ${enroll.stdout}\nstderr: ${enroll.stderr}`,
    );
  }

  const logOut = fs.openSync(path.join(LOGS_DIR, `agent-runtime-${runTag}.log`), 'a');
  const logErr = fs.openSync(path.join(LOGS_DIR, `agent-runtime-${runTag}.err.log`), 'a');
  const runtime = spawn(RUNTIME_BIN, ['--config', configPath, 'run'], {
    cwd: root,
    windowsHide: true,
    stdio: ['ignore', logOut, logErr],
  });

  const node = await waitForNode(api, nodeName);

  const provider = await apiData<{ id: string }>(api, 'POST', '/ai/admin/providers', {
    json: {
      name: `${E2E_PROVIDER_NAME} ${runTag}`,
      baseUrl: stub.url,
      defaultModel: E2E_MODEL,
      runtimeNodeId: node.id,
      enabled: true,
    },
  });

  // New workspace: allowedTools не задаём — backend обязан выдать безопасный
  // набор по умолчанию (включая fs.stat и skills.list).
  const workspace = await apiData<{
    id: string;
    allowedTools: string[];
    missingSafeTools?: string[];
  }>(api, 'POST', '/ai/admin/workspaces', {
    json: {
      ownerId: me.user.id,
      runtimeNodeId: node.id,
      name: `E2E Agent WS ${runTag}`,
      rootPath: workspaceRoot,
      allowedPrograms: [],
      maxParallelTasks: 1,
      maxTaskSeconds: 300,
    },
  });
  for (const required of ['fs.stat', 'skills.list', 'fs.apply_patch', 'fs.write', 'fs.read']) {
    if (!workspace.allowedTools.includes(required)) {
      throw new Error(`новый workspace не содержит ${required}: ${workspace.allowedTools.join(', ')}`);
    }
  }
  if ((workspace.missingSafeTools ?? []).length > 0) {
    throw new Error(
      `новый workspace уже отстаёт от безопасного набора: ${workspace.missingSafeTools?.join(', ')}`,
    );
  }

  const harness: AgentHarness = {
    api,
    workspaceId: workspace.id,
    workspaceName: `E2E Agent WS ${runTag}`,
    workspaceRoot,
    nodeId: node.id,
    nodeName,
    providerId: provider.id,
    providerName: `${E2E_PROVIDER_NAME} ${runTag}`,
    stubUrl: stub.url,
    runtime,
    stop: async () => {
      await stopRuntime(harness);
      await stub.close();
      await api.ctx.dispose().catch(() => undefined);
      try {
        fs.closeSync(logOut);
        fs.closeSync(logErr);
      } catch {
        // ignore
      }
    },
  };
  return harness;
}

export async function stopRuntime(harness: AgentHarness): Promise<void> {
  harness.runtime.kill();
  await new Promise<void>((resolve) => {
    if (harness.runtime.exitCode !== null) {
      resolve();
      return;
    }
    harness.runtime.once('exit', () => resolve());
    setTimeout(resolve, 5_000);
  });
}

export async function startRuntime(harness: AgentHarness, runTag: string): Promise<void> {
  // Тот же state/device.json: повторный enroll не нужен.
  const root = path.dirname(harness.workspaceRoot);
  const configPath = path.join(root, 'runtime.toml');
  const logOut = fs.openSync(path.join(LOGS_DIR, `agent-runtime-${runTag}.log`), 'a');
  const logErr = fs.openSync(path.join(LOGS_DIR, `agent-runtime-${runTag}.err.log`), 'a');
  harness.runtime = spawn(RUNTIME_BIN, ['--config', configPath, 'run'], {
    cwd: root,
    windowsHide: true,
    stdio: ['ignore', logOut, logErr],
  });
}
