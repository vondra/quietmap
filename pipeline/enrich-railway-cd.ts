/**
 * Enrich CD railways.arrow with SNCC / CFMK corridor defaults.
 *
 * The DRC's railways are Belgian colonial-era (built 1890s–1950s) and severely
 * deteriorated. SNCC (Société Nationale des Chemins de fer du Congo) runs the
 * ~5,000 km Voie Nationale out of Katanga; CFMK runs the 366 km Matadi↔Kinshasa
 * line (one of Africa's first railways, 1898). Three gauge systems coexist
 * (1067 mm Cape, 1000 mm metre, 600 mm) and there is NO metro and NO tram. SNCC
 * publishes no GIS and no GTFS, so — like NG / EG — we apply corridor/tier
 * defaults from published operating context, not a feed. Counts are trains/day
 * in BOTH directions (engine convention).
 *
 * Traffic is so sparse that this enrichment mostly corrects the engine's generic
 * rail default DOWN toward reality (a Congolese line carries a few trains/week,
 * not the tens/day a default assumes). docs/about/africa/cd.md:
 *
 *   CFMK Matadi↔Kinshasa (366 km, Cape gauge)      pax 2 / frt 4
 *   SNCC Copperbelt Lubumbashi↔Kamina (+Kolwezi)   pax 1 / frt 3
 *   Dormant: Kamina↔Ilebo + eastern branches       pax 0 / frt 1
 *
 * All DRC rail is heavy/narrow gauge (rail_type 0 or 3); there is no light rail,
 * so railType 1/2 is left at the engine default. Mining sidings in Katanga
 * (usage=industrial) get a freight-only spur count. Anything not on a named
 * corridor falls to the dormant 0/1 (covers the abandoned Kamina↔Ilebo section,
 * the metre-gauge eastern/Kivu lines, and the 600 mm Vicicongo network).
 *
 * Gated to Congolese soil by makeCountryGate('CD') so the bbox cannot stamp CD
 * counts on CG / CF / SS / UG / RW / BI / TZ / ZM / AO track. Sources + URLs in
 * data/enrichment/2026/cd/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-cd.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_CD_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRailTrains } from './lib/railways-arrow.js'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_CD_NATIONAL_RAILWAY


const CD_HEX_BBOX: [number, number, number, number] = [-13.5, 12.0, 5.5, 31.5]

// ── Corridor polylines [lon, lat] — matched by min distance to line ──
// CFMK Matadi↔Kinshasa (Cape gauge, ~366 km) — parallels RN1 up from the port.
const CFMK_MATADI_KINSHASA: [number, number][] = [
  [13.45, -5.82], [13.92, -5.70], [14.44, -5.56], [15.10, -5.13], [15.31, -4.40],
]
// SNCC Voie Nationale Copperbelt: Lubumbashi → Likasi → Tenke → Kamina, plus the
// Tenke → Kolwezi mining spur. Cobalt/copper export rail (where it still runs).
const SNCC_COPPERBELT: [number, number][] = [
  [27.48, -11.66], [26.73, -10.98], [26.07, -10.60], [25.40, -9.50], [24.99, -8.74],
]
const SNCC_KOLWEZI_SPUR: [number, number][] = [
  [26.07, -10.60], [25.47, -10.72],
]
// Largely abandoned Kamina → Kananga → Ilebo section (Cape gauge → Kasaï river port).
const SNCC_KAMINA_ILEBO: [number, number][] = [
  [24.99, -8.74], [23.60, -7.00], [22.42, -5.90], [20.58, -4.33],
]

function nearLine(lat: number, lon: number, line: [number, number][], thresholdM: number): boolean {
  return pointToPolylineDist(lat, lon, line) <= thresholdM
}

/** Classify one rail row into trains/day (both directions). null = engine default. */
function classify(lat: number, lon: number, railType: number, usage: number): { pax: number; frt: number } | null {
  // No light rail / tram anywhere in the DRC — leave at engine default.
  if (railType === 1 || railType === 2) return null
  // Heavy rail (0) and colonial narrow/metre gauge (3).
  if (railType === 0 || railType === 3) {
    if (nearLine(lat, lon, CFMK_MATADI_KINSHASA, 8000)) return { pax: 2, frt: 4 }
    if (nearLine(lat, lon, SNCC_COPPERBELT, 8000) ||
        nearLine(lat, lon, SNCC_KOLWEZI_SPUR, 8000)) return { pax: 1, frt: 3 }
    // pax floored at 1, not 0: normalize_rail reads pax=0 as "use def_pax"
    // (80/day on a mainline), so 0 would un-silence these freight-only/dormant
    // lines. usage===2 (industrial siding) is the one exception: its def_pax IS 0.
    if (nearLine(lat, lon, SNCC_KAMINA_ILEBO, 8000)) return { pax: 1, frt: 1 }
    if (usage === 2) return { pax: 0, frt: 2 } // Katanga mining siding
    return { pax: 1, frt: 1 } // dormant eastern/Kivu/Vicicongo branches
  }
  return null // funicular etc. → engine default
}

async function main() {
  console.log(`=== CD Railway Enrichment — SNCC/CFMK corridor defaults (${YEAR}) ===\n`)

  const inCD = makeCountryGate('CD')
  const hexDirs = iterateCountryHexes(H3R4_DIR, CD_HEX_BBOX, 'railways.arrow')
  console.log(`  CD-bbox hexes with railways.arrow: ${hexDirs.length}\n`)

  let totalRows = 0, enriched = 0, hexesUpdated = 0, preserved = 0, skippedService = 0
  const tierCount: Record<string, number> = {}
  let sumPax = 0, sumFrt = 0
  const startTime = Date.now()

  const tierKey = (pax: number, frt: number): string =>
    pax === 2 ? 'CFMK-Matadi-Kinshasa' : pax === 1 ? 'SNCC-Copperbelt'
    : frt === 2 ? 'mining-siding' : 'dormant-eastern'

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRailTrains(
      resolve(H3R4_DIR, hex, 'railways.arrow'),
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        if (!inBbox(row.midLat, row.midLon, CD_HEX_BBOX)) return null
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
      inCD, // #31.7 central country gate — see writeRailTrains
    )
    totalRows += r.rows
    skippedService += r.skippedService
    if (r.updated) hexesUpdated++

    const elapsed = Date.now() - startTime
    if (elapsed > 10_000 && hi % 50 === 0) {
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
