//! Shared parallel-track divisor pass: multi-track corridors must not multiply
//! per-track defaults (task #30 part 1).
//!
//! WHY: DE mainline stations measure +8..17 dB hot vs EBA masts
//! (docs/validation/findings/2026-07-11-de-mainline-trackside-hot.md). On
//! unenriched rows every parallel track carries the FULL usage default, so an
//! n-track corridor emits n× its real traffic. `parallel_divisor` divides
//! per-track traffic. This pass computes it OSM-corridor-side, keyed on OSM
//! ref/name identity, for every source that DID NOT migrate to the graph-walk
//! driver (rail-walk-enrich.ts) — see WALK-MANAGED SOURCES below for the ones
//! that did.
//!
//! CORRIDOR IDENTITY (deterministic, geometry-light): rows group by
//! (first non-empty of [ref, name]) + rail_type + usage. `service > 0` rows
//! are excluded entirely — they neither receive nor grant a divisor. Rows with
//! NEITHER ref nor name are left untouched (no guessing). Multi-value refs
//! ("2600;2601") are exact-string tokens in v1 — a way tagged with the pair
//! does not group with "2600".
//!
//! DIVISOR: per row, min(3, count of DISTINCT osm_id in the same corridor
//! group with a segment within 50 m of the row's midpoint, self included) —
//! point-to-segment distance (not mid-to-mid, which misses parallel tracks
//! whose microsegment phase is offset by ~half a segment). A candidate
//! segment sharing an endpoint node (nodeKey) with the row is skipped:
//! consecutive ways of ONE track meet at a node, and near that junction a
//! short end-segment's midpoint lies within 50 m of the continuation way
//! ALONG the line — longitudinal continuity, not a parallel track. Residual
//! v1 imprecision: with ultra-short (<~40 m) end-segments the continuation
//! way's SECOND segment can still slip inside 50 m — a sliver-local halving,
//! accepted.
//!
//! WALK-MANAGED SOURCES (2026-07-16 — replaces the old czpttKey-identity
//! asymmetry now that CZ's bespoke chord matcher is gone, migration #26): a
//! row's `source_id` may name a dataset flagged `railDivisorFromWalk` in
//! enrichment-datasets.ts (today: cz-szcd-gtfs, cz-timetable-silent) — the
//! graph-walk driver already computed (or deliberately left at 1) that row's
//! divisor via its own lateral parallel-track spread
//! (rail-graph-metrics.ts::applyParallelSpread), and this pass must never
//! touch it: raising a walk-set 1 would be exactly as wrong as lowering a
//! walk-set 3. Such rows are excluded from grouping entirely — neither a
//! divisor RECIPIENT nor a candidate counted toward a neighbour's divisor.
//!
//! ONLY-RAISE (every other, non-walk-managed source): this pass writes where
//! current == 1 and computed > 1, never lowers. Idempotent by construction
//! (second run: current > 1 → keep). Full column provenance is the #30
//! follow-up if this ever proves too coarse.
//!
//! Usage:
//!   DATA_YEAR=2026 npx tsx pipeline/enrich-railways-parallel.ts --bbox 47.3,5.9,55.1,15.0
//!   DATA_YEAR=2026 npx tsx pipeline/enrich-railways-parallel.ts --world
//!   DATA_YEAR=2026 npx tsx pipeline/enrich-railways-parallel.ts --bbox S,W,N,E --dry-run
//!   SHARD=0/8 DATA_YEAR=2026 npx tsx pipeline/enrich-railways-parallel.ts --world

import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { tableFromIPC } from 'apache-arrow'
import { writeRailParallelDivisor } from './lib/railways-arrow.js'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import { nodeKey, pointToSegmentDist } from './lib/spatial.js'
import { DATASETS } from './lib/enrichment-datasets.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

/** Dataset ids whose rows the graph-walk driver already owns the divisor
 *  for (see WALK-MANAGED SOURCES in the module doc) — this pass must never
 *  group or write over them. */
const RAIL_DIVISOR_FROM_WALK_SOURCE_IDS = new Set(DATASETS.filter((d) => d.railDivisorFromWalk).map((d) => d.id))


/** Same 50 m as enrich-railway-cz.ts's inline detection. */
const PARALLEL_RADIUS_M = 50
/** Same clamp as CZ: beyond 3 tracks the marginal per-track correction is <1.3 dB. */
const DIVISOR_CAP = 3
/** Candidate-bucketing grid pitch — must exceed the query radius so a ±1-cell
 *  scan covers the disc; true distances always via pointToSegmentDist. */
const GRID_CELL_M = 128
/** Query rectangle half-width: radius + slack for the grid's group-wide cosLat
 *  vs pointToSegmentDist's row-local cosLat (<1 m at hex scale, padded 10). */
const QUERY_PAD_M = PARALLEL_RADIUS_M + 10
// Bucketing-only projection constants (values match lib/spatial.ts).
const M_PER_DEG_LAT = 110_540
const M_PER_DEG_LON_EQ = 111_320

/** One railways.arrow row as the divisor computation sees it. */
export interface ParallelRailRow {
  /** Stringified osm_id — distinct-way identity (bigint-safe). */
  osmId: string
  /** First non-empty of [ref, name], trimmed; '' = no corridor identity. */
  corridorToken: string
  railType: number
  usage: number
  service: number
  /** Dataset id this row is currently stamped under (0 = unspecified) — used
   *  ONLY to exclude rows a `railDivisorFromWalk` source owns (see the
   *  WALK-MANAGED SOURCES module doc); not otherwise part of the corridor
   *  grouping identity. */
  sourceId: number
  startLat: number
  startLon: number
  endLat: number
  endLon: number
}

/**
 * Per-row parallel-track divisor for one hex's rows: `null` = row not eligible
 * (service > 0, no corridor token, or owned by a walk-managed source — leave
 * whatever is stored), else min(3, distinct osm_ids of the row's corridor
 * within 50 m, self included).
 */
export function computeParallelDivisors(rows: ParallelRailRow[]): Array<number | null> {
  const n = rows.length
  const out: Array<number | null> = new Array(n).fill(null)

  const groups = new Map<string, number[]>()
  for (let i = 0; i < n; i++) {
    const r = rows[i]
    if (r.service > 0) continue // sidings/yards/spurs: excluded entirely
    if (!r.corridorToken) continue // no ref, no name → no corridor identity, no guessing
    if (RAIL_DIVISOR_FROM_WALK_SOURCE_IDS.has(r.sourceId)) continue // the walk owns this row's divisor
    const key = `${r.corridorToken}\x1f${r.railType}\x1f${r.usage}`
    let g = groups.get(key)
    if (!g) groups.set(key, (g = []))
    g.push(i)
  }

  for (const idxs of groups.values()) {
    // Fast path: a single distinct way can never be its own parallel track.
    const firstOsm = rows[idxs[0]].osmId
    if (idxs.every((i) => rows[i].osmId === firstOsm)) {
      for (const i of idxs) out[i] = 1
      continue
    }

    // Per-corridor grid so the neighbour scan is O(n·k), not O(n²) — big
    // station hexes hold thousands of same-ref microsegments.
    const kx = M_PER_DEG_LON_EQ * Math.cos((rows[idxs[0]].startLat * Math.PI) / 180)
    const ky = M_PER_DEG_LAT
    const grid = new Map<string, number[]>()
    for (const i of idxs) {
      const r = rows[i]
      const x0 = Math.min(r.startLon, r.endLon) * kx
      const x1 = Math.max(r.startLon, r.endLon) * kx
      const y0 = Math.min(r.startLat, r.endLat) * ky
      const y1 = Math.max(r.startLat, r.endLat) * ky
      for (let cx = Math.floor(x0 / GRID_CELL_M); cx <= Math.floor(x1 / GRID_CELL_M); cx++) {
        for (let cy = Math.floor(y0 / GRID_CELL_M); cy <= Math.floor(y1 / GRID_CELL_M); cy++) {
          const k = `${cx}_${cy}`
          let cell = grid.get(k)
          if (!cell) grid.set(k, (cell = []))
          cell.push(i)
        }
      }
    }

    // Quantized endpoint identity (nodeKey) for the junction exclusion.
    const startKeys = new Map<number, string>()
    const endKeys = new Map<number, string>()
    for (const i of idxs) {
      startKeys.set(i, nodeKey(rows[i].startLat, rows[i].startLon))
      endKeys.set(i, nodeKey(rows[i].endLat, rows[i].endLon))
    }

    for (const i of idxs) {
      const r = rows[i]
      const midLat = (r.startLat + r.endLat) / 2
      const midLon = (r.startLon + r.endLon) / 2
      const mx = midLon * kx
      const my = midLat * ky
      const iStart = startKeys.get(i)!
      const iEnd = endKeys.get(i)!
      const found = new Set<string>([r.osmId])
      const seen = new Set<number>()
      outer: for (
        let cx = Math.floor((mx - QUERY_PAD_M) / GRID_CELL_M);
        cx <= Math.floor((mx + QUERY_PAD_M) / GRID_CELL_M);
        cx++
      ) {
        for (
          let cy = Math.floor((my - QUERY_PAD_M) / GRID_CELL_M);
          cy <= Math.floor((my + QUERY_PAD_M) / GRID_CELL_M);
          cy++
        ) {
          const cell = grid.get(`${cx}_${cy}`)
          if (!cell) continue
          for (const j of cell) {
            if (found.size >= DIVISOR_CAP) break outer // clamp reached — counting more changes nothing
            if (j === i || seen.has(j)) continue // a segment sits in several cells
            seen.add(j)
            const q = rows[j]
            if (found.has(q.osmId)) continue
            // Junction, not parallelism: consecutive ways of the SAME track
            // share an endpoint node, and near it a short end-segment's
            // midpoint is within 50 m of the continuation ALONG the line.
            const jStart = startKeys.get(j)!
            const jEnd = endKeys.get(j)!
            if (jStart === iStart || jStart === iEnd || jEnd === iStart || jEnd === iEnd) continue
            const d = pointToSegmentDist(midLat, midLon, q.startLat, q.startLon, q.endLat, q.endLon)
            if (d < PARALLEL_RADIUS_M) found.add(q.osmId)
          }
        }
      }
      out[i] = Math.min(DIVISOR_CAP, found.size)
    }
  }

  return out
}

/** Per-hex outcome, aggregated by main() into the run summary. */
export interface HexDivisorStats {
  rows: number
  /** Non-service rows carrying a corridor token (grouped + computed). */
  eligibleRows: number
  serviceRows: number
  /** Non-service rows with neither ref nor name — always untouched. */
  noTokenRows: number
  /** computedHist[d-1] = eligible rows whose computed divisor is d (1..3). */
  computedHist: [number, number, number]
  /** writtenHist[d-1] = rows actually written d (only-raise: index 0 stays 0). */
  writtenHist: [number, number, number]
  /** Eligible rows whose stored divisor was already >1 (CZ-style) — never touched. */
  keptExistingAbove1: number
  /** True when the file was rewritten (always false under --dry-run). */
  changed: boolean
}

/**
 * One hex: read railways.arrow → group by corridor → compute divisors → write
 * via `writeRailParallelDivisor` under the only-raise rule (see module header).
 * Read happens outside the writer's flock, which is safe here: shards split by
 * hex, so no two workers ever touch the same file.
 */
export async function enrichHexRailParallelDivisor(
  arrowPath: string,
  opts: { dryRun?: boolean } = {},
): Promise<HexDivisorStats> {
  const stats: HexDivisorStats = {
    rows: 0,
    eligibleRows: 0,
    serviceRows: 0,
    noTokenRows: 0,
    computedHist: [0, 0, 0],
    writtenHist: [0, 0, 0],
    keptExistingAbove1: 0,
    changed: false,
  }

  const table = tableFromIPC(readFileSync(arrowPath))
  const n = table.numRows
  stats.rows = n
  if (n === 0) return stats

  const osmIdCol = table.getChild('osm_id')
  const sLat = table.getChild('start_lat')
  const sLon = table.getChild('start_lon')
  const eLat = table.getChild('end_lat')
  const eLon = table.getChild('end_lon')
  if (!osmIdCol || !sLat || !sLon) return stats // malformed hex — never touch

  const refCol = table.getChild('ref')
  const nameCol = table.getChild('name')
  const rtCol = table.getChild('rail_type')
  const usCol = table.getChild('usage')
  const svcCol = table.getChild('service')
  const srcCol = table.getChild('source_id')

  const rows: ParallelRailRow[] = new Array(n)
  for (let i = 0; i < n; i++) {
    const startLat = sLat.get(i) as number
    const startLon = sLon.get(i) as number
    const ref = ((refCol?.get(i) as string | null) ?? '').trim()
    const name = ((nameCol?.get(i) as string | null) ?? '').trim()
    const service = (svcCol?.get(i) as number) ?? 0
    rows[i] = {
      osmId: String(osmIdCol.get(i)),
      corridorToken: ref || name,
      railType: (rtCol?.get(i) as number) ?? 0,
      usage: (usCol?.get(i) as number) ?? 0,
      service,
      sourceId: (srcCol?.get(i) as number) ?? 0,
      startLat,
      startLon,
      endLat: (eLat?.get(i) as number) ?? startLat,
      endLon: (eLon?.get(i) as number) ?? startLon,
    }
    if (service > 0) stats.serviceRows++
    else if (!rows[i].corridorToken) stats.noTokenRows++
  }

  const computed = computeParallelDivisors(rows)
  const exDiv = table.getChild('parallel_divisor')
  const decision: Array<number | null> = new Array(n).fill(null)
  let anyWrite = false
  for (let i = 0; i < n; i++) {
    const c = computed[i]
    if (c === null) continue
    stats.eligibleRows++
    stats.computedHist[c - 1]++
    // `|| 1` mirrors the CZ read (and the engine's missing-column default): a
    // stored 0 means 1. current > 1 is the CZ-protected zone — never touched.
    const current = exDiv ? ((exDiv.get(i) as number) || 1) : 1
    if (current > 1) {
      stats.keptExistingAbove1++
      continue
    }
    if (c > 1) {
      decision[i] = c
      stats.writtenHist[c - 1]++
      anyWrite = true
    }
  }

  if (anyWrite && !opts.dryRun) {
    const res = await writeRailParallelDivisor(arrowPath, (i) => decision[i])
    stats.changed = res.updated
  }
  return stats
}

async function main() {
  const world = process.argv.includes('--world')
  const dryRun = process.argv.includes('--dry-run')
  const bboxArg = process.argv.includes('--bbox') ? process.argv[process.argv.indexOf('--bbox') + 1] : ''
  const BBOX = bboxArg ? (bboxArg.split(',').map(Number) as [number, number, number, number]) : null
  if (!world && (!BBOX || BBOX.length !== 4 || BBOX.some((x) => !Number.isFinite(x)))) {
    console.error('Usage: enrich-railways-parallel.ts --bbox S,W,N,E | --world [--dry-run]')
    process.exit(1)
  }
  if (!existsSync(H3R4_DIR)) {
    console.error(`ERROR: H3R4 directory not found: ${H3R4_DIR}`)
    process.exit(1)
  }

  // Same enumeration shape as enrich-roads-built-up: --bbox → centroid filter,
  // else full tree; sorted so SHARD slices reproduce across runs.
  let hexDirs = (
    world
      ? readdirSync(H3R4_DIR).filter((d) => d.length === 15 && d.endsWith('ffffffff'))
      : iterateCountryHexes(H3R4_DIR, BBOX!, 'railways.arrow')
  ).sort()

  let shardSuffix = ''
  if (process.env.SHARD) {
    const m = /^(\d+)\/(\d+)$/.exec(process.env.SHARD)
    const i = m ? Number(m[1]) : NaN
    const nShards = m ? Number(m[2]) : NaN
    if (!m || nShards <= 0 || i >= nShards) {
      console.error(`ERROR: invalid SHARD="${process.env.SHARD}" (expected i/n with 0 <= i < n)`)
      process.exit(1)
    }
    hexDirs = hexDirs.slice(Math.floor((i * hexDirs.length) / nShards), Math.floor(((i + 1) * hexDirs.length) / nShards))
    shardSuffix = ` | shard ${process.env.SHARD}`
  }

  console.log(
    `=== Parallel-track divisor pass (${YEAR}) — scope ${world ? 'WORLD' : BBOX!.join(',')}` +
      `${shardSuffix}${dryRun ? ' | DRY-RUN' : ''} — ${hexDirs.length} hexes ===`,
  )

  const t0 = Date.now()
  let hexesScanned = 0
  let hexesWithEligible = 0
  let hexesChanged = 0
  const totals: HexDivisorStats = {
    rows: 0,
    eligibleRows: 0,
    serviceRows: 0,
    noTokenRows: 0,
    computedHist: [0, 0, 0],
    writtenHist: [0, 0, 0],
    keptExistingAbove1: 0,
    changed: false,
  }
  for (const hexId of hexDirs) {
    const arrowPath = resolve(H3R4_DIR, hexId, 'railways.arrow')
    if (!existsSync(arrowPath)) continue
    const s = await enrichHexRailParallelDivisor(arrowPath, { dryRun })
    hexesScanned++
    if (s.eligibleRows > 0) hexesWithEligible++
    if (s.changed) hexesChanged++
    totals.rows += s.rows
    totals.eligibleRows += s.eligibleRows
    totals.serviceRows += s.serviceRows
    totals.noTokenRows += s.noTokenRows
    totals.keptExistingAbove1 += s.keptExistingAbove1
    for (let d = 0; d < 3; d++) {
      totals.computedHist[d] += s.computedHist[d]
      totals.writtenHist[d] += s.writtenHist[d]
    }
    if (hexesScanned % 500 === 0) {
      const dt = ((Date.now() - t0) / 1000).toFixed(0)
      console.log(
        `  progress: ${hexesScanned}/${hexDirs.length} hexes in ${dt}s — ` +
          `written 2×${totals.writtenHist[1]} 3×${totals.writtenHist[2]}`,
      )
    }
  }

  const h = totals.computedHist
  const w = totals.writtenHist
  console.log(`\n=== Results ===`)
  console.log(`  hexes: ${hexesScanned} scanned, ${hexesWithEligible} with corridor rows, ${hexesChanged} rewritten`)
  console.log(
    `  rows: ${totals.rows} total | ${totals.eligibleRows} corridor-eligible | ` +
      `${totals.serviceRows} service-excluded | ${totals.noTokenRows} without ref/name (untouched)`,
  )
  console.log(`  computed divisors (eligible rows): 1×${h[0]}  2×${h[1]}  3×${h[2]}`)
  console.log(`  written (only-raise where current==1): 2×${w[1]}  3×${w[2]}`)
  console.log(`  kept: ${totals.keptExistingAbove1} rows already carried divisor >1 (CZ czpttKey values — never lowered)`)
  if (dryRun) console.log(`  DRY-RUN: no files written`)
}

// Import-safe: run only when invoked directly — importing this module (tests)
// must never start a world pass (pattern from enrich-railway-cz.ts).
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((err) => {
    console.error('Error:', err)
    process.exit(1)
  })
}
