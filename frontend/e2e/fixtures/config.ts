import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

/**
 * Общие константы E2E. Все изменяемые данные вынесены за пределы рабочего
 * дерева в короткий корень `C:\nx-e2e`: длинные пути Cloud/Minecraft легко
 * превышают MAX_PATH на Windows.
 */
export const FRONTEND_DIR = path.resolve(fileURLToPath(new URL('.', import.meta.url)), '../..');
export const REPO_ROOT = path.resolve(FRONTEND_DIR, '..');
export const BACKEND_DIR = path.join(REPO_ROOT, 'backend');
export const NEXUS_EXE = path.join(BACKEND_DIR, 'target', 'release', 'nexus.exe');

export const E2E_ROOT = process.env.E2E_ROOT ?? 'C:\\nx-e2e';
export const VOLUME_DIR = path.join(E2E_ROOT, 'volume');
export const FIXTURES_DIR = path.join(E2E_ROOT, 'fixtures');
export const RUNS_DIR = path.join(E2E_ROOT, 'runs');
export const LOGS_DIR = path.join(E2E_ROOT, 'logs');

export const AUTH_DIR = path.join(FRONTEND_DIR, 'e2e', '.auth');
export const ADMIN_STATE = path.join(AUTH_DIR, 'admin.json');
export const USER_STATE = path.join(AUTH_DIR, 'user.json');
export const META_FILE = path.join(AUTH_DIR, 'fixtures.json');

export const SERVER_PORT = Number(process.env.E2E_PORT ?? 8091);
export const BASE_URL = process.env.E2E_BASE_URL ?? `http://127.0.0.1:${SERVER_PORT}`;

export const ADMIN_USERNAME = process.env.E2E_ADMIN_USERNAME ?? 'e2e-admin';
export const ADMIN_PASSWORD = process.env.E2E_ADMIN_PASSWORD ?? 'Nexus!E2E-admin-2026';
export const USER_USERNAME = process.env.E2E_USER_USERNAME ?? 'e2e-user';
export const USER_PASSWORD = process.env.E2E_USER_PASSWORD ?? 'Nexus!E2E-user-2026';
export const LOCK_USERNAME = process.env.E2E_LOCK_USERNAME ?? 'e2e-lock';
export const LOCK_PASSWORD = process.env.E2E_LOCK_PASSWORD ?? 'Nexus!E2E-lock-2026';

export const VOLUME_NAME = 'E2E Fixtures';

export interface E2EMeta {
  runId: string;
  createdAt: string;
  user: { id: string; username: string; password: string };
  lockUser: { id: string; username: string; password: string };
  volume: { id: string | null; name: string; path: string };
  files: {
    pdf: string;
    highlight: string;
    share: string;
    replace: string;
    trash: string;
    bulkDir: string;
    bulkRootName: string;
    pauseDir: string;
    pauseFile: string;
    largeFileBytes: number;
  };
  fixtures: {
    bulkCount: number;
    bulkGenerateMs: number;
    bulkGeneratedNow: boolean;
    bulkLinkMs: number;
    defenderNote: string;
  };
}

export function readMeta(): E2EMeta {
  if (!fs.existsSync(META_FILE)) {
    throw new Error(`Не найден ${META_FILE}. Playwright globalSetup должен выполниться до тестов.`);
  }
  return JSON.parse(fs.readFileSync(META_FILE, 'utf8')) as E2EMeta;
}

/** Минимальный .env-парсер: без зависимостей и без переопределения process.env. */
export function readDotEnv(file: string): Record<string, string> {
  const result: Record<string, string> = {};
  if (!fs.existsSync(file)) return result;
  for (const rawLine of fs.readFileSync(file, 'utf8').split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line || line.startsWith('#')) continue;
    const eq = line.indexOf('=');
    if (eq <= 0) continue;
    const key = line.slice(0, eq).trim();
    let value = line.slice(eq + 1).trim();
    if (
      (value.startsWith('"') && value.endsWith('"')) ||
      (value.startsWith("'") && value.endsWith("'"))
    ) {
      value = value.slice(1, -1);
    }
    result[key] = value;
  }
  return result;
}

export function serverEnv(): Record<string, string> {
  const fileEnv = readDotEnv(path.join(REPO_ROOT, '.env'));
  const pick = (key: string, fallback: string): string =>
    process.env[key] ?? fileEnv[key] ?? fallback;

  return {
    DATABASE_URL: pick('DATABASE_URL', 'postgres://nexus:nexus@127.0.0.1:5432/nexus'),
    NEXUS_SECRET: pick('NEXUS_SECRET', 'e2e-secret-e2e-secret-e2e-secret-2026'),
    NEXUS_HOST: '127.0.0.1',
    NEXUS_PORT: String(SERVER_PORT),
    NEXUS_PUBLIC_URL: BASE_URL,
    NEXUS_COOKIE_SECURE: 'false',
    NEXUS_STATIC_DIR: path.join(FRONTEND_DIR, 'dist'),
    NEXUS_LOG_DIR: LOGS_DIR,
    NEXUS_LOG_FILE: 'nexus-e2e.log',
    NEXUS_LOG_KEEP: '3',
    RUST_LOG: process.env.RUST_LOG ?? 'nexus=info,tower_http=warn,sqlx=warn',
  };
}
