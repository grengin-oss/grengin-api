// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { defineConfig, devices } from '@playwright/test';

// The stack (Postgres, grengin-api, webapp, mock IdP/LLM/MCP, catalog) is started by
// stack/inside.sh before this runs; specs share one database, so they run serially.
export default defineConfig({
  testDir: './specs',
  fullyParallel: false,
  workers: 1,
  retries: process.env.CI ? 1 : 0,
  timeout: 60_000,
  expect: { timeout: 15_000 },
  reporter: [['list'], ['html', { open: 'never', outputFolder: 'report' }], ['junit', { outputFile: 'report/junit.xml' }]],
  globalSetup: './support/global-setup.ts',
  use: {
    baseURL: process.env.E2E_WEB_URL ?? 'http://127.0.0.1:4173',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    video: 'retain-on-failure',
    viewport: { width: 1366, height: 900 },
  },
  projects: [
    { name: 'login', testMatch: /login\.setup\.ts/ },
    {
      name: 'app',
      dependencies: ['login'],
      use: { ...devices['Desktop Chrome'], storageState: 'state/owner.json' },
    },
  ],
});
