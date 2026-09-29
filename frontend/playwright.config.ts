// Browser tests of the built map against `vite preview`; every API answer is mocked in the page.
import { existsSync } from 'node:fs'
import { defineConfig } from '@playwright/test'

// Use an installed Chrome when there is one instead of downloading a second
// browser; fall back to Playwright's managed Chromium otherwise.
const systemChrome = process.env.PLAYWRIGHT_CHROME
  ?? (existsSync('/usr/bin/google-chrome') ? '/usr/bin/google-chrome' : undefined)

export default defineConfig({
  testDir: './e2e',
  outputDir: './test-results',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 20_000,
  expect: { timeout: 10_000 },
  forbidOnly: !!process.env.CI,
  reporter: process.env.CI ? 'github' : 'list',
  use: {
    baseURL: 'http://127.0.0.1:4173',
    browserName: 'chromium',
    viewport: { width: 1280, height: 720 },
    deviceScaleFactor: 1,
    contextOptions: { reducedMotion: 'reduce' },
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
    video: 'off',
    launchOptions: systemChrome ? { executablePath: systemChrome } : undefined,
  },
  webServer: {
    command: 'npx vite preview --outDir dist.check --host 127.0.0.1 --port 4173 --strictPort',
    url: 'http://127.0.0.1:4173',
    // Never attach to a preview started from another checkout. --strictPort
    // then turns an occupied port into a visible test failure.
    reuseExistingServer: false,
    timeout: 30_000,
  },
})
