//! Road `built_up` flag from the Overture building footprints (task #15).
//!
//! Writes a u8 `built_up` column into every roads.arrow: 1 = rural, 2 = urban,
//! 0 = unknown ONLY when a 1° tile the sampling window touches was never
//! ingested into the obstacle store. The engine
//! (`noise-compute::defaults::resolve_speed_default`) uses it to pick the
//! country's legal urban/rural implicit speed for UNTAGGED roads of classes
//! 2/3/4/9; 0 falls back to the legacy world speed table. The column is written
//! for every row regardless of class — the engine decides what consumes it.
//!
//! Decision rule (see lib/building-footprints.ts): a segment is urban iff the
//! building footprints centred within ±8.5/3600° of its MIDPOINT carry at least
//! BUILT_UP_MIN_BUILT_PIXELS pixels' worth of area on the retired 30 m raster
//! grid.
//!
//! WHY it no longer reads the building raster: that raster was only ever an
//! urban-density proxy for this one flag and has been deleted. The
//! vector obstacle store the engine already screens against carries the same
//! Overture footprints, so the probe reads those instead. Cutover validation
//! measured 27 951 road segments across CZ/DE/FR/GB/US/BR:
//! the two probes give the same answer for 97.30 % of segments, no segment
//! changes to or from UNKNOWN because the same manifest defines coverage, and
//! of the 5 483 segments that actually
//! consult the flag — class 2/3/4/9 with no OSM maxspeed and no taper — 2.48 %
//! resolve a different default speed.
//!
//! Idempotent + safe: unchanged hexes are left byte-identical; changed hexes go
//! through `withArrowWrite` (flock + tmp + rename, never truncate in place).
//! Runs after OSM extraction, independent of AADT enrichment (only writes
//! built_up). Per-hex, self-contained → SHARD=i/n parallelizes it like
//! service-tree (wired in scripts/osm-to-h3r4.sh).
//!
//! Usage:
//!   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-built-up.ts
//!   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-built-up.ts --prefix 841e309
//!   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-built-up.ts --bbox 49.7,13.9,50.4,15.0
//!   SHARD=0/96 DATA_YEAR=2026 node_modules/.bin/tsx pipeline/enrich-roads-built-up.ts

import { existsSync, readdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { makeVector, makeTable, type Table } from 'apache-arrow'
import { withArrowWrite } from './lib/provenance.js'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import {
  BuildingFootprintSampler,
  OBSTACLE_STORE_DIR,
  OBSTACLE_INGEST_MANIFEST,
  BUILT_UP_UNKNOWN,
} from './lib/building-footprints.js'
import { H3R4_DIR } from './lib/data-year.js'

const PREFIX = process.argv.includes('--prefix') ? process.argv[process.argv.indexOf('--prefix') + 1] : ''
const bboxArg = process.argv.includes('--bbox') ? process.argv[process.argv.indexOf('--bbox') + 1] : ''
const BBOX = bboxArg ? (bboxArg.split(',').map(Number) as [number, number, number, number]) : null
if (BBOX && (BBOX.length !== 4 || BBOX.some((x) => !Number.isFinite(x)))) {
  console.error(`ERROR: --bbox must be minLat,minLon,maxLat,maxLon (got "${bboxArg}")`)
  process.exit(1)
}

const sampler = new BuildingFootprintSampler()

interface HexResult {
  rows: number
  unknown: number // built_up=0 (a 1° tile the window touches was never ingested)
  rural: number
  urban: number
  changed: boolean
}

/** One hex: classify every segment midpoint, rewrite roads.arrow with the
 *  built_up column added (or replaced — re-runs are byte-identical no-ops). */
async function processHex(arrowPath: string): Promise<HexResult> {
  const res: HexResult = { rows: 0, unknown: 0, rural: 0, urban: 0, changed: false }
  await withArrowWrite(arrowPath, (table: Table): Table => {
    const n = table.numRows
    res.rows = n
    const sLat = table.getChild('start_lat')
    const sLon = table.getChild('start_lon')
    const eLat = table.getChild('end_lat')
    const eLon = table.getChild('end_lon')
    if (n === 0 || !sLat || !sLon) return table // empty/malformed hex — never touch

    const existing = table.getChild('built_up')
    const builtUp = new Uint8Array(n)
    let sameAsExisting = existing !== null
    for (let i = 0; i < n; i++) {
      const startLat = sLat.get(i) as number
      const startLon = sLon.get(i) as number
      const midLat = (startLat + ((eLat?.get(i) as number) ?? startLat)) / 2
      const midLon = (startLon + ((eLon?.get(i) as number) ?? startLon)) / 2
      const v = sampler.classifyBuiltUp(midLat, midLon)
      builtUp[i] = v
      if (v === BUILT_UP_UNKNOWN) res.unknown++
      else if (v === 1) res.rural++
      else res.urban++
      if (sameAsExisting && (existing!.get(i) as number) !== v) sameAsExisting = false
    }
    if (sameAsExisting) return table // idempotent re-run → leave bytes untouched

    // Rebuild preserving every other column verbatim (same idiom as
    // writeRoadAadt in lib/roads-arrow.ts, but adding a column — so the
    // schema-copy loop appends built_up when absent, replaces in place when not).
    // eslint-disable-next-line @typescript-eslint/no-explicit-any -- makeTable's
    // typing wants TypedArrays; mixing existing Vectors with makeVector is fine
    // at runtime (same bridge as writeRoadAadt).
    const cols: Record<string, any> = {}
    for (const f of table.schema.fields) {
      if (f.name === 'built_up') continue
      cols[f.name] = table.getChild(f.name)!
    }
    cols['built_up'] = makeVector(builtUp)
    res.changed = true
    return makeTable(cols)
  })
  return res
}

async function main() {
  if (!existsSync(H3R4_DIR)) {
    console.error(`ERROR: H3R4 directory not found: ${H3R4_DIR}`)
    process.exit(1)
  }
  // Fail loud if the obstacle store or its ingest manifest is absent —
  // otherwise every segment world-wide would silently get built_up=0 and the
  // engine would never see an urban/rural signal. The manifest is the one that
  // decides UNKNOWN, so its absence is just as fatal as an empty store.
  if (!existsSync(OBSTACLE_STORE_DIR) || readdirSync(OBSTACLE_STORE_DIR).length === 0) {
    console.error(`ERROR: obstacle store missing or empty: ${OBSTACLE_STORE_DIR}`)
    process.exit(1)
  }
  if (!existsSync(OBSTACLE_INGEST_MANIFEST)) {
    console.error(`ERROR: obstacle ingest manifest missing: ${OBSTACLE_INGEST_MANIFEST}`)
    process.exit(1)
  }

  // Same enumeration shape as continuity-fill/service-tree: --bbox → region,
  // else full tree (optionally --prefix), sorted so SHARD slices reproduce.
  let hexDirs = (
    BBOX
      ? iterateCountryHexes(H3R4_DIR, BBOX)
      : readdirSync(H3R4_DIR).filter((d) => !d.startsWith('.') && (!PREFIX || d.startsWith(PREFIX)))
  ).sort()

  if (process.env.SHARD) {
    const m = /^(\d+)\/(\d+)$/.exec(process.env.SHARD)
    const i = m ? Number(m[1]) : NaN
    const nShards = m ? Number(m[2]) : NaN
    if (!m || nShards <= 0 || i >= nShards) {
      console.error(`ERROR: invalid SHARD="${process.env.SHARD}" (expected i/n with 0 <= i < n)`)
      process.exit(1)
    }
    hexDirs = hexDirs.slice(Math.floor((i * hexDirs.length) / nShards), Math.floor(((i + 1) * hexDirs.length) / nShards))
  }

  const t0 = Date.now()
  let hexes = 0
  let changedHexes = 0
  let missingTileHexes = 0
  const totals = { rows: 0, unknown: 0, rural: 0, urban: 0 }
  for (const hexId of hexDirs) {
    const arrowPath = resolve(H3R4_DIR, hexId, 'roads.arrow')
    if (!existsSync(arrowPath)) continue
    const r = await processHex(arrowPath)
    hexes++
    if (r.changed) changedHexes++
    if (r.unknown > 0) missingTileHexes++
    totals.rows += r.rows
    totals.unknown += r.unknown
    totals.rural += r.rural
    totals.urban += r.urban
    if (hexes % 1000 === 0) {
      const dt = ((Date.now() - t0) / 1000).toFixed(0)
      console.log(`  progress: ${hexes}/${hexDirs.length} hexes in ${dt}s — ${totals.rows} rows (${totals.urban} urban / ${totals.rural} rural / ${totals.unknown} unknown)`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  ${hexes} hexes scanned, ${changedHexes} rewritten`)
  console.log(`  ${totals.rows} segments: ${totals.urban} urban (2), ${totals.rural} rural (1), ${totals.unknown} unknown (0, 1° tile never ingested)`)
  if (missingTileHexes > 0) console.log(`  WARNING: ${missingTileHexes} hex(es) had segments over never-ingested tiles`)
}

main().catch((err) => {
  console.error('Error:', err)
  process.exit(1)
})
