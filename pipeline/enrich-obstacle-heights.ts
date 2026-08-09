/**
 * Height-ladder enricher for the vector obstacle store — the manifest face of
 * `scripts/obstacles/enrich-obstacle-heights.py` (which does the raster work).
 *
 * The worker deterministically REGENERATES `prepared/{year}/h3r4/<cell>/obstacles.arrow`
 * from the immutable Overture staging shards + height rasters (never from its
 * own previous output — gg 2026-08-09), applying the tier ladder (full table:
 * the worker header and `noise_compute::low_profile`):
 *   tier 3  city/national measured per-building zonal (regional 1 m raster; now IPR Praha)
 *   tier 4  areal prior — GHS-BUILT-H ANBH 100 m at the centroid (replaces flat 8 m only)
 *
 * Scope: `--bbox S,W,N,E` selects PROMOTED cells (obstacles.arrow present)
 * whose H3 BOUNDARY overlaps the bbox — centre-only tests silently drop the
 * cells a regional raster clips (gg finding); `--cells hex,...` is
 * verification isolation (never in a chain run). Cells staged but never
 * promoted are skipped loudly — promotion policy stays with the world cutover.
 *
 * `--enrich-only` skips the cache download (`scripts/obstacles/download-height-rasters.sh`:
 * GHSL ANBH GeoTIFF + the IPR Praha exportImage tiles stamped EPSG:5514 +
 * mosaic.vrt + a Žižkov-tower control-point validation). The GHSL raster is
 * required; a regional raster is optional per REGIONAL_RASTERS — cells not
 * overlapping any region run ANBH-only, and per-row zonal coverage decides
 * inside overlapping cells.
 *
 * Re-run contract: an OSM planet re-extract does NOT touch obstacles.arrow, so
 * routine post-extract chains have nothing to redo (manifest lists this step
 * with a permanent skip, like `global-buildings`). After an Overture re-ingest
 * + re-promotion, re-run manually:
 *   npx tsx pipeline/enrich-obstacle-heights.ts --enrich-only [--bbox S,W,N,E]
 *
 * Registry: `global-ghsl-built-h` (9866) + `cz-ipr-praha-vysky` (9867) in
 * enrichment-datasets.ts — obstacle rows carry `height_tier`, not `source_id`;
 * the entries document license/URL/rank for docs and attribution.
 */

import { spawnSync } from 'node:child_process'
import { existsSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { cellToBoundary } from 'h3-js'
import { DATA_YEAR } from './lib/data-year.js'

const REPO_ROOT = resolve(import.meta.dirname, '..')
const ENRICH = resolve(REPO_ROOT, 'data', 'enrichment')
const H3R4_DIR = resolve(REPO_ROOT, 'data', 'prepared', DATA_YEAR, 'h3r4')
const STAGING_DIR = resolve(ENRICH, 'global', 'overture-obstacles', 'h3r4')
const WORKER = resolve(REPO_ROOT, 'scripts', 'obstacles', 'enrich-obstacle-heights.py')
const DOWNLOADER = resolve(REPO_ROOT, 'scripts', 'obstacles', 'download-height-rasters.sh')

const GHSL_TIF = resolve(
  ENRICH,
  'global',
  'ghsl-built-h',
  'GHS_BUILT_H_ANBH_E2018_GLOBE_R2023A_54009_100_V1_0.tif',
)

/** Regional per-building height rasters (tier 3). bbox = [S, W, N, E] WGS84
 *  envelope of the raster — a cell whose H3 boundary OVERLAPS it gets the
 *  raster passed to the worker (per-row zonal coverage decides inside).
 *  One entry per covered area; adding a region = cache its raster + one row. */
const REGIONAL_RASTERS: ReadonlyArray<{
  key: string
  vrt: string
  bbox: readonly [number, number, number, number]
}> = [
  {
    key: 'cz-ipr-praha-vysky',
    vrt: resolve(ENRICH, DATA_YEAR, 'cz', 'ipr-relativni-vysky', 'mosaic.vrt'),
    // Official dataset envelope from the IPR metadata — an eyeballed smaller
    // bbox drops fringe cells from the regional group (gg pass 1).
    bbox: [49.918791, 14.197594, 50.213928, 14.769633],
  },
]

function parseArgs(argv: string[]): { enrichOnly: boolean; bbox: number[] | null; cells: string[] | null } {
  let enrichOnly = false
  let bbox: number[] | null = null
  let cells: string[] | null = null
  for (let i = 2; i < argv.length; i++) {
    const a = argv[i]
    if (a === '--enrich-only') enrichOnly = true
    else if (a === '--bbox') {
      bbox = argv[++i].split(',').map(Number)
      if (bbox.length !== 4 || bbox.some(Number.isNaN)) throw new Error(`bad --bbox ${argv[i]}`)
    } else if (a === '--cells') cells = argv[++i].split(',')
    else throw new Error(`unknown argument ${a}`)
  }
  return { enrichOnly, bbox, cells }
}

/** [S, W, N, E] envelope of a cell's H3 boundary. */
function cellEnvelope(hex: string): [number, number, number, number] {
  const b = cellToBoundary(hex)
  let s = 90, w = 180, n = -90, e = -180
  for (const [lat, lon] of b) {
    s = Math.min(s, lat); n = Math.max(n, lat)
    w = Math.min(w, lon); e = Math.max(e, lon)
  }
  return [s, w, n, e]
}

function overlaps(a: readonly number[], b: readonly number[]): boolean {
  return a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

/** Promoted cells in scope: hex dirs carrying obstacles.arrow whose boundary
 *  envelope overlaps the bbox. (Antimeridian-straddling cells are the usual
 *  envelope caveat — none of the promoted store's cells wrap today.) */
function promotedCellsInScope(bbox: number[] | null): string[] {
  const out: string[] = []
  for (const hex of readdirSync(H3R4_DIR)) {
    if (!/^[0-9a-f]{15}$/.test(hex)) continue
    if (!existsSync(resolve(H3R4_DIR, hex, 'obstacles.arrow'))) continue
    if (bbox && !overlaps(cellEnvelope(hex), bbox)) continue
    out.push(hex)
  }
  return out.sort()
}

function main(): void {
  const { enrichOnly, bbox, cells } = parseArgs(process.argv)

  if (!enrichOnly) {
    // DATA_YEAR must reach the shell explicitly — its fallback literal would
    // drift the cache into the wrong year once the committed pin advances.
    const dl = spawnSync('bash', [DOWNLOADER], {
      stdio: 'inherit',
      env: { ...process.env, DATA_YEAR },
    })
    if (dl.status !== 0) throw new Error('height raster download failed')
  }
  if (!existsSync(GHSL_TIF)) {
    throw new Error(
      `GHSL ANBH raster missing (${GHSL_TIF}) — run scripts/obstacles/download-height-rasters.sh once`,
    )
  }

  const targets = cells ?? promotedCellsInScope(bbox)
  if (targets.length === 0) {
    console.log('no promoted cells in scope — nothing to enrich')
    return
  }

  // Group by the regional raster overlapping the cell boundary (first match
  // wins); one worker run per group keeps the ~4 GB regional array loaded
  // exactly once.
  const groups = new Map<string, string[]>()
  for (const hex of targets) {
    const env = cellEnvelope(hex)
    const region = REGIONAL_RASTERS.find((r) => overlaps(env, r.bbox))
    if (region && !existsSync(region.vrt)) {
      // NEVER silently degrade to ANBH-only: regenerating a regional cell
      // without its raster erases every tier-3 height it had (gg pass 1).
      // Deleting the REGIONAL_RASTERS row is the explicit opt-out.
      throw new Error(
        `cell ${hex} overlaps regional raster ${region.key} but ${region.vrt} is missing — ` +
          `run scripts/obstacles/download-height-rasters.sh (or remove the region row to accept ANBH-only)`,
      )
    }
    const key = region ? region.vrt : ''
    let group = groups.get(key)
    if (!group) groups.set(key, (group = []))
    group.push(hex)
  }

  // Cells travel via a manifest file, never argv — a world run's cell list
  // exceeds ARG_MAX as a single argument (gg pass 2: ~8k cells is the ceiling).
  const manifestDir = mkdtempSync(join(tmpdir(), 'obstacle-heights-'))
  try {
    let g = 0
    for (const [vrt, hexes] of groups) {
      const cellsFile = join(manifestDir, `cells-${g++}.txt`)
      writeFileSync(cellsFile, hexes.join('\n') + '\n')
      const args = [
        WORKER,
        '--h3r4-dir', H3R4_DIR,
        '--staging-dir', STAGING_DIR,
        '--ghsl', GHSL_TIF,
        '--cells-file', cellsFile,
      ]
      if (vrt) args.push('--regional', vrt)
      console.log(
        `[obstacle-heights] ${hexes.length} cell(s) ${vrt ? `with regional ${vrt}` : 'ANBH-only'}`,
      )
      const run = spawnSync('python3', args, { stdio: 'inherit' })
      if (run.status !== 0) throw new Error(`enrich worker failed (exit ${run.status})`)
    }
  } finally {
    rmSync(manifestDir, { recursive: true, force: true })
  }
}

main()
