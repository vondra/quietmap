import { existsSync } from 'node:fs'
import { defineConfig } from '@playwright/test'

const realMode = process.env.E2E_MODE === 'real'
const realBaseUrl = process.env.E2E_REAL_BASE_URL
if (realMode && !realBaseUrl) {
  throw new Error('E2E_REAL_BASE_URL is required for npm run e2e:real')
}

// GitHub's Ubuntu image and the development hosts already carry Chrome. Use it
// instead of downloading a second ~browser-sized artifact; fall back to
// Playwright's managed Chromium on other machines.
const systemChrome = process.env.PLAYWRIGHT_CHROME
  ?? (existsSync('/usr/bin/google-chrome') ? '/usr/bin/google-chrome' : undefined)

export default defineConfig({
  testDir: './e2e',
  outputDir: './test-results',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: realMode ? 60_000 : 20_000,
  expect: { timeout: 10_000 },
  forbidOnly: !!process.env.CI,
  reporter: process.env.CI ? 'github' : 'list',
  use: {
    baseURL: realMode ? realBaseUrl : 'http://127.0.0.1:4173',
    browserName: 'chromium',
    viewport: { width: 1280, height: 720 },
    deviceScaleFactor: 1,
    contextOptions: { reducedMotion: 'reduce' },
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
    video: 'off',
    launchOptions: systemChrome ? { executablePath: systemChrome } : undefined,
  },
  webServer: realMode ? undefined : {
    command: 'npm run preview:check',
    url: 'http://127.0.0.1:4173',
    // Never attach to a preview started from another checkout. The command's
    // --strictPort then turns an occupied port into a visible test failure.
    reuseExistingServer: false,
    timeout: 30_000,
  },
})
