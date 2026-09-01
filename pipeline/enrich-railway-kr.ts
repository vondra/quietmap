/**
 * KR railways.arrow heal pass — retracts the legacy KORAIL class-default stamps.
 *
 * Korea publishes no public GTFS for KORAIL, Seoul Metro, Busan Metro, etc.
 * (privacy concerns + government portal geofencing). All open data sources
 * (data.go.kr, KTDB, KRIC) require Korean i-PIN authentication or KR IP.
 * There is therefore NO measured train-count data to stamp: every row this
 * source id ever wrote was a CNOSSOS class default masquerading as national
 * data. That fallback was purged 2026-07-10 — unknown rows stay source_id=0
 * and the ENGINE default table
 * (engine/noise-compute/src/emission/railway.rs::default_traffic) owns them.
 *
 * What remains here is the self-heal: a retract pass that disowns the
 * pre-2026-07-10 stamps (exact OLD_FALLBACK tuple match; KR has no
 * matched-stops structure, so tuple-only is the accepted signature — there are
 * no real matches to protect). DELETE this whole file after the world rail
 * repaint confirms 0 retractions.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-kr.ts
 */

import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import { writeRailTrains, type RailRow } from './lib/railways-arrow.js'
import { makeCountryGate, segmentWhollyOutside } from './lib/country-polygon.js'
import { SOURCE_ID_KR_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
import { H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_KR_NATIONAL_RAILWAY


// Coarse hex shortlist [minLat,minLon,maxLat,maxLon] — the same enumeration the
// legacy stamper used, so every hex it could have written is revisited (the
// retract itself is ownership-scoped: only rows carrying MY_SOURCE_ID).
const KR_BBOX: [number, number, number, number] = [33, 124.5, 39, 132]

// Retract signature for stamps the pre-2026-07-10 fallback design wrote: KR's deleted
// class-default table, verbatim (pax only — the stamper always wrote frt=0).
// rail_type: 0=rail 1=tram 2=light_rail 3=narrow_gauge 4=funicular; usage: 0=main 1=branch 2=industrial
const OLD_FALLBACK = (railType: number, usage: number): number => {
  if (railType === 2) return 250  // light_rail (urban)
  if (railType === 1) return 200  // tram
  if (railType === 3) return 30   // narrow gauge
  if (railType === 4) return 30   // funicular
  if (usage === 1) return 80      // branch
  if (usage === 2) return 20      // industrial
  return 200                      // main line — KORAIL trunk
}
const wasOldFallbackStamp = (row: RailRow): boolean =>
  row.existingFrt === 0 && row.existingPax === OLD_FALLBACK(row.railType, row.usage)

async function main() {
  console.log(`=== KR Railway Heal — retract legacy KORAIL class-default stamps ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}\n`)

  const hexDirs = iterateCountryHexes(H3R4_DIR, KR_BBOX, 'railways.arrow')
  console.log(`  KR-bbox hexes with railways.arrow: ${hexDirs.length}\n`)

  // Created here (not module scope): makeCountryGate may download+convert the
  // CGAZ polygon on first run — keep that off the import path.
  const inKr = makeCountryGate('KR')

  let totalRows = 0, totalRetracted = 0, hexesUpdated = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const r = await writeRailTrains(
      resolve(H3R4_DIR, hexDirs[hi], 'railways.arrow'),
      // No timetable source exists for KR (see header) — nothing is ever stamped.
      () => null,
      undefined,
      // CRITICAL-1b EXEMPTION — deliberately NO retractSafe gate: kr is a pure heal
      // with NO inputs by design, so there is no input snapshot whose incompleteness
      // could fake "no coverage" (the failure mode the gate exists for elsewhere).
      // Tuple-only retraction is the intended signature: nothing real can carry the
      // KORAIL source id + the exact OLD_FALLBACK tuple — every such row is a legacy
      // class-default stamp; no measured KR count was ever written, see header.
      {
        sourceId: MY_SOURCE_ID,
        when: (row) => {
          // Country-bleed disown (#26C/#31.7): any owned row WHOLLY outside KR
          // (start+mid+end — the shared R9 predicate) is foreign track the
          // KORAIL id must not speak for (KR_BBOX brushes North Korea and
          // Kyushu/Tsushima). Midpoint-only here used to delete genuine
          // DMZ-crossing straddlers every run.
          if (segmentWhollyOutside(inKr, row.midLat, row.midLon, row.startLat, row.startLon, row.endLat, row.endLon)) return true
          return wasOldFallbackStamp(row)
        },
      },
      inKr, // #31.7 central country gate — see writeRailTrains
    )
    totalRows += r.rows
    totalRetracted += r.retracted
    if (r.updated) hexesUpdated++

    if (hi % 50 === 0 || hi === hexDirs.length - 1) {
      const elapsed = ((Date.now() - startTime) / 1000).toFixed(0)
      console.log(`  [${elapsed}s] ${hi + 1}/${hexDirs.length} hexes, ${hexesUpdated} updated, ${totalRetracted.toLocaleString()} retracted`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  Total scanned: ${totalRows.toLocaleString()}`)
  console.log(`  Retracted legacy defaults: ${totalRetracted.toLocaleString()}`)
  console.log(`  Hexes updated: ${hexesUpdated}/${hexDirs.length}`)
  console.log(`\n=== Done ===`)
}

// Import-safe: run only when invoked directly — importing this file must never
// trigger an enrichment pass (pattern from enrich-roads-cz.ts).
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch(err => { console.error('Error:', err); process.exit(1) })
}
