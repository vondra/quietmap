/**
 * Enrich UA railways.arrow with Ukrzaliznytsia / tram corridor-tier defaults.
 *
 * NOTE: Ukraine has been under Russian invasion since February 2022. This is a
 * **pre-war baseline**. Rail is critical wartime logistics (grain-export
 * corridors to the EU border + Black Sea ports, refugee evacuation, military
 * supply) but per-line traffic is now a state secret; occupied/frontline track
 * (Donbas, Zaporizhzhia, Kherson, Mariupol) is destroyed or military-only.
 *
 * Ukrzaliznytsia (Укрзалізниця) publishes no open GTFS and no per-segment
 * traffic — schedules live behind the booking site (booking.uz.gov.ua), and
 * вантажонапруженість (freight density) per line is internal. So — like RU/DZ —
 * we apply tier defaults over OSM rail geometry, gated to Ukrainian soil by the
 * country polygon. Counts are trains/day in BOTH directions (engine convention).
 *
 * Ukraine's network is ~22,300 km of 1,520 mm broad gauge — one of the world's
 * largest — strongly mixed passenger+freight (iron ore Kryvyi Rih, steel, grain,
 * coal pre-war). Freight is the acoustically dominant case (long heavy consists),
 * so the freight/day figure is the load-bearing one on the trunk lines.
 *
 * ## rail_type coverage (OSM `railway=*` → engine code, classify.rs)
 *   The three metros (Kyiv / Kharkiv / Dnipro) are `railway=subway`, which the
 *   extractor does NOT emit (classify.rs allowlist = rail/tram/light_rail/
 *   narrow_gauge/funicular) — and underground metro radiates negligible surface
 *   noise anyway, so their absence is correct. What we DO classify:
 *     0 rail        Ukrzaliznytsia heavy rail (trunk vs branch vs industrial)
 *     1 tram        ~20 Soviet-era city tram systems (extensive, frequent)
 *     2 light_rail  Kryvyi Rih metrotram (швидкісний трамвай, partly underground,
 *                   metro-like) + Kyiv fast-tram lines
 *     3 narrow_gauge Carpathian lines (Borzhava etc.) — mostly tourist/light
 *     4 funicular   Kyiv funicular — left at engine default (tiny)
 *
 * ## Tiers (trains/day, both directions) — heavy rail anchored to docs/about/europe/ua.md
 *
 * | tier                    | pax | frt | applied to |
 * |-------------------------|----:|----:|------------|
 * | major-city tram         | 180 |   0 | Kyiv/Kharkiv/Odesa/Lviv/Dnipro/Kryvyi Rih |
 * | other tram              |  90 |   0 | smaller city tram networks |
 * | metrotram / fast tram   | 150 |   0 | light_rail (Kryvyi Rih + Kyiv) |
 * | other light_rail        | 120 |   0 | light_rail elsewhere |
 * | UZ main trunk           |  20 |  15 | usage=main heavy rail |
 * | UZ branch               |   5 |   8 | usage=branch |
 * | industrial spur         |   0 |   6 | usage=industrial |
 * | narrow gauge            |   2 |   1 | Carpathian tourist lines |
 *
 * Counts are coarse (±30-40%) but freight/day moves Lden in ~1-2 dB steps, so this
 * is the right granularity given zero published per-line data. The usage=main/branch
 * split (OSM `usage=*`) gives the trunk-vs-branch distinction for free — far better
 * than a flat default. Full derivation in data/enrichment/2026/ua/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-ua.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_UA_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
import { inBbox } from './lib/spatial.js'
import { writeRailTrains } from './lib/railways-arrow.js'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_UA_NATIONAL_RAILWAY


const UA_HEX_BBOX: [number, number, number, number] = [44.0, 22.0, 52.5, 40.5]

// ── Tram / light-rail city boxes (centre lat/lon, half-width deg) ──
// Major tram networks (frequent, high ridership) vs the metrotram/fast-tram
// light-rail. Kryvyi Rih has BOTH a large classic tram net and the metrotram.
const nearBox = (lat: number, lon: number, clat: number, clon: number, half: number) =>
  lat >= clat - half && lat <= clat + half && lon >= clon - half && lon <= clon + half

interface CityPt { name: string; lat: number; lon: number; half: number }
const MAJOR_TRAM_CITIES: CityPt[] = [
  { name: 'Kyiv',       lat: 50.45, lon: 30.52, half: 0.28 },
  { name: 'Kharkiv',    lat: 49.99, lon: 36.23, half: 0.20 },
  { name: 'Odesa',      lat: 46.48, lon: 30.72, half: 0.20 },
  { name: 'Lviv',       lat: 49.84, lon: 24.03, half: 0.15 },
  { name: 'Dnipro',     lat: 48.46, lon: 35.04, half: 0.20 },
  { name: 'Kryvyi Rih', lat: 47.91, lon: 33.39, half: 0.30 }, // long N-S city along the ore belt
]
// light_rail (rail_type 2) hotspots: Kryvyi Rih metrotram + Kyiv fast-tram lines.
const LIGHT_RAIL_HIGH: CityPt[] = [
  { name: 'Kryvyi Rih metrotram', lat: 47.91, lon: 33.39, half: 0.30 },
  { name: 'Kyiv fast tram',       lat: 50.46, lon: 30.45, half: 0.28 },
]
const inAnyBox = (lat: number, lon: number, pts: CityPt[]) =>
  pts.some(c => nearBox(lat, lon, c.lat, c.lon, c.half))

/** Classify one rail/tram row into trains/day. null = leave at engine default. */
function classify(lat: number, lon: number, railType: number, usage: number): { pax: number; frt: number } | null {
  if (railType === 1) // tram
    return inAnyBox(lat, lon, MAJOR_TRAM_CITIES) ? { pax: 180, frt: 0 } : { pax: 90, frt: 0 }
  if (railType === 2) // light_rail (metrotram / fast tram)
    return inAnyBox(lat, lon, LIGHT_RAIL_HIGH) ? { pax: 150, frt: 0 } : { pax: 120, frt: 0 }
  if (railType === 3) return { pax: 2, frt: 1 }   // narrow gauge (Carpathian tourist lines)
  if (railType !== 0) return null                  // funicular → engine default

  // Heavy rail (Ukrzaliznytsia): usage gives trunk vs branch vs industrial.
  if (usage === 2) return { pax: 0, frt: 6 }       // industrial spur
  if (usage === 1) return { pax: 5, frt: 8 }       // branch
  return { pax: 20, frt: 15 }                       // main trunk
}

async function main() {
  console.log(`=== UA Railway Enrichment — Ukrzaliznytsia/tram corridor-tier defaults, pre-war baseline (${YEAR}) ===\n`)

  const inUA = makeCountryGate('UA')
  const hexDirs = iterateCountryHexes(H3R4_DIR, UA_HEX_BBOX, 'railways.arrow')
  console.log(`  UA-bbox hexes with railways.arrow: ${hexDirs.length}\n`)

  let totalRows = 0, enriched = 0, hexesUpdated = 0, preserved = 0, skippedService = 0
  const tierCount: Record<string, number> = {}
  let sumPax = 0, sumFrt = 0
  const startTime = Date.now()

  const tierKey = (pax: number, frt: number): string =>
    pax === 180 ? 'major-tram' : pax === 90 ? 'other-tram' : pax === 150 ? 'metrotram/fast-tram'
    : pax === 120 ? 'other-light-rail' : pax === 20 ? 'UZ-main-trunk' : pax === 5 ? 'UZ-branch'
    : pax === 0 && frt === 6 ? 'industrial-spur' : 'narrow-gauge'

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRailTrains(
      resolve(H3R4_DIR, hex, 'railways.arrow'),
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        if (!inBbox(row.midLat, row.midLon, UA_HEX_BBOX)) return null
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
      inUA, // #31.7 central country gate — see writeRailTrains
    )
    totalRows += r.rows
    skippedService += r.skippedService
    if (r.updated) hexesUpdated++

    const elapsed = Date.now() - startTime
    if (elapsed > 10_000 && hi % 100 === 0) {
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
