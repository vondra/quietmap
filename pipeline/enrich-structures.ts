/**
 * The ONE per-cell structure table — the manifest face of
 * `scripts/structures/build-structures.py` (the builder; its module docstring
 * is the schema/semantics authority for structures.arrow).
 *
 * Terminal chain phase: every pre-merge buildings.arrow reader/writer
 * (buildings-cz/es, service-tree, the gate auditor) sits in an earlier phase;
 * this step merges buildings.arrow + barriers.arrow + the Overture one-degree
 * parquet stock into each prepared cell's structures.arrow and passes
 * --retire-inputs, so a finished cell holds only structures.arrow. The
 * fresh-extract tail (osm-to-h3r4.sh) runs the same face WITHOUT
 * --retire-inputs — the enrichers still need the pre-merge files there.
 *
 * Scope: `--bbox S,W,N,E` selects PREPARED cells whose H3 BOUNDARY overlaps the
 * bbox — centre-only tests silently drop the cells a regional raster clips;
 * `--cells hex,...` is verification isolation (never in a chain run).
 *
 * `--enrich-only` skips the cache download
 * (`scripts/obstacles/download-height-rasters.sh`: GHSL ANBH GeoTIFF + the IPR
 * Praha exportImage tiles stamped EPSG:5514 + mosaic.vrt + a Žižkov-tower
 * control-point validation). The GHSL raster and the Overture parquet cache are
 * required; a regional raster is optional per REGIONAL_RASTERS — a cell
 * overlapping a region whose mosaic is missing is an ERROR, never a silent
 * ANBH-only rebuild (that would erase every tier-3 height it had).
 *
 * The builder is deterministic and idempotent per cell (rebuild iff an input
 * is newer than the output), so a chain re-run rebuilds exactly the cells the
 * earlier phases touched. Two drivers: the osm-to-h3r4.sh tail runs this face
 * on a fresh extract (no --enrich-only — the height-raster download is part of
 * the fresh-extract cache fill; built-up samples the tables right after), and
 * the chain runs it as the terminal structures step with --enrich-only.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-structures.ts [--enrich-only] [--bbox S,W,N,E] [--retire-inputs]
 */

import { spawnSync } from 'node:child_process'
import { existsSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { cellToBoundary } from 'h3-js'
import { DATA_YEAR, H3R4_DIR } from './lib/data-year.js'

const REPO_ROOT = resolve(import.meta.dirname, '..')
const ENRICH = resolve(REPO_ROOT, 'data', 'enrichment')
const BUILDER = resolve(REPO_ROOT, 'scripts', 'structures', 'build-structures.py')
const DOWNLOADER = resolve(REPO_ROOT, 'scripts', 'obstacles', 'download-height-rasters.sh')

const GHSL_TIF = resolve(
  ENRICH,
  'global',
  'ghsl-built-h',
  'GHS_BUILT_H_ANBH_E2018_GLOBE_R2023A_54009_100_V1_0.tif',
)

/** The Overture one-degree parquet cache (scripts/obstacles/download-overture-world.sh) —
 *  the builder's screening-stock source. */
const OVERTURE_PARQUET_DIR = resolve(REPO_ROOT, 'data', 'source', 'enrichment', 'global', 'overture-buildings', 'parquet')

/** Regional per-building height rasters (tier 3). bbox = [S, W, N, E] WGS84
 *  envelope of the raster — a cell whose H3 boundary OVERLAPS it gets the
 *  raster passed to the builder (per-row zonal coverage decides inside).
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
    // bbox drops fringe cells from the regional group.
    bbox: [49.918791, 14.197594, 50.213928, 14.769633],
  },
]

function parseArgs(argv: string[]): { enrichOnly: boolean; bbox: number[] | null; cells: string[] | null; retireInputs: boolean } {
  let enrichOnly = false
  let bbox: number[] | null = null
  let cells: string[] | null = null
  let retireInputs = false
  for (let i = 2; i < argv.length; i++) {
    const a = argv[i]
    if (a === '--enrich-only') enrichOnly = true
    else if (a === '--retire-inputs') retireInputs = true
    else if (a === '--bbox') {
      bbox = argv[++i].split(',').map(Number)
      if (bbox.length !== 4 || bbox.some(Number.isNaN)) throw new Error(`bad --bbox ${argv[i]}`)
    } else if (a === '--cells') cells = argv[++i].split(',')
    else throw new Error(`unknown argument ${a}`)
  }
  return { enrichOnly, bbox, cells, retireInputs }
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

function regionalRasterForCell(hex: string): (typeof REGIONAL_RASTERS)[number] | undefined {
  return REGIONAL_RASTERS.find((r) => overlaps(cellEnvelope(hex), r.bbox))
}

/** Prepared cells in scope: every hex dir whose boundary envelope overlaps the
 *  bbox. (Antimeridian-straddling cells are the usual envelope caveat.) */
function preparedCellsInScope(bbox: number[] | null): string[] {
  const out: string[] = []
  for (const hex of readdirSync(H3R4_DIR)) {
    if (!/^[0-9a-f]{15}$/.test(hex)) continue
    if (bbox && !overlaps(cellEnvelope(hex), bbox)) continue
    out.push(hex)
  }
  return out.sort()
}

function main(): void {
  const { enrichOnly, bbox, cells, retireInputs } = parseArgs(process.argv)

  const requested = [...new Set(cells ?? preparedCellsInScope(bbox))].sort()
  if (requested.length === 0) {
    console.log('no prepared cells in scope — nothing to build')
    return
  }

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
  if (!existsSync(OVERTURE_PARQUET_DIR) || readdirSync(OVERTURE_PARQUET_DIR).filter((f) => f.endsWith('.parquet')).length === 0) {
    throw new Error(
      `Overture parquet cache missing or empty (${OVERTURE_PARQUET_DIR}) — run scripts/obstacles/download-overture-world.sh first`,
    )
  }

  // Group by the regional raster overlapping the cell boundary (first match
  // wins); one builder run per group keeps the ~4 GB regional array loaded
  // exactly once.
  const groups = new Map<string, { cells: string[]; vrt: string }>()
  for (const hex of requested) {
    if (!/^[0-9a-f]{15}$/.test(hex)) throw new Error(`invalid H3R4 cell '${hex}'`)
    const region = regionalRasterForCell(hex)
    if (region && !existsSync(region.vrt)) {
      // NEVER silently degrade to ANBH-only: rebuilding a regional cell
      // without its raster erases every tier-3 height it had.
      // Deleting the REGIONAL_RASTERS row is the explicit opt-out.
      throw new Error(
        `cell ${hex} overlaps regional raster ${region.key} but ${region.vrt} is missing — ` +
          `run scripts/obstacles/download-height-rasters.sh (or remove the region row to accept ANBH-only)`,
      )
    }
    const key = region?.vrt ?? ''
    let group = groups.get(key)
    if (!group) groups.set(key, (group = { cells: [], vrt: key }))
    group.cells.push(hex)
  }

  // Cells travel via a manifest file, never argv — a world run's cell list
  // exceeds ARG_MAX as a single argument.
  const tmp = mkdtempSync(join(tmpdir(), 'structures-'))
  try {
    let g = 0
    for (const group of groups.values()) {
      const cellsFile = join(tmp, `cells-${g++}.txt`)
      writeFileSync(cellsFile, group.cells.join('\n') + '\n')
      const args = [
        BUILDER,
        '--h3r4-dir', H3R4_DIR,
        '--overture-parquet', OVERTURE_PARQUET_DIR,
        '--ghsl', GHSL_TIF,
        '--cells-file', cellsFile,
      ]
      // Chain mode retires the pre-merge inputs; the fresh-extract tail must not
      // (the buildings enrichers run after it).
      if (retireInputs) args.push('--retire-inputs')
      if (group.vrt) args.push('--regional', group.vrt)
      console.log(`[structures] ${group.cells.length} cell(s) ${group.vrt ? `with regional ${group.vrt}` : 'ANBH-only'}`)
      const run = spawnSync('python3', args, { stdio: 'inherit' })
      if (run.status !== 0) throw new Error(`build-structures.py failed (exit ${run.status})`)
    }
  } finally {
    rmSync(tmp, { recursive: true, force: true })
  }
}

main()
