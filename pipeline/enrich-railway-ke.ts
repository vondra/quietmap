/**
 * Enrich KE railways.arrow with SGR / Nairobi-commuter corridor-tier defaults.
 *
 * Kenya Railways publishes no GIS/GTFS — not even for the flagship SGR. So, like
 * the road layer, we apply corridor-tier defaults over the OSM rail geometry, gated
 * to Kenyan soil by the country polygon. Counts are trains/day in BOTH directions
 * (engine convention).
 *
 * Kenya's rail context:
 *   - **SGR (Madaraka Express)** — standard gauge, the only busy line:
 *       Phase 1 Mombasa↔Nairobi, 472 km, opened 2017. ~2.7 M pax/yr + heavy
 *       container freight to the Embakasi ICD (Northern Corridor port traffic).
 *       Phase 2A Nairobi↔Suswa/Naivasha, 120 km, opened 2019 — lighter use.
 *   - **Nairobi Commuter Rail** — 4 radial lines on the old metre gauge
 *       (Syokimau / Embakasi / Ruiru / Kikuyu), modest but frequent service.
 *   - **Old metre gauge** — colonial (1895–1970s), mostly defunct branches
 *       (Nakuru↔Kisumu, Nanyuki, Nyahururu, Magadi soda line).
 *   - **No trams, light rail, or metros** anywhere in Kenya.
 *
 * ## Tiers (trains/day, both directions)
 *
 * | tier                              | pax | frt | applied to |
 * |-----------------------------------|----:|----:|------------|
 * | SGR Phase 1 (Mombasa↔Nairobi)     |   8 |  20 | container corridor + ICD |
 * | SGR Phase 2A (Nairobi↔Naivasha)   |   4 |   8 | Rift extension |
 * | Nairobi commuter (≤30 km of CBD)  |  20 |   4 | old-MGR radial lines |
 * | industrial spur                   |   0 |   4 | usage=industrial (Magadi etc.) |
 * | old metre gauge (defunct)         |   1 |   4 | everything else |
 *
 * Counts are coarse (±30-40%) but freight/day moves Lden in ~1-2 dB steps, so this
 * is the right granularity given zero published per-line data. SGR is checked before
 * the commuter zone so its Nairobi-terminus freight (frt 20 → Embakasi ICD) isn't
 * mislabeled as low-freight commuter. Full derivation in
 * data/enrichment/2026/ke/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-ke.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_KE_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
import { inBbox, flatDist, pointToPolylineDist } from './lib/spatial.js'
import { writeRailTrains } from './lib/railways-arrow.js'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_KE_NATIONAL_RAILWAY


const KE_HEX_BBOX: [number, number, number, number] = [-4.7, 33.9, 5.5, 41.9]

// ── SGR corridor polylines [lon, lat] — matched by min distance to the whole line ──
// Phase 1: Mombasa (Miritini) → Voi → Mtito Andei → Emali → Athi River → Nairobi (Syokimau).
const SGR_PHASE1: [number, number][] = [
  [39.55, -4.03], [38.86, -3.50], [38.556, -3.396], [38.169, -2.690],
  [37.66, -2.30], [37.12, -1.90], [36.95, -1.45], [36.90, -1.34],
]
// Phase 2A: Nairobi (Syokimau) → Ongata Rongai → Ngong → Mai Mahiu → Suswa (Naivasha).
const SGR_PHASE2A: [number, number][] = [
  [36.90, -1.34], [36.70, -1.40], [36.55, -1.30], [36.42, -1.05], [36.35, -0.95],
]

// Nairobi CBD — commuter radial lines (old MGR) fan out within ~30 km of here.
const NAIROBI_LAT = -1.286, NAIROBI_LON = 36.817

const near = (lat: number, lon: number, line: [number, number][], m = 12000) => pointToPolylineDist(lat, lon, line) <= m

/** Classify one rail row into trains/day. null = leave at engine default. */
function classify(lat: number, lon: number, railType: number, usage: number): { pax: number; frt: number } | null {
  if (railType !== 0) return null                  // Kenya has no tram / light_rail / metro
  if (usage === 2) return { pax: 0, frt: 4 }       // industrial spur (Magadi soda line etc.)

  // SGR first so its Nairobi-terminus freight isn't swallowed by the commuter zone.
  if (near(lat, lon, SGR_PHASE1)) return { pax: 8, frt: 20 }
  if (near(lat, lon, SGR_PHASE2A)) return { pax: 4, frt: 8 }

  // Remaining rail within ~30 km of the Nairobi CBD is old-MGR commuter service.
  if (flatDist(lat, lon, NAIROBI_LAT, NAIROBI_LON) <= 30000) return { pax: 20, frt: 4 }

  if (usage === 1) return { pax: 1, frt: 4 }       // branch (defunct metre gauge)
  return { pax: 1, frt: 4 }                         // other old metre gauge
}

async function main() {
  console.log(`=== KE Railway Enrichment — SGR/commuter corridor-tier defaults (${YEAR}) ===\n`)

  const inKE = makeCountryGate('KE')
  const hexDirs = iterateCountryHexes(H3R4_DIR, KE_HEX_BBOX, 'railways.arrow')
  console.log(`  KE-bbox hexes with railways.arrow: ${hexDirs.length}\n`)

  let totalRows = 0, enriched = 0, hexesUpdated = 0, preserved = 0, skippedService = 0
  const tierCount: Record<string, number> = {}
  let sumPax = 0, sumFrt = 0
  const startTime = Date.now()

  const tierKey = (pax: number, frt: number): string =>
    pax === 8 ? 'sgr-phase1' : pax === 4 ? 'sgr-phase2a' : pax === 20 ? 'nairobi-commuter'
    : pax === 0 ? 'industrial' : 'metre-gauge'

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRailTrains(
      resolve(H3R4_DIR, hex, 'railways.arrow'),
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        if (!inBbox(row.midLat, row.midLon, KE_HEX_BBOX)) return null
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
      inKE, // #31.7 central country gate — see writeRailTrains
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
