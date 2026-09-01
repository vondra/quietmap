/**
 * Enrich UZ railways.arrow with UTY corridor/tier defaults.
 *
 * No official UTY (O'zbekiston Temir Yo'llari) GTFS or open per-segment traffic
 * feed exists — timetables are state-controlled and live only behind the booking
 * site. So — like the road layer and the neighbours RU/KZ — we apply Uzbekistan-
 * tuned corridor/tier defaults over OSM geometry, gated by makeCountryGate('UZ').
 * Counts are trains/day in BOTH directions (the engine's convention).
 *
 * UTY runs ~6,950 km of broad gauge (1,520 mm) — Central Asia's most developed
 * network, the only one with both a metro and high-speed rail. Service is light
 * by European standards, so the freight/day figure on the transit corridors is
 * the acoustically load-bearing one (long heavy consists at moderate speed).
 *
 * ## NOT covered — Tashkent Metro
 * The Metro (1977, Central Asia's oldest) is tagged `railway=subway`, which the
 * osm-extract crate drops from railways.arrow (it accepts only rail/tram/
 * light_rail/narrow_gauge/funicular). So the Metro cannot be enriched here; its
 * noise would need a separate subway pass. Documented in provenance as a gap.
 *
 * ## Tiers (trains/day, both directions)
 *
 * | tier                  | pax | frt | applied to                                        |
 * |-----------------------|----:|----:|---------------------------------------------------|
 * | Silk Road spine       |   8 |  14 | Tashkent–Jizzakh–Samarkand–Navoiy–Bukhara (HSR    |
 * |                       |     |     | Afrosiyob pax + conventional + China-bound transit)|
 * | Ferghana corridor     |   5 |  11 | Tashkent–Angren–(Kamchik tunnel)–Pap–Namangan–Andijan |
 * | Karakalpakstan main   |   5 |  11 | Bukhara–Urgench–Nukus–Kungrad (NW corridor)       |
 * | South / Termez        |   4 |  12 | Samarkand–Qarshi–Termez (trans-Afghan freight)    |
 * | other main            |   4 |   8 | usage=main off the named corridors                |
 * | branch                |   2 |   5 | usage=branch single-track                         |
 * | industrial spur       |   0 |   6 | usage=industrial                                  |
 * | tram / light_rail     | 200 |   0 | any rail_type 1/2 (Tashkent tram mostly closed)   |
 *
 * Derivation (see provenance.md): Afrosiyob (Talgo 250 km/h) ~4-6 services/dir/day
 * Tashkent–Samarkand ⇒ ~8-12 pax both ways on the spine; conventional intercity +
 * Belt-and-Road China/Kazakhstan↔Afghanistan transit freight loads the spine and
 * the Termez (Hairatan) gateway. Counts are coarse (±30-40%) but freight/day moves
 * Lden in ~1-2 dB steps, so this is the right granularity. Estimates flagged in
 * provenance.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-uz.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_UZ_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRailTrains } from './lib/railways-arrow.js'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_UZ_NATIONAL_RAILWAY


const UZ_HEX_BBOX: [number, number, number, number] = [37.2, 55.9, 45.6, 73.2]

// ── Corridor polylines [lon, lat] — matched by min distance to the whole line ──
// Coarse routes through junction cities; the per-corridor threshold catches the
// real line without grabbing distant parallels. UTY mains are the only heavy rail
// in their corridors (sparse desert/valley terrain), so a wide band is safe.
const SILK_ROAD_SPINE: [number, number][] = [
  [69.24, 41.31], [67.84, 40.12], [66.96, 39.65], [65.38, 40.10], [64.42, 39.77],
]
const FERGHANA: [number, number][] = [
  [69.24, 41.31], [70.14, 41.02], [71.10, 40.87], [71.67, 40.99], [72.34, 40.78],
]
const KARAKALPAK_NW: [number, number][] = [
  [64.42, 39.77], [62.20, 40.10], [60.63, 41.55], [59.61, 42.46], [58.54, 43.06],
]
const SOUTH_TERMEZ: [number, number][] = [
  [66.96, 39.65], [65.79, 38.86], [67.28, 37.22],
]

function nearLine(lat: number, lon: number, line: [number, number][], thresholdM: number): boolean {
  return pointToPolylineDist(lat, lon, line) <= thresholdM
}

/** Classify one heavy-rail/tram row into trains/day. null = leave at engine default. */
function classify(lat: number, lon: number, railType: number, usage: number): { pax: number; frt: number } | null {
  if (railType === 1 || railType === 2) return { pax: 200, frt: 0 } // tram / light_rail
  if (railType !== 0) return null                                    // narrow_gauge / funicular → default
  if (usage === 2) return { pax: 0, frt: 6 }                         // industrial spur

  // Named corridors first — a spine segment inside a city is still the freight main.
  if (nearLine(lat, lon, SILK_ROAD_SPINE, 12000)) return { pax: 8, frt: 14 } // Afrosiyob + transit
  if (nearLine(lat, lon, FERGHANA, 12000)) return { pax: 5, frt: 11 }        // Kamchik corridor
  if (nearLine(lat, lon, KARAKALPAK_NW, 15000)) return { pax: 5, frt: 11 }   // NW main
  if (nearLine(lat, lon, SOUTH_TERMEZ, 12000)) return { pax: 4, frt: 12 }    // trans-Afghan freight

  if (usage === 1) return { pax: 2, frt: 5 }                         // branch
  return { pax: 4, frt: 8 }                                          // other main
}

async function main() {
  console.log(`=== UZ Railway Enrichment — UTY corridor/tier defaults (${YEAR}) ===\n`)

  const inUZ = makeCountryGate('UZ')
  const hexDirs = iterateCountryHexes(H3R4_DIR, UZ_HEX_BBOX, 'railways.arrow')
  console.log(`  UZ-bbox hexes with railways.arrow: ${hexDirs.length}\n`)

  let totalRows = 0, enriched = 0, hexesUpdated = 0, preserved = 0, skippedService = 0
  const tierCount: Record<string, number> = {}
  let sumPax = 0, sumFrt = 0
  const startTime = Date.now()

  const tierKey = (pax: number, frt: number): string =>
    pax === 8 ? 'Silk-Road-spine' : frt === 12 && pax === 4 ? 'South-Termez'
    : pax === 5 ? 'Ferghana/Karakalpak' : pax === 200 ? 'tram'
    : frt === 6 && pax === 0 ? 'industrial' : pax === 2 ? 'branch' : 'other-main'

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRailTrains(
      resolve(H3R4_DIR, hex, 'railways.arrow'),
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        if (!inBbox(row.midLat, row.midLon, UZ_HEX_BBOX)) return null
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
      inUZ, // #31.7 central country gate — see writeRailTrains
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
