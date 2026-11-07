import { defineConfig } from '@playwright/test';
export default defineConfig({
  testDir: './tests/ui',
  fullyParallel: true,
  timeout: 30_000,
  use: {
    baseURL: 'http://127.0.0.1:1420',
    viewport: { width: 1240, height: 840 },
    colorScheme: 'light',
    launchOptions: { executablePath: process.env.PLAYWRIGHT_CHROME_PATH ?? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' },
    screenshot: 'only-on-failure',
  },
  webServer: { command: 'npm --prefix ui run dev', url: 'http://127.0.0.1:1420', reuseExistingServer: !process.env.CI },
});
