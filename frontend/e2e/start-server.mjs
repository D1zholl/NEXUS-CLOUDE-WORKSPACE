import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

/**
 * Запуск release-сборки backend для Playwright webServer.
 *
 * Скрипт вызывается через `npm.cmd run e2e:server` (Windows), не через bash.
 * `.env` читается самим backend из backend/, переменные Playwright имеют
 * приоритет (dotenvy не переопределяет уже заданное окружение).
 */
const here = path.dirname(fileURLToPath(import.meta.url));
const frontendDir = path.resolve(here, '..');
const repoRoot = path.resolve(frontendDir, '..');
const backendDir = path.join(repoRoot, 'backend');
const exe = path.join(
  backendDir,
  'target',
  'release',
  process.platform === 'win32' ? 'nexus.exe' : 'nexus',
);

if (!fs.existsSync(exe)) {
  console.error(
    `[e2e] release-бинарник не найден: ${exe}\n` +
      '[e2e] выполните: cd backend; cargo build --release',
  );
  process.exit(1);
}

const logDir = process.env.NEXUS_LOG_DIR;
if (logDir) fs.mkdirSync(logDir, { recursive: true });

console.log(`[e2e] старт release backend: ${exe}`);
console.log(
  `[e2e] ${process.env.NEXUS_HOST ?? '127.0.0.1'}:${process.env.NEXUS_PORT ?? '8080'}, ` +
    `static=${process.env.NEXUS_STATIC_DIR ?? '(default)'}`,
);

const child = spawn(exe, ['serve'], {
  cwd: backendDir,
  env: process.env,
  stdio: 'inherit',
  windowsHide: true,
});

let stopping = false;
function stop(signal) {
  if (stopping) return;
  stopping = true;
  try {
    child.kill(signal);
  } catch {
    // Процесс уже завершился.
  }
}

process.on('SIGINT', () => stop('SIGINT'));
process.on('SIGTERM', () => stop('SIGTERM'));
process.on('exit', () => stop('SIGTERM'));

child.on('error', (error) => {
  console.error(`[e2e] не удалось запустить backend: ${error.message}`);
  process.exit(1);
});

child.on('exit', (code, signal) => {
  if (signal) console.error(`[e2e] backend остановлен сигналом ${signal}`);
  process.exit(code ?? (signal ? 1 : 0));
});
