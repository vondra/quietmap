/**
 * Enrich DZ railways.arrow with SNTF/Algiers-Metro/tramway corridor-tier defaults.
 *
 * No open rail geometry or GTFS exists: SNTF (sntf.dz), ANESRIF (infrastructure),
 * EMA (Algiers Metro) and Setram (the 7 tramways) publish corporate HTML only —
 * no machine-readable feed. So — like the road layer — we apply tier defaults
 * over the OSM rail geometry, gated to Algerian soil by the country polygon.
 * Counts are trains/day in BOTH directions (engine convention).
 *
 * Algeria's rail context: SNTF runs North Africa's 2nd largest network (~4,200 km),
 * a partially 25 kV-electrified northern coastal spine (Oran↔Algiers↔Constantine↔
 * Annaba — among Africa's longest electrified sections). Freight is dominated by
 * the Tébessa (Djebel Onk) phosphate + Ouenza iron-ore lines feeding Annaba port
 * and the El Hadjar steel complex. Urban transit: Algiers Metro (2011, Africa's 2nd
 * heavy metro after Cairo) + 7 Alstom Citadis tramways (Algiers, Oran, Constantine,
 * Sétif, Sidi Bel Abbès, Ouargla — Africa's southernmost — and Mostaganem).
 *
 * ## Tiers (trains/day, both directions)
 *
 * | tier                                  | pax | frt | applied to |
 * |---------------------------------------|----:|----:|------------|
 * | Algiers Metro (light_rail)            | 150 |   0 | rail_type 2 |
 * | Algiers Tramway                       |  80 |   0 | tram near Algiers |
 * | Oran Tramway                          |  60 |   0 | tram near Oran |
 * | other tramways (Constantine/SBA/...)  |  40 |   0 | tram elsewhere |
 * | SNTF Northern main line               |  15 |  12 | Oran↔Algiers↔Annaba spine |
 * | Tébessa–Annaba phosphate+iron corridor|   1 |  18 | freight-dominated |
 * | secondary main (off-spine usage=main) |   4 |   6 | Hauts Plateaux / Oran–Tlemcen / Béchar |
 * | industrial spur                       |   0 |   6 | usage=industrial |
 * | other / branch                        |   1 |   3 | usage=branch + fallback |
 *
 * The `secondary main` tier is a refinement of the doc's original 3-row table:
 * ~78% of DZ heavy-rail rows are tagged usage=main but lie off the busy coastal
 * spine (interior Hauts Plateaux, Oran–Tlemcen, deep-south freight). Dumping all
 * of them to branch (1/3) underrates real main-line freight; 4/6 sits between the
 * electrified spine and a true branch.
 *
 * Counts are coarse (±30-40%) but freight/day moves Lden in ~1-2 dB steps, so this
 * is the right granularity given zero published per-line data. Full derivation in
 * data/enrichment/2026/dz/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-dz.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_DZ_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRailTrains } from './lib/railways-arrow.js'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_DZ_NATIONAL_RAILWAY


const DZ_HEX_BBOX: [number, number, number, number] = [18.9, -8.7, 37.1, 12.0]

// ── SNTF corridor polylines [lon, lat] — matched by min distance to the whole line ──
// Northern main spine, densified to follow the real winding coastal/Tell route
// (Oran ↔ Relizane ↔ Chlef ↔ Khemis Miliana ↔ Blida ↔ Algiers ↔ Thénia ↔ Bouira ↔
// BBA ↔ Sétif ↔ Constantine ↔ Guelma ↔ Annaba): a sparse 10-vertex chord missed
// ~36 k usage=main rows where the line curves >12 km off the straight segment.
const NORTHERN_MAIN: [number, number][] = [
  [-0.63, 35.70], [-0.10, 35.78], [0.56, 35.74], [1.01, 36.00], [1.33, 36.17],
  [2.22, 36.26], [2.83, 36.47], [3.06, 36.75], [3.55, 36.73], [3.90, 36.38],
  [4.76, 36.07], [5.41, 36.19], [5.69, 36.15], [6.61, 36.36], [7.10, 36.40],
  [7.43, 36.46], [7.77, 36.90],
]
// Phosphate (Djebel Onk/Tébessa) + iron-ore (Ouenza) freight lines → Annaba port
// & El Hadjar steel. SNTF's #1 freight commodity; passenger service minimal.
const PHOSPHATE_IRON: [number, number][] = [
  [7.95, 34.70], [8.12, 35.40], [8.13, 35.95], [7.77, 36.90],
]

const near = (lat: number, lon: number, line: [number, number][], m = 12000) => pointToPolylineDist(lat, lon, line) <= m

// Tram-city boxes (centre lat/lon, half-width deg). Algiers + Oran trams carry
// more than the smaller-city Citadis lines.
const nearBox = (lat: number, lon: number, clat: number, clon: number, half: number) =>
  lat >= clat - half && lat <= clat + half && lon >= clon - half && lon <= clon + half

/** Classify one rail/tram row into trains/day. null = leave at engine default. */
function classify(lat: number, lon: number, railType: number, usage: number): { pax: number; frt: number } | null {
  if (railType === 2) return { pax: 150, frt: 0 } // Algiers Metro (light_rail)
  if (railType === 1) {                            // tram
    if (nearBox(lat, lon, 36.75, 3.06, 0.25)) return { pax: 80, frt: 0 }  // Algiers
    if (nearBox(lat, lon, 35.70, -0.63, 0.15)) return { pax: 60, frt: 0 } // Oran
    return { pax: 40, frt: 0 }                      // Constantine/Sétif/SBA/Ouargla/Mostaganem
  }
  if (railType !== 0) return null                   // narrow_gauge / funicular → default
  if (usage === 2) return { pax: 0, frt: 6 }        // industrial spur

  // Freight corridor first (phosphate/iron dominates), then the passenger trunk.
  if (near(lat, lon, PHOSPHATE_IRON)) return { pax: 1, frt: 18 }
  if (near(lat, lon, NORTHERN_MAIN)) return { pax: 15, frt: 12 }

  if (usage === 1) return { pax: 1, frt: 3 }        // branch
  return { pax: 4, frt: 6 }                          // secondary main off-spine (Oran-Tlemcen, Hauts Plateaux, Béchar)
}

async function main() {
  console.log(`=== DZ Railway Enrichment — SNTF/Metro/tramway corridor-tier defaults (${YEAR}) ===\n`)

  const inDZ = makeCountryGate('DZ')
  const hexDirs = iterateCountryHexes(H3R4_DIR, DZ_HEX_BBOX, 'railways.arrow')
  console.log(`  DZ-bbox hexes with railways.arrow: ${hexDirs.length}\n`)

  let totalRows = 0, enriched = 0, hexesUpdated = 0, preserved = 0, skippedService = 0
  const tierCount: Record<string, number> = {}
  let sumPax = 0, sumFrt = 0
  const startTime = Date.now()

  const tierKey = (pax: number, frt: number): string =>
    pax === 150 ? 'algiers-metro' : pax === 80 ? 'algiers-tram' : pax === 60 ? 'oran-tram'
    : pax === 40 ? 'other-tram' : pax === 15 ? 'northern-main' : pax === 1 && frt === 18 ? 'phosphate-iron'
    : pax === 0 && frt === 6 ? 'industrial' : pax === 4 ? 'secondary-main' : 'branch'

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRailTrains(
      resolve(H3R4_DIR, hex, 'railways.arrow'),
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        if (!inBbox(row.midLat, row.midLon, DZ_HEX_BBOX)) return null
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
      inDZ, // #31.7 central country gate — see writeRailTrains
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
