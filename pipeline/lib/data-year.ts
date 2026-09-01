import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

// Dataset year — ONE source of truth: the DATA_YEAR env var overrides, else
// the committed scripts/dataset-year.json. Enrichment scripts import this
// instead of hardcoding a year fallback (a bare `npx tsx pipeline/…` then
// always tracks the committed year).
export const DATA_YEAR: string =
  process.env.DATA_YEAR ||
  JSON.parse(readFileSync(resolve(import.meta.dirname, '..', '..', 'scripts', 'dataset-year.json'), 'utf-8')).current_year

/** The H3-res-4 extract tree of DATA_YEAR — every enricher reads and patches it in place. */
export const H3R4_DIR: string = resolve(import.meta.dirname, '..', '..', 'data', 'prepared', DATA_YEAR, 'h3r4')
