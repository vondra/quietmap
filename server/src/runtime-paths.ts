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
// One immutable prepared year: z9/x/y contains Arrows, structures, admin.bin
// and native DEM/forest/IMD files; rasters.sqlite and inputs.sqlite bind its generation.
export const PREPARED_YEAR_DIR = process.env.PREPARED_YEAR_DIR
  ? resolve(process.env.PREPARED_YEAR_DIR)
  : resolve(REPO_ROOT, 'data', 'prepared', DATA_YEAR)

// Optional retained corner generation from an approved repaint. Native receipt
// validation requires the same prepared sources, rasters and compiled physics.
export const SURFACE_CORNERS_DIR = process.env.SURFACE_CORNERS_DIR
  ? resolve(process.env.SURFACE_CORNERS_DIR)
  : null
