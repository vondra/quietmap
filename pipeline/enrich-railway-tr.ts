/**
 * Enrich TR railways.arrow with TCDD/YHT + Istanbul-rail corridor-tier defaults.
 *
 * TCDD publishes timetables as PDF only — no GTFS, no open GIS (re-confirmed
 * 2026-04). So — like CN/IN/RU/EG — we apply family + corridor-tier defaults over
 * the OSM rail geometry, gated to Turkish soil by the CGAZ country polygon (Greece/
 * Bulgaria/Georgia/Armenia/Iran/Iraq/Syria all overlap the bbox). Counts are
 * trains/day in BOTH directions (engine convention).
 *
 * TCDD runs ~12,500 km of standard gauge — one of the world's larger networks —
 * and Turkey has built out high-speed rail (YHT) and urban metros aggressively
 * since the 2000s.
 *
 * ## Family routing (rail_type → engine inputs.rs codes)
 *   tram (1)        — Istanbul T1, İzmir/Antalya/Konya/Gaziantep/Kayseri/Samsun trams
 *   light_rail (2)  — Istanbul Metro (M1-M11), Ankara Metro/Ankaray, İzmir Metro,
 *                     BursaRay, Adana Metro
 *   funicular (4)   — Istanbul F1/F2 (short shuttles)
 *   rail (0)        — TCDD heavy rail; classified by corridor below
 *
 * ## Corridor tiers (pax / frt trains/day, both directions)
 *
 * | tier                              | pax | frt | applied to |
 * |-----------------------------------|----:|----:|------------|
 * | Marmaray (Istanbul commuter)      | 400 |   0 | Halkalı↔Bosphorus tunnel↔Gebze |
 * | IZBAN (İzmir commuter)            | 150 |   4 | Aliağa↔İzmir↔Selçuk |
 * | YHT high-speed (passenger-only)   |  40 |   0 | Ankara↔Eskişehir↔Istanbul / ↔Konya / ↔Sivas |
 * | ore/coal heavy freight            |   6 |  20 | Divriği↔İskenderun (İsdemir), Zonguldak↔Karabük (Kardemir) |
 * | TCDD main intercity               |  20 |  12 | Ankara hub ↔ İzmir/Adana/Sivas/Kars/Diyarbakır + Thrace |
 * | branch (usage=branch)             |   1 |   3 | secondary lines |
 * | industrial spur (usage=industrial)|   0 |   5 | sidings |
 * | main fallback (other operational) |   8 |   6 | usage=main not on a named corridor |
 *
 * Urban metro/tram counts take an Istanbul boost (~1.5 M metro pax/day, T1 tram is
 * one of Europe's busiest). Counts are coarse (±30-40%) but freight/day moves Lden
 * in ~1-2 dB steps — the right granularity given zero published per-line data.
 * Full derivation in data/enrichment/2026/tr/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-tr.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_TR_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRailTrains } from './lib/railways-arrow.js'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_TR_NATIONAL_RAILWAY


const TR_HEX_BBOX: [number, number, number, number] = [35.8, 25.6, 42.2, 44.8]
// Greater Istanbul (both banks) — urban-rail counts take a boost inside this box.
const ISTANBUL_BOX: [number, number, number, number] = [40.80, 28.45, 41.35, 29.45]

// ── Corridor polylines [lon,lat] — matched by min distance to the whole line ──
const MARMARAY: [number, number][] = [
  [28.79, 41.00], [28.85, 41.00], [28.95, 40.99], [28.98, 41.01], [29.01, 41.02],
  [29.06, 41.00], [29.16, 40.96], [29.31, 40.86], [29.43, 40.79],
]
const IZBAN: [number, number][] = [
  [26.97, 38.80], [27.03, 38.65], [27.14, 38.46], [27.15, 38.42], [27.25, 38.30], [27.37, 37.95],
]
// YHT high-speed (dedicated alignment, passenger-only, no freight).
const YHT: [number, number][][] = [
  [[32.86, 39.93], [31.80, 39.80], [30.52, 39.78], [29.98, 40.05], [29.92, 40.77], [29.55, 40.85], [29.25, 40.88]], // Ankara↔Eskişehir↔Pendik
  [[32.86, 39.93], [32.15, 39.58], [32.30, 38.70], [32.48, 37.87]], // Ankara↔Konya
  [[32.86, 39.93], [33.51, 39.84], [34.80, 39.82], [36.00, 39.78], [37.02, 39.75]], // Ankara↔Sivas
]
// Iron-ore / coal heavy-freight lines feeding the steelworks.
const ORE_FREIGHT: [number, number][][] = [
  [[38.12, 39.37], [38.31, 38.36], [37.00, 37.00], [36.17, 36.58]], // Divriği ore ↔ İskenderun (İsdemir)
  [[31.80, 41.45], [32.63, 41.20]], // Zonguldak coal ↔ Karabük (Kardemir)
]
// TCDD conventional intercity (passenger + freight) — Ankara hub spokes + Thrace + east.
const MAIN_TCDD: [number, number][][] = [
  [[32.86, 39.93], [31.40, 39.20], [30.55, 38.76], [29.40, 38.68], [27.43, 38.61], [27.14, 38.42]], // Ankara↔İzmir
  [[32.86, 39.93], [33.51, 39.84], [34.50, 39.10], [35.48, 38.73], [36.30, 39.30], [37.02, 39.75]], // Ankara↔Kayseri↔Sivas
  [[32.48, 37.87], [33.60, 37.90], [34.68, 37.97], [35.10, 37.40], [35.32, 37.00], [34.63, 36.81]], // Konya↔Niğde↔Adana↔Mersin (Toros)
  [[35.32, 37.00], [36.25, 37.07], [37.38, 37.07]], // Adana↔Osmaniye↔Gaziantep (Syria gateway)
  [[30.52, 39.78], [29.98, 39.42], [27.89, 39.65], [27.43, 38.61]], // Eskişehir↔Kütahya↔Balıkesir↔Manisa
  [[28.95, 41.05], [28.00, 41.28], [26.56, 41.68]], // Istanbul↔Çerkezköy↔Edirne (Thrace, Bulgaria gateway)
  [[37.02, 39.75], [39.49, 39.75], [41.27, 39.90], [43.10, 40.60]], // Sivas↔Erzincan↔Erzurum↔Kars
  [[38.31, 38.36], [40.24, 37.91], [41.13, 37.88]], // Malatya↔Diyarbakır↔Batman
  [[29.25, 40.88], [30.52, 39.78], [32.86, 39.93]], // Istanbul↔Eskişehir↔Ankara conventional (parallels YHT)
]

const near = (lat: number, lon: number, line: [number, number][], m = 12000) => pointToPolylineDist(lat, lon, line) <= m
const nearAny = (lat: number, lon: number, lines: [number, number][][], m = 12000) => lines.some(l => near(lat, lon, l, m))

/** Classify one rail/tram row into trains/day. null = leave at engine default. */
function classify(lat: number, lon: number, railType: number, usage: number): { pax: number; frt: number } | null {
  const istanbul = inBbox(lat, lon, ISTANBUL_BOX)
  if (railType === 4) return { pax: 100, frt: 0 }                       // funicular
  if (railType === 1) return { pax: istanbul ? 350 : 250, frt: 0 }     // tram
  if (railType === 2) return { pax: istanbul ? 500 : 400, frt: 0 }     // light_rail / metro
  if (railType !== 0) return null                                       // narrow_gauge → default

  if (usage === 2) return { pax: 0, frt: 5 }                            // industrial spur

  // Heavy rail by corridor — most-specific / busiest first.
  if (near(lat, lon, MARMARAY, 6000)) return { pax: 400, frt: 0 }       // Istanbul commuter
  if (near(lat, lon, IZBAN, 6000)) return { pax: 150, frt: 4 }          // İzmir commuter
  if (nearAny(lat, lon, YHT, 8000)) return { pax: 40, frt: 0 }          // high-speed
  if (nearAny(lat, lon, ORE_FREIGHT)) return { pax: 6, frt: 20 }        // ore/coal freight
  if (nearAny(lat, lon, MAIN_TCDD)) return { pax: 20, frt: 12 }         // main intercity

  if (usage === 1) return { pax: 1, frt: 3 }                            // branch
  return { pax: 8, frt: 6 }                                             // other operational main
}

async function main() {
  console.log(`=== TR Railway Enrichment — TCDD/YHT + Istanbul-rail corridor-tier defaults (${YEAR}) ===\n`)

  const inTR = makeCountryGate('TR')
  const hexDirs = iterateCountryHexes(H3R4_DIR, TR_HEX_BBOX, 'railways.arrow')
  console.log(`  TR-bbox hexes with railways.arrow: ${hexDirs.length}\n`)

  let totalRows = 0, enriched = 0, hexesUpdated = 0, preserved = 0, skippedService = 0
  const tierCount: Record<string, number> = {}
  let sumPax = 0, sumFrt = 0
  const startTime = Date.now()

  const tierKey = (pax: number, frt: number): string =>
    pax >= 500 ? 'metro' : pax === 400 && frt === 0 ? 'marmaray-or-metro'
    : pax === 350 || pax === 250 ? 'tram' : pax === 150 ? 'izban' : pax === 100 ? 'funicular'
    : pax === 40 ? 'yht' : frt === 20 ? 'ore-freight' : pax === 20 ? 'main-intercity'
    : pax === 0 ? 'industrial' : pax === 1 ? 'branch' : 'main-fallback'

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRailTrains(
      resolve(H3R4_DIR, hex, 'railways.arrow'),
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        if (!inBbox(row.midLat, row.midLon, TR_HEX_BBOX)) return null
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
      inTR, // #31.7 central country gate — see writeRailTrains
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
