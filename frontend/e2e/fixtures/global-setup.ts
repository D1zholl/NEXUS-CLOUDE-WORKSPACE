import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

import { chromium, request, type APIRequestContext } from '@playwright/test';

import {
  ADMIN_PASSWORD,
  ADMIN_STATE,
  ADMIN_USERNAME,
  AUTH_DIR,
  BASE_URL,
  E2E_ROOT,
  FIXTURES_DIR,
  LOCK_PASSWORD,
  LOCK_USERNAME,
  LOGS_DIR,
  META_FILE,
  NEXUS_EXE,
  RUNS_DIR,
  USER_PASSWORD,
  USER_STATE,
  USER_USERNAME,
  VOLUME_DIR,
  VOLUME_NAME,
  BACKEND_DIR,
  type E2EMeta,
  serverEnv,
} from './config';

interface LoginResult {
  ctx: APIRequestContext;
  csrf: string;
}

function ensureDir(dir: string): void {
  fs.mkdirSync(dir, { recursive: true });
}

async function tryLogin(username: string, password: string): Promise<LoginResult> {
  const ctx = await request.newContext({
    baseURL: BASE_URL,
    extraHTTPHeaders: { Accept: 'application/json' },
  });
  const response = await ctx.post('/api/v1/auth/login', {
    data: { username, password },
  });
  if (!response.ok()) {
    const text = await response.text().catch(() => '');
    await ctx.dispose();
    throw new Error(`login ${username}: HTTP ${response.status()} ${text}`);
  }
  const payload = (await response.json()) as {
    data: { csrfToken: string; user: { id: string; username: string } };
  };
  return { ctx, csrf: payload.data.csrfToken };
}

function createAdmin(): void {
  if (!fs.existsSync(NEXUS_EXE)) {
    throw new Error(
      `Не найден release-бинарник ${NEXUS_EXE}. Соберите его: cargo build --release (в backend).`,
    );
  }
  const env = {
    ...process.env,
    ...serverEnv(),
    E2E_ADMIN_PASSWORD: ADMIN_PASSWORD,
  };
  const result = spawnSync(
    NEXUS_EXE,
    ['create-admin', '--username', ADMIN_USERNAME, '--password-env', 'E2E_ADMIN_PASSWORD'],
    {
      cwd: BACKEND_DIR,
      env,
      encoding: 'utf8',
      timeout: 120_000,
      windowsHide: true,
    },
  );
  if (result.status !== 0) {
    throw new Error(
      `Не удалось создать E2E-администратора ${ADMIN_USERNAME}.\n` +
        `stdout: ${result.stdout ?? ''}\nstderr: ${result.stderr ?? ''}\n` +
        `Если пользователь уже существует с другим паролем, задайте ` +
        `E2E_ADMIN_PASSWORD (и при необходимости E2E_ADMIN_USERNAME).`,
    );
  }
  console.log(`[e2e] создан администратор ${ADMIN_USERNAME}`);
}

async function ensureAdmin(): Promise<LoginResult> {
  try {
    return await tryLogin(ADMIN_USERNAME, ADMIN_PASSWORD);
  } catch (error) {
    console.log(
      `[e2e] вход администратора не удался (${(error as Error).message}); ` +
        'пробую создать через CLI',
    );
    createAdmin();
    return await tryLogin(ADMIN_USERNAME, ADMIN_PASSWORD);
  }
}

async function api<T>(
  ctx: APIRequestContext,
  csrf: string,
  method: 'GET' | 'POST' | 'PATCH' | 'DELETE',
  url: string,
  data?: unknown,
): Promise<T> {
  const headers: Record<string, string> = { Accept: 'application/json' };
  if (method !== 'GET') {
    headers['X-CSRF-Token'] = csrf;
    if (data !== undefined) headers['Content-Type'] = 'application/json';
  }
  const response = await ctx.fetch(url, {
    method,
    headers,
    data: data === undefined ? undefined : JSON.stringify(data),
  });
  const text = await response.text();
  if (!response.ok()) {
    throw new Error(`${method} ${url}: HTTP ${response.status()} ${text}`);
  }
  const payload = text ? (JSON.parse(text) as { data: T }) : { data: undefined as T };
  return payload.data;
}

async function ensureVolume(admin: LoginResult): Promise<{ id: string | null; path: string }> {
  ensureDir(VOLUME_DIR);
  const real = fs.realpathSync.native(VOLUME_DIR);
  const volumes = await api<
    Array<{ id: string; name: string; rootPath: string; enabled: boolean }>
  >(admin.ctx, admin.csrf, 'GET', '/api/v1/storage/volumes');
  const existing = volumes.find(
    (volume) => path.resolve(volume.rootPath).toLowerCase() === real.toLowerCase(),
  );
  if (existing) return { id: existing.id, path: existing.rootPath };

  const created = await api<{ id: string; rootPath: string }>(
    admin.ctx,
    admin.csrf,
    'POST',
    '/api/v1/storage/volumes',
    {
      name: VOLUME_NAME,
      path: real,
      mode: 'auto',
      priority: 50,
      enabled: true,
    },
  );
  console.log(`[e2e] создан том ${created.id} (${created.rootPath})`);
  return { id: created.id, path: created.rootPath };
}

async function ensureUser(
  admin: LoginResult,
  username: string,
  password: string,
  displayName: string,
): Promise<{ id: string; username: string }> {
  const page = await api<{ items: Array<{ id: string; username: string }> }>(
    admin.ctx,
    admin.csrf,
    'GET',
    `/api/v1/users?perPage=100&q=${encodeURIComponent(username)}`,
  );
  let user = page.items.find((item) => item.username.toLowerCase() === username.toLowerCase());

  if (!user) {
    const created = await api<{ id: string; username: string }>(
      admin.ctx,
      admin.csrf,
      'POST',
      '/api/v1/users',
      {
        username,
        displayName,
        password,
        roleIds: [],
        status: 'active',
      },
    );
    user = { id: created.id, username: created.username };
    console.log(`[e2e] создан пользователь ${username} (${user.id})`);
  } else {
    await api(admin.ctx, admin.csrf, 'PATCH', `/api/v1/users/${user.id}`, {
      status: 'active',
    });
  }

  // Идемпотентность: пароль и блокировка всегда приводятся к известному
  // состоянию, чтобы повторный запуск не зависел от прошлых прогонов.
  await api(admin.ctx, admin.csrf, 'POST', `/api/v1/users/${user.id}/password`, { password });
  await api(admin.ctx, admin.csrf, 'POST', `/api/v1/users/${user.id}/unlock`, {});
  return user;
}

function writeText(file: string, content: string): void {
  ensureDir(path.dirname(file));
  fs.writeFileSync(file, content, 'utf8');
}

function generateBulkFixture(): {
  dir: string;
  count: number;
  generateMs: number;
  generatedNow: boolean;
} {
  const dir = path.join(FIXTURES_DIR, 'bulk-2000');
  const expected = 2000;
  const marker = path.join(dir, '.complete');
  if (fs.existsSync(marker)) {
    return { dir, count: expected, generateMs: 0, generatedNow: false };
  }

  fs.rmSync(dir, { recursive: true, force: true });
  ensureDir(dir);
  const started = performance.now();
  for (let index = 0; index < expected; index += 1) {
    const number = String(index + 1).padStart(4, '0');
    const nested = index % 20 === 0 ? `sub-${number}` : '';
    const target = nested
      ? path.join(dir, nested, `file-${number}.txt`)
      : path.join(dir, `file-${number}.txt`);
    if (nested) ensureDir(path.dirname(target));
    writeText(
      target,
      `NEXUS E2E bulk fixture #${number}\n` +
        `row=${number}\n` +
        `checksum-seed=${(index * 2654435761) >>> 0}\n` +
        `The quick brown fox jumps over the lazy dog.\n`,
    );
  }
  writeText(marker, String(Date.now()));
  const generateMs = Math.round(performance.now() - started);
  return { dir, count: expected, generateMs, generatedNow: true };
}

function linkBulkFixture(runId: string, sourceDir: string): { dir: string; linkMs: number } {
  const rootName = `bulk-2000-${runId}`;
  const destination = path.join(RUNS_DIR, runId, rootName);
  fs.rmSync(destination, { recursive: true, force: true });
  ensureDir(destination);

  const started = performance.now();
  const walk = (from: string, to: string): void => {
    for (const entry of fs.readdirSync(from, { withFileTypes: true })) {
      if (entry.name.startsWith('.')) continue;
      const source = path.join(from, entry.name);
      const target = path.join(to, entry.name);
      if (entry.isDirectory()) {
        ensureDir(target);
        walk(source, target);
        continue;
      }
      try {
        fs.linkSync(source, target);
      } catch {
        fs.copyFileSync(source, target);
      }
    }
  };
  walk(sourceDir, destination);
  return { dir: destination, linkMs: Math.round(performance.now() - started) };
}

function ensureLargeFile(): { file: string; bytes: number } {
  const bytes = 70 * 1024 * 1024;
  const file = path.join(FIXTURES_DIR, 'large-70mb.bin');
  if (!fs.existsSync(file) || fs.statSync(file).size !== bytes) {
    ensureDir(path.dirname(file));
    const buffer = Buffer.alloc(bytes, 0xa5);
    fs.writeFileSync(file, buffer);
  }
  return { file, bytes };
}

async function generatePdf(target: string, runId: string): Promise<void> {
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage();
    await page.setContent(`<!doctype html>
      <html lang="ru"><head><meta charset="utf-8">
      <style>
        body { font-family: Arial, sans-serif; padding: 48px; color: #102030; }
        h1 { color: #0a5c8a; }
        .box { border: 2px solid #58d4ff; border-radius: 12px; padding: 24px; }
      </style></head>
      <body>
        <div class="box">
          <h1>NEXUS E2E PDF</h1>
          <p>Run: ${runId}</p>
          <p>Проверка рендера PDF.js в &lt;canvas&gt;.</p>
          <p>Page 1 of 1</p>
        </div>
      </body></html>`);
    await page.pdf({ path: target, format: 'A4', printBackground: true });
  } finally {
    await browser.close();
  }
}

async function globalSetup(): Promise<void> {
  ensureDir(AUTH_DIR);
  ensureDir(E2E_ROOT);
  ensureDir(FIXTURES_DIR);
  ensureDir(RUNS_DIR);
  ensureDir(LOGS_DIR);

  const admin = await ensureAdmin();
  const volume = await ensureVolume(admin);

  const user = await ensureUser(admin, USER_USERNAME, USER_PASSWORD, 'E2E User');
  const lockUser = await ensureUser(admin, LOCK_USERNAME, LOCK_PASSWORD, 'E2E Lock User');

  const runId = `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`;

  const bulk = generateBulkFixture();
  const bulkRun = linkBulkFixture(runId, bulk.dir);
  const bulkRootName = path.basename(bulkRun.dir);

  const large = ensureLargeFile();
  const pauseRootName = `pause-${runId}`;
  const pauseDir = path.join(RUNS_DIR, runId, pauseRootName);
  ensureDir(pauseDir);
  const pauseFile = path.join(pauseDir, `large-${runId}.bin`);
  try {
    fs.linkSync(large.file, pauseFile);
  } catch {
    fs.copyFileSync(large.file, pauseFile);
  }

  const filesDir = path.join(FIXTURES_DIR, `run-${runId}`);
  ensureDir(filesDir);
  const pdf = path.join(filesDir, `preview-${runId}.pdf`);
  const highlight = path.join(filesDir, `preview-${runId}.js`);
  const share = path.join(filesDir, `share-${runId}.txt`);
  const replace = path.join(filesDir, `replace-${runId}.txt`);
  const trash = path.join(filesDir, `trash-${runId}.txt`);

  await generatePdf(pdf, runId);
  writeText(
    highlight,
    `// NEXUS E2E highlight ${runId}\n` +
      `export function nexusPreview(file) {\n` +
      `  const checksum = "sha256";\n` +
      `  const marker = "E2E-HIGHLIGHT-${runId}";\n` +
      `  return { file, checksum, marker, ok: true };\n` +
      `}\n`,
  );
  writeText(share, `NEXUS E2E share ${runId}\nПубличная ссылка Cloud Storage.\n`);
  writeText(replace, `replace-v1 ${runId}\n`);
  writeText(trash, `trash ${runId}\n`);

  // Выделенный E2E-администратор тоже не рисует 3D-фон: System-сценарии
  // открывают 13 страниц в headless Chromium, а SwiftShader здесь не нужен.
  const adminSettings = await api<Record<string, unknown>>(
    admin.ctx,
    admin.csrf,
    'GET',
    '/api/v1/me/settings',
  );
  const adminBackground =
    (adminSettings['appearance.background'] as Record<string, unknown> | undefined) ?? {};
  await api(admin.ctx, admin.csrf, 'PUT', '/api/v1/me/settings/appearance.background', {
    ...adminBackground,
    enabled: false,
  });

  const adminState = await admin.ctx.storageState();
  await admin.ctx.dispose();
  fs.writeFileSync(ADMIN_STATE, JSON.stringify(adminState, null, 2));

  const userLogin = await tryLogin(USER_USERNAME, USER_PASSWORD);
  // Headless Chromium рисует 3D-фон через SwiftShader и заметно замедляет
  // 2000-файловую загрузку. Фон не относится к I1 (Cloud/вход), поэтому для
  // выделенного E2E-пользователя отключаем его; unit-тесты фона не затронуты.
  const mySettings = await api<Record<string, unknown>>(
    userLogin.ctx,
    userLogin.csrf,
    'GET',
    '/api/v1/me/settings',
  );
  const background =
    (mySettings['appearance.background'] as Record<string, unknown> | undefined) ?? {};
  await api(userLogin.ctx, userLogin.csrf, 'PUT', '/api/v1/me/settings/appearance.background', {
    ...background,
    enabled: false,
  });
  fs.writeFileSync(USER_STATE, JSON.stringify(await userLogin.ctx.storageState(), null, 2));
  await userLogin.ctx.dispose();

  const meta: E2EMeta = {
    runId,
    createdAt: new Date().toISOString(),
    user: { id: user.id, username: USER_USERNAME, password: USER_PASSWORD },
    lockUser: { id: lockUser.id, username: LOCK_USERNAME, password: LOCK_PASSWORD },
    volume,
    files: {
      pdf,
      highlight,
      share,
      replace,
      trash,
      bulkDir: bulkRun.dir,
      bulkRootName,
      pauseDir,
      pauseFile,
      largeFileBytes: large.bytes,
    },
    fixtures: {
      bulkCount: bulk.count,
      bulkGenerateMs: bulk.generateMs,
      bulkGeneratedNow: bulk.generatedNow,
      bulkLinkMs: bulkRun.linkMs,
      defenderNote: bulk.generatedNow
        ? `2000 файлов созданы за ${bulk.generateMs} мс; отдельное исключение Defender не задавалось.`
        : 'Готовый каталог 2000 файлов переиспользован; время генерации сохранено в отчёте.',
    },
  };
  fs.writeFileSync(META_FILE, JSON.stringify(meta, null, 2), 'utf8');

  console.log(
    `[e2e] run=${runId}; fixtures: ${bulk.count} files, generate=${bulk.generateMs} ms, ` +
      `hardlink=${bulkRun.linkMs} ms; volume=${volume.path}`,
  );
}

export default globalSetup;
