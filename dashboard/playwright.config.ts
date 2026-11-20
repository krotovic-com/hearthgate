import { defineConfig } from '@playwright/test';

// End-to-end tests against a real Hearthgate started by e2e/global-setup.ts.
export default defineConfig({
  testDir: 'e2e',
  globalSetup: './e2e/global-setup.ts',
  globalTeardown: './e2e/global-teardown.ts',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: process.env.CI ? 'github' : 'list',
  use: {
    ignoreHTTPSErrors: true,
    viewport: { width: 1440, height: 1000 },
    trace: 'off',
  },
});
