import path from 'node:path';

import { defineConfig, devices } from '@playwright/test';

import { BASE_URL, FRONTEND_DIR, serverEnv } from './e2e/fixtures/config';

const npm = process.platform === 'win32' ? 'npm.cmd' : 'npm';

export default defineConfig({
  testDir: path.join(FRONTEND_DIR, 'e2e'),
  testMatch: '**/*.spec.ts',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 90_000,
  expect: { timeout: 15_000 },
  outputDir: path.join(FRONTEND_DIR, 'test-results'),
  reporter: [
    ['list'],
    ['html', { open: 'never', outputFolder: path.join(FRONTEND_DIR, 'playwright-report') }],
  ],
  globalSetup: path.join(FRONTEND_DIR, 'e2e', 'fixtures', 'global-setup.ts'),
  use: {
    baseURL: BASE_URL,
    locale: 'ru-RU',
    timezoneId: 'Europe/Moscow',
    viewport: { width: 1440, height: 900 },
    acceptDownloads: true,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    actionTimeout: 15_000,
    navigationTimeout: 30_000,
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
  webServer: {
    command: `${npm} run e2e:server`,
    cwd: FRONTEND_DIR,
    url: `${BASE_URL}/api/v1/health`,
    reuseExistingServer: false,
    timeout: 180_000,
    stdout: 'pipe',
    stderr: 'pipe',
    env: serverEnv(),
  },
});
