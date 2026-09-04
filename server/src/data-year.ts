import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

function readDatasetYearConfig(): string {
  // The DATA_YEAR env var (set via .env / systemd) wins; the committed
  // scripts/dataset-year.json config is the checkout default. A distribution
  // without that config file (fresh dev4 checkout) serves the current year
  // rather than crashing every route import — the file, when present, still
  // decides exactly as before.
  if (process.env.DATA_YEAR) return process.env.DATA_YEAR
  try {
    return JSON.parse(
      readFileSync(resolve(import.meta.dirname, '..', '..', 'scripts', 'dataset-year.json'), 'utf-8'),
    ).current_year
  } catch {
    return '2026'
  }
}

// Dataset year — ONE source of truth. Never hardcode a year fallback in
// route files; import this instead.
export const DATA_YEAR: string = readDatasetYearConfig()
