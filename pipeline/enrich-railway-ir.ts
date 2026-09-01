/**
 * Enrich IR railways.arrow with RAI / urban-metro corridor-tier defaults.
 *
 * No open rail data exists: RAI (Islamic Republic of Iran Railways) publishes no
 * GTFS — timetables are PDF/HTML only — and the urban metros (Tehran/Mashhad/
 * Isfahan/Tabriz/Shiraz) have no machine-readable feeds. So — like the road layer
 * — we apply tier defaults over the OSM rail geometry, gated to Iranian soil by
 * the country polygon. Counts are trains/day in BOTH directions (engine convention).
 *
 * Iran's rail context: RAI runs ~13,000 km of 1,435 mm standard gauge radiating
 * from Tehran. Freight-heavy on the long SE container line to Bandar Abbas; the
 * pilgrim corridor to Mashhad is the busiest for passengers. The metros are mostly
 * underground (OSM `railway=subway`, which the extractor drops — correctly, since
 * underground track emits no surface noise); only the SURFACE light_rail/tram
 * segments (e.g. Tehran Line 5 to Karaj, Mashhad/Isfahan at-grade stretches) reach
 * railways.arrow as rail_type 2/1 and get the metro tier here.
 *
 * ## Tiers (trains/day, both directions)
 *
 * | tier                              | pax | frt | applied to |
 * |-----------------------------------|----:|----:|------------|
 * | Tehran surface metro              | 350 |   0 | rail_type 1/2 in Greater Tehran |
 * | other metros (Mashhad/Isfahan/…)  |  80 |   0 | rail_type 1/2 elsewhere |
 * | Tehran↔Mashhad (pilgrim spine)    |  15 |   8 | busiest RAI passenger corridor |
 * | Tehran↔Isfahan↔Shiraz (centre)    |   8 |  10 | central trunk |
 * | Tehran↔Tabriz (NW)                |   5 |   8 | Azerbaijan link |
 * | Tehran↔Bandar Abbas (SE)          |   3 |  10 | ~1,500 km container freight |
 * | other / branch                    |   3 |   5 | usage main-not-on-corridor / branch |
 * | industrial spur                   |   0 |   5 | usage=industrial |
 *
 * Counts are coarse (±30-40%) but freight/day moves Lden in ~1-2 dB steps, so this
 * is the right granularity given zero published per-line data. Full derivation in
 * data/enrichment/2026/ir/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-ir.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_IR_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRailTrains } from './lib/railways-arrow.js'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_IR_NATIONAL_RAILWAY


const IR_HEX_BBOX: [number, number, number, number] = [25.0, 44.0, 39.8, 63.5]

// Greater Tehran box (lat/lon half-widths) — wide enough to catch the surface
// Line 5 express out to Karaj. Surface metro here = 350/day, elsewhere = 80/day.
const TEHRAN = { lat: 35.70, lon: 51.42, half: 0.45 }
const inTehran = (lat: number, lon: number) =>
  Math.abs(lat - TEHRAN.lat) <= TEHRAN.half && Math.abs(lon - TEHRAN.lon) <= TEHRAN.half

// ── RAI corridor polylines [lon, lat] — matched by min distance to the whole line ──
const MASHHAD: [number, number][] = [
  [51.42, 35.70], [53.39, 35.58], [54.98, 36.42], [56.40, 36.30], [58.80, 36.22], [59.57, 36.30],
]
const ISFAHAN_SHIRAZ: [number, number][] = [
  [51.42, 35.70], [50.88, 34.64], [51.45, 33.98], [51.67, 32.65], [52.20, 31.20], [52.53, 29.59],
]
const TABRIZ: [number, number][] = [
  [51.42, 35.70], [50.00, 36.27], [48.49, 36.68], [47.70, 37.45], [46.29, 38.08],
]
const BANDAR_ABBAS: [number, number][] = [
  [51.42, 35.70], [50.88, 34.64], [51.90, 32.00], [54.37, 31.90], [55.40, 31.60], [55.95, 29.00], [56.28, 27.18],
]

const near = (lat: number, lon: number, line: [number, number][], m = 9000) => pointToPolylineDist(lat, lon, line) <= m

/** Classify one rail/tram row into trains/day. null = leave at engine default. */
function classify(lat: number, lon: number, railType: number, usage: number): { pax: number; frt: number } | null {
  if (railType === 1 || railType === 2) {                     // surface tram / light_rail = urban metro
    return inTehran(lat, lon) ? { pax: 350, frt: 0 } : { pax: 80, frt: 0 }
  }
  if (railType !== 0) return null                             // narrow_gauge / funicular → default
  if (usage === 2) return { pax: 0, frt: 5 }                  // industrial spur

  // Busiest corridor first; near Tehran the radials converge and pax-spine wins.
  if (near(lat, lon, MASHHAD)) return { pax: 15, frt: 8 }
  if (near(lat, lon, ISFAHAN_SHIRAZ)) return { pax: 8, frt: 10 }
  if (near(lat, lon, TABRIZ)) return { pax: 5, frt: 8 }
  if (near(lat, lon, BANDAR_ABBAS)) return { pax: 3, frt: 10 }

  return { pax: 3, frt: 5 }                                   // other operational main / branch
}

async function main() {
  console.log(`=== IR Railway Enrichment — RAI/metro corridor-tier defaults (${YEAR}) ===\n`)

  const inIR = makeCountryGate('IR')
  const hexDirs = iterateCountryHexes(H3R4_DIR, IR_HEX_BBOX, 'railways.arrow')
  console.log(`  IR-bbox hexes with railways.arrow: ${hexDirs.length}\n`)

  let totalRows = 0, enriched = 0, hexesUpdated = 0, preserved = 0, skippedService = 0
  const tierCount: Record<string, number> = {}
  let sumPax = 0, sumFrt = 0
  const startTime = Date.now()

  const tierKey = (pax: number, frt: number): string =>
    pax === 350 ? 'tehran-metro' : pax === 80 ? 'other-metro'
    : pax === 15 ? 'mashhad' : pax === 8 ? 'isfahan-shiraz' : pax === 5 ? 'tabriz'
    : pax === 3 && frt === 10 ? 'bandar-abbas' : pax === 0 ? 'industrial' : 'main-branch'

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRailTrains(
      resolve(H3R4_DIR, hex, 'railways.arrow'),
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        if (!inBbox(row.midLat, row.midLon, IR_HEX_BBOX)) return null
        const t = classify(row.midLat, row.midLon, row.railType, row.usage)
        if (!t) return null
        return { pax: t.pax, frt: t.frt, sourceId: MY_SOURCE_ID }
      },
      (_row, _i, applied) => {
        enriched++
        const k = tierKey(applied.pax, applied.frt)
        tierCount[k] = (tierCount[k] || 0) + 1
        sumPax += applied.pax
        sumFrt += applied.frt
      },
      undefined,
      inIR, // #31.7 central country gate — see writeRailTrains
    )
    totalRows += r.rows
    skippedService += r.skippedService
    if (r.updated) hexesUpdated++

    const elapsed = Date.now() - startTime
    if (elapsed > 10_000 && hi % 20 === 0) {
      console.log(`  [${(elapsed / 1000).toFixed(0)}s] ${hi + 1}/${hexDirs.length} hexes, ${enriched.toLocaleString()} enriched`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  Total rail rows scanned: ${totalRows.toLocaleString()}`)
  console.log(`  Skipped service tracks:  ${skippedService.toLocaleString()}`)
  console.log(`  Preserved (higher prio): ${preserved.toLocaleString()}`)
  console.log(`  Enriched:                ${enriched.toLocaleString()}`)
  console.log(`  Hexes updated:           ${hexesUpdated}/${hexDirs.length}`)
  console.log(`\n  By tier:`)
  for (const [k, v] of Object.entries(tierCount).sort((a, b) => b[1] - a[1])) {
    console.log(`    ${k}: ${v.toLocaleString()}`)
  }
  console.log(`\n  Mean pax/day: ${(sumPax / Math.max(enriched, 1)).toFixed(0)}, mean frt/day: ${(sumFrt / Math.max(enriched, 1)).toFixed(0)}`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
