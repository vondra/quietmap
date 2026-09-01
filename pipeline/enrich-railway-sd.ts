/**
 * Enrich SD railways.arrow with Sudan Railways corridor-tier defaults.
 *
 * No open rail geometry, GTFS or timetable exists: Sudan Railways Corporation
 * (SRC, sudanrailways.gov.sd) publishes corporate HTML only, and the network is
 * barely operational after decades of disinvestment + the 2023– civil war. So —
 * like the road layer — we apply corridor/usage tier defaults over the OSM rail
 * geometry, gated to Sudanese soil by the country polygon. Counts are trains/day
 * in BOTH directions (engine convention).
 *
 * Sudan's rail context: SRC runs ~4,750 km of 1,067 mm CAPE (narrow) gauge —
 * historically one of Africa's largest networks, now severely degraded with only
 * a handful of trains/day. There is NO standard-gauge, NO metro/tram/light-rail
 * anywhere in the country.
 *
 *   ⇒ KEY DIFFERENCE vs the EU/DZ templates: Sudan's mainlines are tagged in OSM
 *     as EITHER `railway=rail` (rail_type 0) OR `railway=narrow_gauge`
 *     (rail_type 3) — mappers are inconsistent for Cape-gauge networks. We treat
 *     BOTH as Sudan Railways heavy rail. (rail_type 1/2/4 = tram/light_rail/
 *     funicular do not exist here → left at engine default.)
 *
 * ## Tiers (trains/day, both directions)
 *
 * | corridor                                   | pax | frt | applied to |
 * |--------------------------------------------|----:|----:|------------|
 * | Khartoum–Atbara–Haiya–Port Sudan (artery)  |  1  |  3  | main freight to the coast |
 * | Khartoum–Sennar–Kosti–El Obeid (western)   |  1  |  2  | gum-arabic / Kordofan line |
 * | Khartoum–Atbara–Abu Hamed–Wadi Halfa (N)   |  1  |  1  | Nile-valley north, near-dormant |
 * | Sennar–Gedaref–Kassala–Port Sudan (eastern)|  0  |  1  | eastern freight, intermittent |
 * | industrial spur (usage=2)                  |  0  |  1  | sugar/port spurs |
 * | branch / off-corridor fallback             |  0  |  1  | degraded branch lines |
 *
 * Counts are deliberately low — this network moves only a few trains/day at best
 * and much of it is dormant. At 1-3 freight/day rail noise is minor next to road
 * noise, but non-zero on the corridors. Coarse (the real timetable is unpublished
 * and fluid in wartime); full derivation in data/enrichment/2026/sd/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-sd.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_SD_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRailTrains } from './lib/railways-arrow.js'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_SD_NATIONAL_RAILWAY


const SD_HEX_BBOX: [number, number, number, number] = [8.7, 21.8, 22.2, 38.6]

// ── Sudan Railways corridor polylines [lon, lat] — matched by min distance to
// the whole line. Coarse chords over long desert routes; an 18 km tolerance
// absorbs the winding real alignment without crossing onto a parallel corridor
// (the lines are far apart). Junction city: Atbara (N) and Sennar (S/W/E split).
const PORTSUDAN_ARTERY: [number, number][] = [   // Khartoum→Shendi→Atbara→Haiya→Port Sudan
  [32.53, 15.58], [33.44, 16.69], [33.99, 17.70], [35.0, 18.0], [36.32, 18.33], [37.22, 19.62],
]
const WESTERN_KORDOFAN: [number, number][] = [   // Khartoum→Wad Madani→Sennar→Kosti→El Obeid
  [32.53, 15.58], [33.52, 14.40], [33.62, 13.55], [32.66, 13.17], [30.22, 13.18],
]
const NORTHERN_NILE: [number, number][] = [      // Khartoum→Atbara→Abu Hamed→Wadi Halfa
  [32.53, 15.58], [33.44, 16.69], [33.99, 17.70], [33.32, 19.53], [32.2, 20.7], [31.35, 21.80],
]
const EASTERN_KASSALA: [number, number][] = [    // Sennar→Gedaref→Kassala→Haiya→Port Sudan
  [33.62, 13.55], [35.38, 14.04], [36.40, 15.45], [36.4, 16.8], [36.32, 18.33], [37.22, 19.62],
]

const near = (lat: number, lon: number, line: [number, number][], m = 18000) => pointToPolylineDist(lat, lon, line) <= m

/** Classify one rail row into trains/day. null = leave at engine default. */
function classify(lat: number, lon: number, railType: number, usage: number): { pax: number; frt: number } | null {
  // Sudan has no tram/light_rail/funicular. Only rail (0) + narrow_gauge (3) are
  // Sudan Railways — handle both; everything else stays at the engine default.
  if (railType !== 0 && railType !== 3) return null
  if (usage === 2) return { pax: 0, frt: 1 }        // industrial spur (sugar/port)

  // Highest-freight corridor wins on shared segments (e.g. Khartoum–Atbara is
  // shared by the Port Sudan artery and the northern Nile line → artery's 3 frt).
  if (near(lat, lon, PORTSUDAN_ARTERY)) return { pax: 1, frt: 3 }
  if (near(lat, lon, WESTERN_KORDOFAN)) return { pax: 1, frt: 2 }
  if (near(lat, lon, NORTHERN_NILE))    return { pax: 1, frt: 1 }
  if (near(lat, lon, EASTERN_KASSALA))  return { pax: 1, frt: 1 }

  // pax floored at 1, not 0: normalize_rail reads pax=0 as "use def_pax" (80/day),
  // which would un-silence these degraded off-corridor branches.
  return { pax: 1, frt: 1 }                          // off-corridor / branch — degraded
}

async function main() {
  console.log(`=== SD Railway Enrichment — Sudan Railways corridor-tier defaults (${YEAR}) ===\n`)

  const inSD = makeCountryGate('SD')
  const hexDirs = iterateCountryHexes(H3R4_DIR, SD_HEX_BBOX, 'railways.arrow')
  console.log(`  SD-bbox hexes with railways.arrow: ${hexDirs.length}\n`)

  let totalRows = 0, enriched = 0, hexesUpdated = 0, preserved = 0, skippedService = 0
  const tierCount: Record<string, number> = {}
  let sumPax = 0, sumFrt = 0
  const startTime = Date.now()

  const tierKey = (pax: number, frt: number): string =>
    pax === 1 && frt === 3 ? 'portsudan-artery' : pax === 1 && frt === 2 ? 'western-kordofan'
    : pax === 1 && frt === 1 ? 'northern-nile' : 'eastern/branch'

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRailTrains(
      resolve(H3R4_DIR, hex, 'railways.arrow'),
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        if (!inBbox(row.midLat, row.midLon, SD_HEX_BBOX)) return null
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
      inSD, // #31.7 central country gate — see writeRailTrains
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
  console.log(`\n  Mean pax/day: ${(sumPax / Math.max(enriched, 1)).toFixed(1)}, mean frt/day: ${(sumFrt / Math.max(enriched, 1)).toFixed(1)}`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
