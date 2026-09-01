/**
 * Enrich RU railways.arrow with RZD corridor/tier defaults.
 *
 * No open per-segment rail traffic dataset and no official RZD GTFS feed exist
 * (RZD schedules live only behind consumer booking sites; per-line грузонапряжён-
 * ность is internal). So — like the road layer — we apply Russia-tuned tier
 * defaults, anchored to published RZD capacity/operating figures, not guesses.
 * Counts are trains/day in BOTH directions (the engine's convention).
 *
 * Russia has the world's 3rd-largest network (~85,500 km route, ~53% electrified)
 * and is strongly FREIGHT-dominated (~619 Mt, ~3.3 trillion t-km, 2023; coal ≈ a
 * third of tonnage). Freight is the acoustically dominant case — long, heavy
 * consists at moderate speed — so the freight/day figure is the load-bearing one.
 *
 * ## Tiers (trains/day, both directions)
 *
 * | tier            | pax | frt | applied to |
 * |-----------------|----:|----:|------------|
 * | A  trunk main   |  30 | 110 | Transsib core spine |
 * | A-HS passenger  |  35 |  15 | Moscow–St Petersburg (Sapsan; freight diverted) |
 * | Kuzbass freight |  12 | 120 | Kuzbass coal exits (densest freight on the net) |
 * | Port approach   |   8 |  70 | Ust-Luga / Novorossiysk / Vostochny-Nakhodka |
 * | BAM             |   6 |  50 | Baikal–Amur Mainline (freight-heavy, low pax) |
 * | B  secondary    |  12 |  45 | default for usage=main electrified regional |
 * | C  branch       |   4 |  12 | usage=branch single-track |
 * | S  Moscow node  | 150 |   5 | Moscow suburban (MCD/radials, ~100 pairs/day MCD-2) |
 * | S  SPb node     | 130 |   8 | St Petersburg suburban (~976 trains/day / ~7 radials) |
 * | S  metro minor  |  60 |   8 | other million-plus city suburban nodes |
 * | industrial spur |   0 |   6 | usage=industrial |
 * | tram/light_rail | 200 |   0 | urban tram networks |
 *
 * Derivation (see provenance.md for URLs): Transsib "up to 80 train pairs/day"
 * (~160/day) on the busiest double-track; Eastern Polygon provoznaya capacity
 * 180 Mt/yr (2024) ÷ ~4,500 t gross/train ÷ 365 ≈ ~110 loaded → ~205 total;
 * RZD network-average freight train ~4,045 t gross. Moscow–SPb: 14-15 Sapsan/dir
 * + freight diverted to parallel lines. MCD-2 = 100 train pairs/day. Counts are
 * coarse (±30-40%) but freight/day moves Lden in ~1-2 dB steps, so this is the
 * right granularity. Estimates flagged in provenance.
 *
 * Sources: company.rzd.ru / ria.ru network stats, interfax/neftegaz Eastern-
 * Polygon capacity, ru.wikipedia Sapsan + МЦД, portnews/terminals.ru port
 * capacity, СТН Ц-01-95 freight-density classes. Full list in
 * data/enrichment/2026/ru/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-ru.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_RU_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRailTrains } from './lib/railways-arrow.js'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_RU_NATIONAL_RAILWAY


const RU_HEX_BBOX: [number, number, number, number] = [41.0, 19.0, 82.0, 180.0]

// ── Corridor polylines [lon, lat] — matched by min distance to the whole line ──
// Coarse routes through junction cities; the threshold (per corridor) catches the
// real line without grabbing distant parallels. In Siberia the Transsib is the
// only east-west heavy main, so a wide band there is safe.
const TRANSSIB: [number, number][] = [
  [37.62, 55.75], [39.87, 57.63], [49.66, 58.60], [56.25, 58.01], [60.61, 56.84],
  [65.53, 57.15], [73.37, 54.99], [82.92, 55.03], [92.85, 56.01], [104.28, 52.29],
  [107.58, 51.83], [113.50, 52.03], [119.74, 53.74], [123.95, 54.00], [128.40, 50.92],
  [135.07, 48.48], [131.94, 43.80], [131.89, 43.12],
]
const MOSCOW_SPB: [number, number][] = [
  [37.62, 55.75], [35.91, 56.86], [33.27, 57.88], [30.31, 59.94],
]
const BAM: [number, number][] = [
  [98.00, 55.93], [101.61, 56.15], [105.77, 56.79], [109.32, 55.63],
  [124.72, 55.15], [137.00, 50.55], [140.25, 49.09],
]
const KUZBASS: [number, number][] = [
  [84.95, 55.72], [86.09, 55.35], [87.12, 53.79], [88.07, 53.69],
]

function nearLine(lat: number, lon: number, line: [number, number][], thresholdM: number): boolean {
  return pointToPolylineDist(lat, lon, line) <= thresholdM
}

// ── Port approach + suburban-node bboxes [minLat,minLon,maxLat,maxLon] ──
const PORT_ZONES: [number, number, number, number][] = [
  [59.3, 27.7, 60.0, 28.9],   // Ust-Luga (Mga–Veimarn–Luzhskaya)
  [44.4, 37.4, 45.0, 38.1],   // Novorossiysk
  [42.5, 132.4, 43.2, 133.3], // Vostochny / Nakhodka
]
function inPort(lat: number, lon: number): boolean {
  for (const b of PORT_ZONES) if (inBbox(lat, lon, b)) return true
  return false
}

interface SubNode { lat: number; lon: number; half: number; pax: number; frt: number }
const SUBURBAN: SubNode[] = [
  { lat: 55.75, lon: 37.62, half: 0.55, pax: 150, frt: 5 }, // Moscow
  { lat: 59.94, lon: 30.31, half: 0.45, pax: 130, frt: 8 }, // St Petersburg
  // other million-plus cities — scaled-down suburban frequency
  ...[
    [55.03, 82.92], [56.84, 60.61], [55.79, 49.12], [56.33, 44.00], [56.01, 92.85],
    [55.16, 61.40], [53.20, 50.15], [54.74, 55.97], [47.24, 39.71], [45.04, 38.98],
    [54.99, 73.37], [51.66, 39.20], [58.01, 56.25], [48.71, 44.51],
  ].map(([lat, lon]) => ({ lat, lon, half: 0.20, pax: 60, frt: 8 })),
]
function suburban(lat: number, lon: number): { pax: number; frt: number } | null {
  for (const s of SUBURBAN) {
    if (lat >= s.lat - s.half && lat <= s.lat + s.half &&
        lon >= s.lon - s.half && lon <= s.lon + s.half) return { pax: s.pax, frt: s.frt }
  }
  return null
}

/** Classify one heavy-rail/tram row into trains/day. null = leave at engine default. */
function classify(lat: number, lon: number, railType: number, usage: number): { pax: number; frt: number } | null {
  if (railType === 1 || railType === 2) return { pax: 200, frt: 0 } // tram / light_rail
  if (railType !== 0) return null                                    // narrow_gauge / funicular → default
  if (usage === 2) return { pax: 0, frt: 6 }                         // industrial spur

  // Freight corridors first — a Transsib segment inside a city is still the freight main.
  if (nearLine(lat, lon, MOSCOW_SPB, 8000)) return { pax: 35, frt: 15 }   // A-HS
  if (nearLine(lat, lon, KUZBASS, 15000)) return { pax: 12, frt: 120 }    // densest freight
  if (nearLine(lat, lon, TRANSSIB, 12000)) return { pax: 30, frt: 110 }   // A
  if (nearLine(lat, lon, BAM, 15000)) return { pax: 6, frt: 50 }          // BAM
  if (inPort(lat, lon)) return { pax: 8, frt: 70 }                        // port approaches

  const sub = suburban(lat, lon)
  if (sub) return sub                                                     // Tier S

  if (usage === 1) return { pax: 4, frt: 12 }                            // branch → Tier C
  return { pax: 12, frt: 45 }                                            // main → Tier B
}

async function main() {
  console.log(`=== RU Railway Enrichment — RZD corridor/tier defaults (${YEAR}) ===\n`)

  const inRU = makeCountryGate('RU')
  const hexDirs = iterateCountryHexes(H3R4_DIR, RU_HEX_BBOX, 'railways.arrow')
  console.log(`  RU-bbox hexes with railways.arrow: ${hexDirs.length}\n`)

  let totalRows = 0, enriched = 0, hexesUpdated = 0, preserved = 0, skippedService = 0
  const tierCount: Record<string, number> = {}
  let sumPax = 0, sumFrt = 0
  const startTime = Date.now()

  const tierKey = (pax: number, frt: number): string =>
    frt === 120 ? 'Kuzbass' : frt === 110 ? 'Transsib-A' : frt === 70 ? 'Port' : frt === 50 ? 'BAM'
    : pax === 35 ? 'Moscow-SPb-AHS' : pax === 200 ? 'tram' : pax === 150 || pax === 130 ? 'suburban-major'
    : pax === 60 ? 'suburban-minor' : frt === 6 && pax === 0 ? 'industrial'
    : pax === 4 ? 'branch-C' : 'main-B'

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRailTrains(
      resolve(H3R4_DIR, hex, 'railways.arrow'),
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        if (!inBbox(row.midLat, row.midLon, RU_HEX_BBOX)) return null
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
      inRU, // #31.7 central country gate — see writeRailTrains
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
