import { defineConfig, devices } from '@playwright/test'

const fullstackBaseUrl = process.env.E2E_BASE_URL

export default defineConfig({
  testDir: './e2e',
  testMatch: fullstackBaseUrl ? /fullstack\.spec\.ts/ : undefined,
  testIgnore: fullstackBaseUrl ? undefined : /fullstack\.spec\.ts/,
  fullyParallel: !fullstackBaseUrl,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [['list'], ['html', { open: 'never' }]] : 'list',
  use: {
    baseURL: fullstackBaseUrl ?? 'http://127.0.0.1:4173',
    viewport: { width: 1280, height: 800 },
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
  webServer: fullstackBaseUrl
    ? undefined
    : {
        command: 'npm run build && npm run preview -- --host 127.0.0.1 --port 4173 --strictPort',
        url: 'http://127.0.0.1:4173',
        reuseExistingServer: !process.env.CI,
        timeout: 60_000,
      },
})
