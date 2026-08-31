//! R7 transition taper — CLI + Arrow I/O around `lib/roads-taper-plan.ts`.
//!
//! Provenance-safe by construction:
//!   • writes ONLY onto rows with source_id=0 AND untagged speed_limit — a
//!     census AADT or an OSM-tagged legal speed is never altered, only the
//!     defaulted side of a step ramps toward the measured/tagged value;
//!   • graded speeds go to the dedicated `speed_taper` column (never the OSM
//!     `speed_limit` tag) — the popup labels them "graded_transition", and
//!     the shared writer clears the column on any later overwrite/retract,
//!     so a stale ramp can never masquerade as a posted limit;
//!   • stamps SOURCE_ID_OSM_TRANSITION_TAPER (declared BASELINE, rank 1) so
//!     every real enricher overwrites taper rows on its next run;
//!   • fully reversible + idempotent: prior taper stamps are retracted
//!     in-memory before planning (their pre-taper state is derivable — every
//!     stamped row was src=0 with no OSM speed change), then ONE writeRoadAadt
//!     call re-stamps the fresh plan and retracts stale rows.
//!
//! v1 scope: hexes whose centroid lies in the CZ polygon — the same
//! receiver-country approximation the engine's admin cascade makes
//! (defaults.rs). Widening past CZ needs the engine's country-scaled AADT
//! defaults exported via NAPI (audit doc §6 R7); legal speeds already come
//! from the shared generated table.
//!
//! Usage:
//!   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-taper.ts --bbox 49.7,13.8,50.0,14.4
//!   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-taper.ts --bbox ... --write

import { readFileSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { tableFromIPC } from 'apache-arrow'
import { cellToLatLng } from 'h3-js'
import { SOURCE_ID_OSM_TRANSITION_TAPER } from './lib/source-ids.generated.js'
import { writeRoadAadt, iterateCountryHexes, type RoadAadt } from './lib/roads-arrow.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { nodeKey } from './lib/spatial.js'
import {
  buildTaperPlan, TAPER_CLASSES, TRIGGER_DB,
  type CountrySpeeds, type Seg, type TaperStats,
} from './lib/roads-taper-plan.js'
import { DATA_YEAR as YEAR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_OSM_TRANSITION_TAPER
const HEX_CONCURRENCY = 4

const COUNTRY_SPEED_TABLE = JSON.parse(
  readFileSync(resolve(import.meta.dirname, 'lib/country-speed-defaults.generated.json'), 'utf8'),
) as Record<string, CountrySpeeds>

/** Read one hex's roads.arrow into planner segments — shared with the
 *  discontinuity scanner (audit-map-discontinuities.ts). */
export function readSegs(arrowPath: string): Seg[] {
  const table = tableFromIPC(readFileSync(arrowPath))
  const col = (name: string) => table.getChild(name)
  const [osmC, idxC, sLatC, sLonC, eLatC, eLonC, lenC, clsC, spdC, srcC, juncC, accC, buC] = [
    'osm_id', 'segment_idx', 'start_lat', 'start_lon', 'end_lat', 'end_lon', 'length_m',
    'road_class', 'speed_limit', 'source_id', 'junction', 'access', 'built_up',
  ].map(col)
  const [lC, mC, hC, moC] = ['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto'].map(col)
  const segs: Seg[] = []
  if (!osmC || !sLatC || !sLonC) return segs
  for (let i = 0; i < table.numRows; i++) {
    const sLat = sLatC.get(i) as number
    const sLon = sLonC.get(i) as number
    segs.push({
      i,
      osmId: Number(osmC.get(i)),
      segIdx: (idxC?.get(i) as number) ?? 0,
      cls: (clsC?.get(i) as number) ?? 5,
      src: (srcC?.get(i) as number) ?? 0,
      speedTag: (spdC?.get(i) as number) ?? 0,
      builtUp: (buC?.get(i) as number) ?? 0,
      access: (accC?.get(i) as number) ?? 0,
      roundabout: ((juncC?.get(i) as number) ?? 0) !== 0,
      len: (lenC?.get(i) as number) ?? 0,
      a: nodeKey(sLat, sLon),
      b: nodeKey((eLatC?.get(i) as number) ?? sLat, (eLonC?.get(i) as number) ?? sLon),
      aadt: [
        (lC?.get(i) as number) ?? 0,
        (mC?.get(i) as number) ?? 0,
        (hC?.get(i) as number) ?? 0,
        (moC?.get(i) as number) ?? 0,
      ],
    })
  }
  return segs
}

async function processHex(
  arrowPath: string,
  country: CountrySpeeds,
  write: boolean,
): Promise<TaperStats & { stamped: number; prior: number }> {
  const segs = readSegs(arrowPath)
  // Simulate retraction of any prior taper stamps so the plan is computed on
  // clean facts. Exact by construction: taper AADT lives under src=MY_SOURCE_ID
  // (zeroing restores the default state) and taper speeds live in the separate
  // speed_taper column, which readSegs deliberately never loads — the OSM
  // speed_limit tag in `speedTag` is already the pre-taper truth.
  let prior = 0
  for (const s of segs) {
    if (s.src === MY_SOURCE_ID) {
      s.src = 0
      s.aadt = [0, 0, 0, 0]
      prior++
    }
  }
  const { plan, stats } = buildTaperPlan(segs, country)
  let stamped = 0
  if (write && (plan.size > 0 || prior > 0)) {
    // ONE read+write: fresh plan rows re-stamp (same-source overwrite is
    // idempotent; speed_taper is restated or cleared on every touched row),
    // previously-stamped rows no longer in the plan retract.
    const res = await writeRoadAadt(
      arrowPath,
      (_row, i): RoadAadt | null => {
        const e = plan.get(i)
        if (!e) return null
        const [light, medium, heavy, moto] = e.aadt ?? [0, 0, 0, 0]
        return { light, medium, heavy, moto, sourceId: MY_SOURCE_ID, speedTaper: e.speed }
      },
      undefined,
      TAPER_CLASSES,
      { sourceId: MY_SOURCE_ID, when: (_row, i) => !plan.has(i) },
    )
    stamped = res.matched
  }
  return { ...stats, stamped, prior }
}

async function main() {
  const H3R4_DIR = resolve(import.meta.dirname, `../data/prepared/${YEAR}/h3r4`)
  const bboxArg = process.argv.includes('--bbox') ? process.argv[process.argv.indexOf('--bbox') + 1] : ''
  const WRITE = process.argv.includes('--write')
  const BBOX = bboxArg.split(',').map(Number) as [number, number, number, number]
  if (!existsSync(H3R4_DIR)) {
    console.error(`ERROR: H3R4 directory not found: ${H3R4_DIR}`)
    process.exit(1)
  }
  if (BBOX.length !== 4 || BBOX.some((x) => !Number.isFinite(x))) {
    console.error('Usage: enrich-roads-taper.ts --bbox minLat,minLon,maxLat,maxLon [--write]')
    process.exit(1)
  }
  // v1: CZ-polygon-gated hexes only (mirror validity; module doc). The same
  // receiver-country approximation the engine makes: hex centroid = country.
  const inCz = makeCountryGate('CZ')
  const all = iterateCountryHexes(H3R4_DIR, BBOX)
  const hexes = all.filter((h) => {
    const [lat, lon] = cellToLatLng(h)
    return inCz(lat, lon)
  })
  if (hexes.length < all.length) {
    console.log(`  ${all.length - hexes.length} non-CZ hexes skipped (v1 mirrors the engine's CZ resolution only)`)
  }
  const CZ = COUNTRY_SPEED_TABLE['CZ']
  console.log(`taper ${WRITE ? 'WRITE' : 'dry-run'}: ${hexes.length} hexes in bbox ${BBOX.join(',')}`)

  const sum: TaperStats & { stamped: number; prior: number } = {
    boundaries: 0, skippedUnscaled: 0, speedOnly: 0, aadtOnly: 0, both: 0, kindCounts: {}, top: [], stamped: 0, prior: 0,
  }
  for (let at = 0; at < hexes.length; at += HEX_CONCURRENCY) {
    const chunk = hexes.slice(at, at + HEX_CONCURRENCY)
    const results = await Promise.all(
      chunk.map(async (hex) => ({ hex, r: await processHex(resolve(H3R4_DIR, hex, 'roads.arrow'), CZ, WRITE) })),
    )
    for (const { hex, r } of results) {
      sum.boundaries += r.boundaries
      sum.skippedUnscaled += r.skippedUnscaled
      sum.speedOnly += r.speedOnly
      sum.aadtOnly += r.aadtOnly
      sum.both += r.both
      sum.stamped += r.stamped
      sum.prior += r.prior
      sum.top.push(...r.top)
      console.log(
        `  ${hex}: ${r.boundaries} boundaries → plan speed-only ${r.speedOnly} / aadt-only ${r.aadtOnly} / both ${r.both}` +
          (r.prior ? ` (re-run over ${r.prior} prior stamps)` : '') +
          (WRITE ? ` → stamped ${r.stamped}` : ''),
      )
    }
  }
  sum.top.sort((x, y) => y.db - x.db)
  console.log(`\n=== Taper ${WRITE ? 'written' : 'plan'} ===`)
  console.log(`  boundaries ≥${TRIGGER_DB} dB: ${sum.boundaries} (unscaled-major mirror-skips: ${sum.skippedUnscaled})`)
  console.log(`  rows: speed-only ${sum.speedOnly}, aadt-only ${sum.aadtOnly}, both ${sum.both}` +
    (WRITE ? `, stamped ${sum.stamped}` : ''))
  console.log(`  top steps:`)
  for (const t of sum.top.slice(0, 15)) {
    console.log(`    ${t.db.toFixed(1)} dB  ${t.kind.padEnd(14)} #lat=${t.lat}&lng=${t.lon}&z=15&ro=road`)
  }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch((err) => {
    console.error('Error:', err)
    process.exit(1)
  })
}
