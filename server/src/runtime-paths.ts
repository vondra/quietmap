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
// Prepared vectors for one dataset year: `<prepared-year>/z9/<x>/<y>/`
// holds the per-square arrows (roads, railways, structures, industrial,
// leisure, airborne, cruise, airport_traffic, airport_lines),
// `<prepared-year>/admin/<square_id>/admin.bin` the admin records
// (`square_id` = Morton z-order, `grid::square_id` — the only integer id), and
// `<prepared-year>/rasters/` the DEM/land rasters. The source-reader native
// addon is initialized against this directory (source_init). NO H3 anywhere.
export const PREPARED_YEAR_DIR = process.env.PREPARED_YEAR_DIR
  ? resolve(process.env.PREPARED_YEAR_DIR)
  : resolve(REPO_ROOT, 'data', 'prepared', DATA_YEAR)
