/**
 * Enrich NG railways.arrow with NRC corridor/tier defaults.
 *
 * The Nigerian Railway Corporation (NRC) publishes no GIS and no GTFS feed, and
 * the new Chinese-built standard-gauge lines have no open schedules. So — like
 * the road layer — we apply tier defaults anchored to each corridor's published
 * opening/operating context, not a feed. Counts are trains/day in BOTH
 * directions (the engine's convention); freight is the acoustically dominant
 * case on the ore/SGR lines, passenger frequency on the city metros.
 *
 * Nigeria's network is three things at once:
 *   1. New standard-gauge (rail_type 0) BRI corridors — sparse but operating:
 *        Lagos–Ibadan SGR (157 km, 2021)   pax 16 / frt 20
 *        Abuja–Kaduna SGR (187 km, 2014)    pax  8 / frt  6  (often suspended)
 *        Itakpe–Warri iron-ore line (2020)  pax  2 / frt 20  (Ajaokuta ore)
 *   2. Colonial-era Cape-gauge (rail_type 3, narrow_gauge) — mostly defunct:
 *        default                            pax  1 / frt  4
 *   3. New urban light rail (rail_type 2):
 *        Lagos Blue + Red Line (2023/2024)  pax 250 / frt 0
 *        Abuja Rail Mass Transit (2018)     pax  60 / frt 0
 *
 * Heavy-rail (rail_type 0) segments off the three known SGR corridors fall back
 * to the colonial default (1/4): nearly all such track in Nigeria is the legacy
 * narrow gauge, sometimes tagged `rail`. Light-rail count is picked by which
 * metro's bbox the segment sits in (Lagos far busier than Abuja).
 *
 * Gated to Nigerian soil by makeCountryGate('NG') so the bbox cannot stamp NG
 * counts on Benin/Niger/Chad/Cameroon track. Sources + URLs in
 * data/enrichment/2026/ng/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-ng.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_NG_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRailTrains } from './lib/railways-arrow.js'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_NG_NATIONAL_RAILWAY


const NG_HEX_BBOX: [number, number, number, number] = [4.0, 2.7, 13.9, 14.7]

// ── SGR / ore corridor polylines [lon, lat] — matched by min distance to line ──
const LAGOS_IBADAN_SGR: [number, number][] = [
  [3.39, 6.47], [3.36, 6.70], [3.34, 7.15], [3.62, 7.30], [3.90, 7.40], [3.90, 7.45],
] // Apapa/Ebute-Metta → Agege → Abeokuta → Ibadan (Moniya)
const ABUJA_KADUNA_SGR: [number, number][] = [
  [7.42, 9.00], [7.30, 9.40], [7.40, 10.00], [7.45, 10.52],
] // Idu (Abuja) → Kubwa → Rigasa (Kaduna)
const ITAKPE_WARRI_ORE: [number, number][] = [
  [6.30, 7.60], [6.66, 7.55], [6.50, 7.00], [6.19, 6.25], [5.75, 5.52],
] // Itakpe → Ajaokuta → Agbor → Warri

function nearLine(lat: number, lon: number, line: [number, number][], thresholdM: number): boolean {
  return pointToPolylineDist(lat, lon, line) <= thresholdM
}

// ── Urban light-rail nodes [minLat,minLon,maxLat,maxLon] ──
const LAGOS_METRO: [number, number, number, number] = [6.40, 3.28, 6.70, 3.55] // Blue + Red Line
const ABUJA_METRO: [number, number, number, number] = [8.85, 7.20, 9.10, 7.55] // Abuja Rail Mass Transit

// Central Lagos: the Blue Line (coastal Marina–Mile 2) and Red Line (Oyingbo–
// Agbado, overlaid on the Lagos–Ibadan SGR alignment) are tagged `railway=rail`
// (type 0) and largely unnamed in OSM, so they cannot be told apart from the SGR
// mainline by tag alone. Inside this box, type-0 passenger rail is the frequent
// metro overlay (Blue+Red) plus Apapa container freight — not a 16-train SGR run
// or a defunct line. Beyond the box the SGR reverts to its intercity 16/20.
const LAGOS_URBAN_RAIL: [number, number, number, number] = [6.43, 3.28, 6.72, 3.43]

/** Classify one rail row into trains/day (both directions). null = engine default. */
function classify(lat: number, lon: number, railType: number, usage: number): { pax: number; frt: number } | null {
  // Urban light rail / tram — count depends on which metro (Abuja ARMT).
  if (railType === 1 || railType === 2) {
    if (inBbox(lat, lon, LAGOS_METRO)) return { pax: 250, frt: 0 }
    if (inBbox(lat, lon, ABUJA_METRO)) return { pax: 60, frt: 0 }
    return { pax: 60, frt: 0 } // other urban light rail → Abuja-scale default
  }
  if (usage === 2) return { pax: 0, frt: 6 } // industrial spur

  // Heavy rail (rail_type 0) and colonial narrow gauge (3): Lagos metro overlay
  // first, then the new SGR/ore corridors, else the mostly-defunct legacy net.
  if (railType === 0) {
    if (inBbox(lat, lon, LAGOS_URBAN_RAIL)) return { pax: 250, frt: 20 } // Blue+Red metro + Apapa freight
    if (nearLine(lat, lon, LAGOS_IBADAN_SGR, 6000)) return { pax: 16, frt: 20 }
    if (nearLine(lat, lon, ABUJA_KADUNA_SGR, 6000)) return { pax: 8, frt: 6 }
    if (nearLine(lat, lon, ITAKPE_WARRI_ORE, 8000)) return { pax: 2, frt: 20 }
  }
  if (railType === 0 || railType === 3) return { pax: 1, frt: 4 } // colonial narrow gauge
  return null // funicular etc. → engine default
}

async function main() {
  console.log(`=== NG Railway Enrichment — NRC corridor/tier defaults (${YEAR}) ===\n`)

  const inNG = makeCountryGate('NG')
  const hexDirs = iterateCountryHexes(H3R4_DIR, NG_HEX_BBOX, 'railways.arrow')
  console.log(`  NG-bbox hexes with railways.arrow: ${hexDirs.length}\n`)

  let totalRows = 0, enriched = 0, hexesUpdated = 0, preserved = 0, skippedService = 0
  const tierCount: Record<string, number> = {}
  let sumPax = 0, sumFrt = 0
  const startTime = Date.now()

  const tierKey = (pax: number, frt: number): string =>
    pax === 250 ? 'Lagos-metro' : pax === 60 ? 'Abuja-metro' : pax === 16 ? 'Lagos-Ibadan-SGR'
    : pax === 8 ? 'Abuja-Kaduna-SGR' : frt === 20 && pax === 2 ? 'Itakpe-Warri-ore'
    : pax === 0 ? 'industrial-spur' : 'colonial-narrow-gauge'

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRailTrains(
      resolve(H3R4_DIR, hex, 'railways.arrow'),
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        if (!inBbox(row.midLat, row.midLon, NG_HEX_BBOX)) return null
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
      inNG, // #31.7 central country gate — see writeRailTrains
    )
    totalRows += r.rows
    skippedService += r.skippedService
    if (r.updated) hexesUpdated++
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
