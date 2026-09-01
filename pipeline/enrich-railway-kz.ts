/**
 * Enrich KZ railways.arrow with KTZ corridor/tier defaults.
 *
 * No open per-segment rail traffic dataset and no KTZ (Kazakhstan Temir Zholy)
 * GTFS feed exist — KTZ publishes neither a public schedule feed nor per-line
 * грузонапряжённость (freight density), and Central-Asian operators stay off the
 * open-GTFS registries. So — like the road layer — we apply Kazakhstan-tuned tier
 * defaults anchored to published KTZ network figures, not guesses. Counts are
 * trains/day in BOTH directions (the engine's convention).
 *
 * KTZ runs ~16,600 km of broad gauge (1,520 mm) — one of the world's largest
 * networks by length — Soviet-built and strongly FREIGHT-dominated (oil, grain,
 * coal, uranium, plus a fast-growing China→Europe BRI transit). Freight is the
 * acoustically dominant case (long, heavy consists at moderate speed), so the
 * freight/day figure is the load-bearing one. The Ekibastuz basin alone moves
 * ~100 Mtpa of coal — one of the heaviest single freight corridors on Earth.
 *
 * ## Tiers (trains/day, both directions)
 *
 * | tier               | pax | frt | applied to |
 * |--------------------|----:|----:|------------|
 * | Almaty Metro       |  80 |   0 | subway (OSM-tagged tram) E-W through Almaty centre |
 * | Astana LRT         |  50 |   0 | light_rail, opened 2024, in Astana |
 * | Almaty↔Astana main |   8 |  20 | ~1,300 km N-S passenger backbone |
 * | Ekibastuz coal     |   2 |  30 | Ekibastuz↔Pavlodar + coal exits (densest freight) |
 * | KTZ network main   |   2 |  10 | usage=main incl. Trans-Kazakh / China transit trunk |
 * | branch             |   1 |   4 | usage=branch single-track |
 * | industrial spur    |   0 |   6 | usage=industrial (coal/ore spurs — very common in KZ) |
 * | urban tram (other) |  60 |   0 | tram/light_rail in any other KZ city |
 *
 * Derivation (see provenance-railway.md): KTZ ~16,600 km route; Ekibastuz basin
 * ~100 Mtpa coal ÷ ~4,000 t gross/train ÷ 365 ≈ ~68 loaded → ~30/day per primary
 * exit line once split across the parallel Ekibastuz–Pavlodar / Ekibastuz–Astana
 * routes (rounded; ±30-40 %). Almaty↔Astana = the published main passenger relation
 * (a handful of named trains each way) over a freight-shared double track. Almaty
 * Metro (2011, 1 line, 11 km) and Astana LRT (2024, 22.4 km) are OSM-tagged tram /
 * light_rail respectively — there is no distinct subway rail_type in the engine, so
 * they are matched by rail_type + location. Counts are coarse but freight/day moves
 * Lden in ~1-2 dB steps, so this is the right granularity.
 *
 * The KZ_HEX_BBOX [40,46,56,88] overlaps a wide band of southern Russia (Magnitogorsk,
 * Chelyabinsk, Omsk, Novosibirsk … all carry trams + heavy rail), so the CGAZ KZ
 * polygon gate is REQUIRED — without it ~21k Russian tram rows would be stamped as KZ.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-kz.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_KZ_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRailTrains } from './lib/railways-arrow.js'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_KZ_NATIONAL_RAILWAY


const KZ_HEX_BBOX: [number, number, number, number] = [40.0, 46.0, 56.0, 88.0]

// ── Corridor polylines [lon, lat] — matched by min distance to the whole line ──
// Coarse routes through junction cities; the per-corridor threshold catches the
// real line without grabbing distant parallels. KTZ mains are sparse across the
// steppe, so a wide band is safe.

// Ekibastuz basin coal exits: Ekibastuz ↔ Pavlodar (NE to the Irtysh, towards Russia)
// and Ekibastuz ↔ Astana (the main westward coal haul). Densest freight on the net.
const EKIBASTUZ_COAL: [number, number][] = [
  [76.97, 52.29], // Pavlodar
  [75.37, 51.67], // Ekibastuz
  [73.10, 49.80], // Karaganda direction (coal to central KZ / Astana via the main line)
]
// Almaty ↔ Astana passenger backbone (~1,300 km N-S spine via Balkhash + Karaganda).
const ALMATY_ASTANA: [number, number][] = [
  [76.92, 43.24], // Almaty
  [74.98, 46.84], // Balkhash
  [73.10, 49.80], // Karaganda
  [71.43, 51.13], // Astana
]

function nearLine(lat: number, lon: number, line: [number, number][], thresholdM: number): boolean {
  return pointToPolylineDist(lat, lon, line) <= thresholdM
}

// ── Metro / LRT zones [minLat,minLon,maxLat,maxLon] ──
// Almaty Metro is OSM-tagged tram (rail_type 1) and runs E-W through the centre;
// Astana LRT is light_rail (rail_type 2). Match each by rail_type + its city box so
// a real surface tram elsewhere doesn't inherit the metro frequency.
const ALMATY_METRO: [number, number, number, number] = [43.18, 76.80, 43.32, 77.02]
const ASTANA_LRT: [number, number, number, number] = [50.95, 71.25, 51.25, 71.70]

/** Classify one heavy-rail/tram row into trains/day. null = leave at engine default. */
function classify(lat: number, lon: number, railType: number, usage: number): { pax: number; frt: number } | null {
  // Urban rail first — a subway segment inside a city is not a freight main.
  if (railType === 1 || railType === 2) {
    if (railType === 1 && inBbox(lat, lon, ALMATY_METRO)) return { pax: 80, frt: 0 }  // Almaty Metro (2011)
    if (railType === 2 && inBbox(lat, lon, ASTANA_LRT)) return { pax: 50, frt: 0 }    // Astana LRT (2024)
    return { pax: 60, frt: 0 }                                                        // other KZ urban tram/LRT
  }
  if (railType !== 0) return null                                                     // narrow_gauge / funicular → default
  if (usage === 2) return { pax: 0, frt: 6 }                                          // industrial / coal-ore spur

  // Freight corridors — Ekibastuz first (densest freight), then the passenger backbone.
  if (nearLine(lat, lon, EKIBASTUZ_COAL, 20000)) return { pax: 2, frt: 30 }           // Ekibastuz coal corridor
  if (nearLine(lat, lon, ALMATY_ASTANA, 18000)) return { pax: 8, frt: 20 }            // Almaty↔Astana backbone

  if (usage === 1) return { pax: 1, frt: 4 }                                          // branch single-track
  return { pax: 2, frt: 10 }                                                          // KTZ network main (incl. China transit)
}

async function main() {
  console.log(`=== KZ Railway Enrichment — KTZ corridor/tier defaults (${YEAR}) ===\n`)

  const inKZ = makeCountryGate('KZ')
  const hexDirs = iterateCountryHexes(H3R4_DIR, KZ_HEX_BBOX, 'railways.arrow')
  console.log(`  KZ-bbox hexes with railways.arrow: ${hexDirs.length}\n`)

  let totalRows = 0, enriched = 0, hexesUpdated = 0, preserved = 0, skippedService = 0
  const tierCount: Record<string, number> = {}
  let sumPax = 0, sumFrt = 0
  const startTime = Date.now()

  const tierKey = (pax: number, frt: number): string =>
    pax === 80 ? 'Almaty-Metro' : pax === 50 ? 'Astana-LRT' : pax === 60 ? 'tram-other'
    : frt === 30 ? 'Ekibastuz-coal' : pax === 8 && frt === 20 ? 'Almaty-Astana'
    : frt === 6 && pax === 0 ? 'industrial' : pax === 1 ? 'branch'
    : 'network-main'

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRailTrains(
      resolve(H3R4_DIR, hex, 'railways.arrow'),
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        if (!inBbox(row.midLat, row.midLon, KZ_HEX_BBOX)) return null
        const t = classify(row.midLat, row.midLon, row.railType, row.usage)
        if (!t) return null
        return { pax: t.pax, frt: t.frt, sourceId: MY_SOURCE_ID }
      },
      (_row, _i, applied) => {
        enriched++
        tierCount[tierKey(applied.pax, applied.frt)] = (tierCount[tierKey(applied.pax, applied.frt)] || 0) + 1
        sumPax += applied.pax
        sumFrt += applied.frt
      },
      undefined,
      inKZ, // #31.7 central country gate — see writeRailTrains
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
