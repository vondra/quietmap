// Runtime paths derived from one stable anchor. `src/` and compiled `dist/`
// occupy the same depth below server/, so these resolve identically in both
// execution modes.
import { existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { DATA_YEAR } from './data-year.js'

export const REPO_ROOT = resolve(import.meta.dirname, '..', '..')

// The built SPA: a release bundles it next to its own code (start.sh passes
// --frontend-dir), a plain checkout serves ../frontend/dist. Release-local wins
// when present — it is the copy start.sh built for THIS release. ONE place for
// this lookup: server.ts serves it, runtime-readiness.ts gates on it.
export const FRONTEND_DIST = process.env.FRONTEND_DIST
  ? resolve(process.env.FRONTEND_DIST)
  : existsSync(resolve(import.meta.dirname, 'frontend', 'index.html'))
    ? resolve(import.meta.dirname, 'frontend')
    : resolve(REPO_ROOT, 'frontend', 'dist')
const bundledSourceReader = resolve(import.meta.dirname, 'native/libsource_reader.so')
export const SOURCE_READER_PATH = existsSync(bundledSourceReader) ? bundledSourceReader : resolve(
  REPO_ROOT,
  'engine/target/release/libsource_reader.so',
)
export const H3R4_DIR = process.env.H3R4_DIR
  ? resolve(process.env.H3R4_DIR)
  : resolve(REPO_ROOT, 'data', 'prepared', DATA_YEAR, 'h3r4')
