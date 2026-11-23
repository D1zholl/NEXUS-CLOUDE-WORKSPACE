import { defineConfig, devices } from '@playwright/test';

import baseConfig from './playwright.config';

/**
 * K4: проверка целевых браузеров Windows — установленных Chrome и Edge.
 *
 * Обычный `npx playwright test` использует bundled Chromium, чтобы прогон не
 * зависел от браузеров на образе. Этот конфиг запускает те же сценарии в
 * системных каналах:
 *
 *   npm run e2e:browsers -- e2e/system.spec.ts
 */
export default defineConfig({
  ...baseConfig,
  projects: [
    {
      name: 'chrome',
      use: { ...devices['Desktop Chrome'], channel: 'chrome' },
    },
    {
      name: 'msedge',
      use: { ...devices['Desktop Chrome'], channel: 'msedge' },
    },
  ],
});
