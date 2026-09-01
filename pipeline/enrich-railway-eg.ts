/**
 * Enrich EG railways.arrow with ENR/Cairo-Metro/Alex-Tram corridor-tier defaults.
 *
 * No open rail geometry or GTFS exists: ENR (enr.gov.eg) redirects to a Vaadin
 * login, the National Authority for Tunnels (Cairo Metro) publishes nothing
 * machine-readable, and the Alexandria Tram has no feed. So — like the road layer
 * — we apply tier defaults over the OSM rail geometry, gated to Egyptian soil by
 * the country polygon. Counts are trains/day in BOTH directions (engine convention).
 *
 * Egypt's rail context: ENR is Africa's oldest railway (Alexandria↔Cairo, 1854),
 * ~5,500 km, road-dominated freight (the Nile Valley is too narrow for rail to win
 * the modal split). Cairo Metro is Africa's first metro (1987, 3 lines, ~3.5 M
 * pax/day). Alexandria Tram is Africa's oldest operational tram (since 1863).
 *
 * ## Tiers (trains/day, both directions)
 *
 * | tier                              | pax | frt | applied to |
 * |-----------------------------------|----:|----:|------------|
 * | Cairo↔Alexandria (busiest ENR)    | 100 |  30 | Delta mainline via Banha/Tanta |
 * | Cairo↔Upper Egypt (Luxor/Aswan)   |  40 |  15 | Nile-Valley spine S of Cairo |
 * | Cairo↔Suez Canal                  |  20 |  15 | Ismailia/Port Said/Suez |
 * | main (other operational)          |  10 |   8 | usage=main fallback |
 * | branch                            |   1 |   4 | usage=branch |
 * | industrial spur                   |   0 |   6 | usage=industrial |
 * | Cairo Metro (light_rail)          | 400 |   0 | rail_type 2 |
 * | Alexandria Tram (tram)            | 250 |   0 | rail_type 1 |
 *
 * Counts are coarse (±30-40%) but freight/day moves Lden in ~1-2 dB steps, so this
 * is the right granularity given zero published per-line data. Full derivation in
 * data/enrichment/2026/eg/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-eg.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_EG_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRailTrains } from './lib/railways-arrow.js'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_EG_NATIONAL_RAILWAY


const EG_HEX_BBOX: [number, number, number, number] = [22.0, 24.7, 31.7, 36.9]

// ── ENR corridor polylines [lon, lat] — matched by min distance to the whole line ──
const CAIRO_ALEX: [number, number][] = [
  [31.247, 30.063], [31.18, 30.46], [30.97, 30.79], [30.47, 31.03], [29.90, 31.19],
]
const UPPER_EGYPT: [number, number][] = [
  [31.247, 30.063], [31.10, 29.07], [30.75, 28.10], [31.18, 27.18],
  [31.70, 26.56], [32.72, 26.16], [32.64, 25.70], [32.90, 24.09],
]
// Cairo→Zagazig→Ismailia→Port Said, plus the Ismailia→Suez branch.
const SUEZ_CANAL: [number, number][][] = [
  [[31.247, 30.063], [31.50, 30.59], [32.27, 30.60], [32.30, 31.26]],
  [[32.27, 30.60], [32.55, 29.97]],
]

const near = (lat: number, lon: number, line: [number, number][], m = 10000) => pointToPolylineDist(lat, lon, line) <= m

/** Classify one rail/tram row into trains/day. null = leave at engine default. */
function classify(lat: number, lon: number, railType: number, usage: number): { pax: number; frt: number } | null {
  if (railType === 1) return { pax: 250, frt: 0 } // Alexandria Tram
  if (railType === 2) return { pax: 400, frt: 0 } // Cairo Metro (light_rail)
  if (railType !== 0) return null                 // narrow_gauge / funicular → default
  if (usage === 2) return { pax: 0, frt: 6 }      // industrial spur

  // Busiest corridor first; near Cairo all three converge and Cairo↔Alex wins.
  if (near(lat, lon, CAIRO_ALEX)) return { pax: 100, frt: 30 }
  if (near(lat, lon, UPPER_EGYPT)) return { pax: 40, frt: 15 }
  if (SUEZ_CANAL.some(l => near(lat, lon, l))) return { pax: 20, frt: 15 }

  if (usage === 1) return { pax: 1, frt: 4 }      // branch
  return { pax: 10, frt: 8 }                       // other operational main
}

async function main() {
  console.log(`=== EG Railway Enrichment — ENR/Metro/Tram corridor-tier defaults (${YEAR}) ===\n`)

  const inEG = makeCountryGate('EG')
  const hexDirs = iterateCountryHexes(H3R4_DIR, EG_HEX_BBOX, 'railways.arrow')
  console.log(`  EG-bbox hexes with railways.arrow: ${hexDirs.length}\n`)

  let totalRows = 0, enriched = 0, hexesUpdated = 0, preserved = 0, skippedService = 0
  const tierCount: Record<string, number> = {}
  let sumPax = 0, sumFrt = 0
  const startTime = Date.now()

  const tierKey = (pax: number, frt: number): string =>
    pax === 400 ? 'cairo-metro' : pax === 250 ? 'alex-tram'
    : pax === 100 ? 'cairo-alex' : pax === 40 ? 'upper-egypt' : pax === 20 ? 'suez-canal'
    : pax === 0 && frt === 6 ? 'industrial' : pax === 1 ? 'branch' : 'main'

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRailTrains(
      resolve(H3R4_DIR, hex, 'railways.arrow'),
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        if (!inBbox(row.midLat, row.midLon, EG_HEX_BBOX)) return null
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
      inEG, // #31.7 central country gate — see writeRailTrains
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
