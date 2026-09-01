/**
 * Enrich RU roads.arrow with Russia-tuned CNOSSOS class defaults.
 *
 * No open per-segment AADT dataset exists for Russia. The national open-data
 * portal (data.gov.ru) was taken offline in 2023 and relaunched mid-2025 with a
 * fraction of its datasets and no traffic counts; Rosavtodor publishes only a
 * road *registry* (geometry/classifier, no counts); ROSDORNII measures AADT
 * (GOST 32965-2014 video detectors) but keeps it internal; russianhighways /
 * Avtodor publish only corridor averages in press releases; several `.ru` gov
 * portals geo-block our egress. So — like CN/IN/TH — we apply country-tuned
 * class defaults rather than a per-segment join.
 *
 * The defaults are anchored to real published Russian numbers, not guessed:
 *
 * ## AADT by OSM class (two-way veh/day)
 *
 * | class            | rural / intercity | metro (×mult) | anchor |
 * |------------------|------------------:|--------------:|--------|
 * | 0 motorway       | 25,000            | up to ~90,000 | M-4 Don >40k, M-7 Volga >25k, M-10 20-60k; MKAD/KAD 90-200k |
 * | 1 trunk          | 14,000            | ~45,000       | interpolated below motorway |
 * | 2 primary        |  7,000            | ~22,000       | Moscow arterials (Leningradsky/Kutuzovsky) |
 * | 3 secondary      |  3,000            | ~10,000       | estimate (~3× step-down) |
 * | 4 tertiary       |  1,200            |  ~4,000       | estimate |
 * | 5 residential    |    300            |    ~900       | estimate |
 *
 * Links (10/11/12) get ~half the parent-class flow (ramp traffic).
 *
 * ## City multipliers (vs rural baseline)
 *   Moscow ×3.5, St Petersburg ×3.0 (tier 1) — lands MKAD/KAD-scale arterials
 *   near the measured 90k-200k; 14 other millionniki ×2.0; sub-million regional
 *   capitals ×1.5. No published per-city scalar exists; calibrated by eye to the
 *   measured ring-road counts.
 *
 * ## Vehicle split — heavy share is the single most acoustically important input
 *   Trucks concentrate on the high-order network. Avtodor's 2024 toll data:
 *   trucks+buses = 30% of the stream overall (CKAD 43%, M-4 Don 33%). The
 *   national registered fleet (AUTOSTAT, 1 Jan 2023, 55.86 M) is 81% cars / 8%
 *   LCV / 7% HGV / 4% moto / 1% bus, but in-stream heavy share on trunks is far
 *   higher because trucks drive most of their km there.
 *     motorway/trunk (+links): light 0.62 / medium 0.10 / heavy 0.27 / moto 0.01
 *     primary/secondary        : light 0.77 / medium 0.10 / heavy 0.12 / moto 0.01
 *     tertiary/residential     : light 0.84 / medium 0.10 / heavy 0.05 / moto 0.01
 *
 * Studded winter tyres (шипованная резина) cover the majority of cars for ~5-7
 * months/yr across central+northern Russia (Nordic-like rolling-noise uplift) —
 * documented in provenance as a future `surface_correction` sidecar; the heatmap
 * engine has no studded-tyre input today, so we don't write a dead column.
 *
 * Sources: online47/Avtodor 2024 toll stats, AUTOSTAT/Rosstat fleet, RBC/dorinfo
 * corridor counts, statdata.ru 2023 city populations. Full list + URLs in
 * data/enrichment/2026/ru/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-ru.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_RU_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { inBbox } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCoastalCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_RU_NATIONAL_ROADS


// RU coarse bbox [minLat,minLon,maxLat,maxLon] — fed to iterateCountryHexes (skips
// the rest of the planet) and re-checked per midpoint. Spans Kaliningrad (~19.6°E)
// to Chukotka (~180°E); the tiny sliver east of the antimeridian (Wrangel, far
// Chukotka tip) is dropped — negligible roads. The makeCoastalCountryGate('RU') polygon
// is the real filter; this box is only a cheap pre-scan over neighbours.
// RU_BBOX="minLat,minLon,maxLat,maxLon" narrows the scan to a sub-region (e.g.
// re-running only the Far-East tail after a fix); already-done hexes outside the
// window are left untouched. Defaults to all of Russia.
const RU_HEX_BBOX: [number, number, number, number] = process.env.RU_BBOX
  ? (process.env.RU_BBOX.split(',').map(Number) as [number, number, number, number])
  : [41.0, 19.0, 82.0, 180.0]

// ── City tiers (AADT multiplier vs rural baseline) ──
// half = bbox half-extent in degrees around the centroid. Moscow/SPb wider to
// cover the ring roads + inner oblast; millionniki/regional ~city extent.
interface City { name: string; lat: number; lon: number; mult: number; half: number }

const CITIES: City[] = [
  // Tier 1
  { name: 'Moscow',          lat: 55.75, lon: 37.62, mult: 3.5, half: 0.40 },
  { name: 'Saint Petersburg', lat: 59.94, lon: 30.31, mult: 3.0, half: 0.32 },
  // Tier 2 — millionniki (×2.0)
  { name: 'Novosibirsk',     lat: 55.03, lon: 82.92, mult: 2.0, half: 0.22 },
  { name: 'Yekaterinburg',   lat: 56.84, lon: 60.61, mult: 2.0, half: 0.22 },
  { name: 'Kazan',           lat: 55.79, lon: 49.12, mult: 2.0, half: 0.22 },
  { name: 'Nizhny Novgorod', lat: 56.33, lon: 44.00, mult: 2.0, half: 0.22 },
  { name: 'Krasnoyarsk',     lat: 56.01, lon: 92.85, mult: 2.0, half: 0.22 },
  { name: 'Chelyabinsk',     lat: 55.16, lon: 61.40, mult: 2.0, half: 0.22 },
  { name: 'Samara',          lat: 53.20, lon: 50.15, mult: 2.0, half: 0.22 },
  { name: 'Ufa',             lat: 54.74, lon: 55.97, mult: 2.0, half: 0.22 },
  { name: 'Rostov-on-Don',   lat: 47.24, lon: 39.71, mult: 2.0, half: 0.22 },
  { name: 'Krasnodar',       lat: 45.04, lon: 38.98, mult: 2.0, half: 0.22 },
  { name: 'Omsk',            lat: 54.99, lon: 73.37, mult: 2.0, half: 0.22 },
  { name: 'Voronezh',        lat: 51.66, lon: 39.20, mult: 2.0, half: 0.22 },
  { name: 'Perm',            lat: 58.01, lon: 56.25, mult: 2.0, half: 0.22 },
  { name: 'Volgograd',       lat: 48.71, lon: 44.51, mult: 2.0, half: 0.30 }, // long N-S along Volga
  // Tier 3 — sub-million regional capitals (×1.5)
  { name: 'Saratov',         lat: 51.53, lon: 46.03, mult: 1.5, half: 0.18 },
  { name: 'Tyumen',          lat: 57.15, lon: 65.53, mult: 1.5, half: 0.18 },
  { name: 'Tolyatti',        lat: 53.51, lon: 49.42, mult: 1.5, half: 0.18 },
  { name: 'Barnaul',         lat: 53.35, lon: 83.78, mult: 1.5, half: 0.18 },
  { name: 'Makhachkala',     lat: 42.98, lon: 47.50, mult: 1.5, half: 0.18 },
  { name: 'Izhevsk',         lat: 56.85, lon: 53.20, mult: 1.5, half: 0.18 },
  { name: 'Khabarovsk',      lat: 48.48, lon: 135.07, mult: 1.5, half: 0.18 },
  { name: 'Ulyanovsk',       lat: 54.32, lon: 48.40, mult: 1.5, half: 0.18 },
  { name: 'Irkutsk',         lat: 52.29, lon: 104.28, mult: 1.5, half: 0.18 },
  { name: 'Vladivostok',     lat: 43.12, lon: 131.89, mult: 1.5, half: 0.18 },
  { name: 'Yaroslavl',       lat: 57.63, lon: 39.87, mult: 1.5, half: 0.18 },
  { name: 'Tomsk',           lat: 56.49, lon: 84.95, mult: 1.5, half: 0.18 },
  { name: 'Stavropol',       lat: 45.04, lon: 41.97, mult: 1.5, half: 0.18 },
  { name: 'Kemerovo',        lat: 55.35, lon: 86.09, mult: 1.5, half: 0.18 },
]

function cityMultiplier(lat: number, lon: number): number {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.mult
  }
  return 1.0
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes) ──
const BASE_AADT: Record<number, number> = {
  0: 25000, 10: 12500, // motorway + motorway_link
  1: 14000, 11: 7000,  // trunk + trunk_link
  2: 7000,  12: 3500,  // primary + primary_link
  3: 3000,             // secondary
  4: 1200,             // tertiary
  5: 300,              // residential
}
const RU_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

/** Heavy-vehicle share by road class — trucks concentrate on the high-order network. */
function heavyShare(cls: number): number {
  if (cls === 0 || cls === 1 || cls === 10 || cls === 11) return 0.27 // motorway/trunk + links
  if (cls === 2 || cls === 3 || cls === 12) return 0.12               // primary/secondary + primary_link
  return 0.05                                                          // tertiary/residential
}

function splitVehicles(aadt: number, cls: number) {
  const heavy = heavyShare(cls)
  const medium = 0.10 // LCV
  const moto = 0.01
  const light = 1 - heavy - medium - moto
  return {
    light: Math.round(aadt * light),
    medium: Math.round(aadt * medium),
    heavy: Math.round(aadt * heavy),
    moto: Math.round(aadt * moto),
  }
}

async function main() {
  console.log(`=== RU Roads Enrichment — Russia-tuned CNOSSOS class defaults (${YEAR}) ===\n`)

  // Created here (not module scope): makeCoastalCountryGate may download+convert the CGAZ
  // boundary file on a fresh host. RU has 14 land neighbours + the antimeridian, so
  // a rectangular bbox alone would stamp Russian AADT onto Finnish/Kazakh/Mongolian/
  // Chinese roads that lack their own national enrichment — the polygon gate prevents it.
  const inRU = makeCoastalCountryGate('RU')

  const hexDirs = iterateCountryHexes(H3R4_DIR, RU_HEX_BBOX)
  console.log(`  RU-bbox hexes with roads.arrow: ${hexDirs.length}\n`)

  let totalRoads = 0, enriched = 0, hexesUpdated = 0, preserved = 0, outsideRU = 0
  const byClass: Record<number, number> = {}
  const byMult: Record<string, number> = {}
  let sumHeavy = 0, sumAll = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        // Cheap gates first; the polygon gate (inRU) is the expensive call — last.
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        const base = BASE_AADT[row.roadClass]
        if (base === undefined) return null
        if (!inBbox(row.midLat, row.midLon, RU_HEX_BBOX)) return null
        if (!inRU(row.midLat, row.midLon)) { outsideRU++; return null }

        const mult = cityMultiplier(row.midLat, row.midLon)
        const aadt = base * mult
        const s = splitVehicles(aadt, row.roadClass)
        return { ...s, sourceId: MY_SOURCE_ID }
      },
      (row, _i, applied) => {
        enriched++
        byClass[row.roadClass] = (byClass[row.roadClass] || 0) + 1
        const m = cityMultiplier(row.midLat, row.midLon)
        const key = m === 3.5 ? 'Moscow ×3.5' : m === 3.0 ? 'SPb ×3.0' : m === 2.0 ? 'millionnik ×2.0' : m === 1.5 ? 'regional ×1.5' : 'rural ×1.0'
        byMult[key] = (byMult[key] || 0) + 1
        sumHeavy += applied.heavy
        sumAll += applied.light + applied.medium + applied.heavy
      },
      RU_COVERAGE,
    )
    totalRoads += r.rows
    if (r.updated) hexesUpdated++

    const elapsed = Date.now() - startTime
    if (elapsed > 10_000 && hi % 100 === 0) {
      console.log(`  [${(elapsed / 1000).toFixed(0)}s] ${hi + 1}/${hexDirs.length} hexes, ${enriched.toLocaleString()} enriched`)
    }
  }

  const CLASS_NAME: Record<number, string> = {
    0: 'motorway', 1: 'trunk', 2: 'primary', 3: 'secondary', 4: 'tertiary',
    5: 'residential', 10: 'motorway_link', 11: 'trunk_link', 12: 'primary_link',
  }
  console.log(`\n=== Results ===`)
  console.log(`  Total roads scanned:     ${totalRoads.toLocaleString()}`)
  console.log(`  Preserved (higher prio): ${preserved.toLocaleString()}`)
  console.log(`  Outside RU polygon:      ${outsideRU.toLocaleString()}`)
  console.log(`  Enriched:                ${enriched.toLocaleString()} (${(100 * enriched / Math.max(totalRoads, 1)).toFixed(1)}%)`)
  console.log(`  Hexes updated:           ${hexesUpdated}/${hexDirs.length}`)
  console.log(`\n  By road class:`)
  for (const cls of Object.keys(byClass).map(Number).sort((a, b) => a - b)) {
    console.log(`    ${CLASS_NAME[cls] ?? cls}: ${byClass[cls].toLocaleString()}`)
  }
  console.log(`\n  By city tier:`)
  for (const [k, v] of Object.entries(byMult).sort((a, b) => b[1] - a[1])) {
    console.log(`    ${k}: ${v.toLocaleString()}`)
  }
  console.log(`\n  Mean heavy share (light+med+heavy): ${(100 * sumHeavy / Math.max(sumAll, 1)).toFixed(1)}%  (expect ~5-27% by class mix)`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
