import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests/e2e',
  testMatch: '*.e2e.ts',
  outputDir: './test-results',
  fullyParallel: false,
  retries: 0,
  workers: 1,

  reporter: [['list'], ['html', { open: 'never' }]],

  use: {
    baseURL: 'https://localhost:3940',
    ignoreHTTPSErrors: true,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },

  webServer: {
    command: 'cargo run',
    url: 'https://localhost:3940',
    ignoreHTTPSErrors: true,
    timeout: 120_000,
    reuseExistingServer: true,
    env: {
      DEN_PASSWORD: 'e2e-test-pass',
      DEN_PORT: '3940',
      DEN_DATA_DIR: './data-e2e',
      DEN_BIND_ADDRESS: '127.0.0.1',
    },
  },

  projects: [{ name: 'chromium' }],
});
