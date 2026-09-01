/**
 * Enrich UA roads.arrow with Ukraine-tuned CNOSSOS class defaults.
 *
 * NOTE: Ukraine has been under Russian invasion since February 2022. This is a
 * **pre-war baseline** — actual flows in occupied/frontline oblasts (Donetsk,
 * Luhansk, Zaporizhzhia, Kherson, Kharkiv approaches, Mariupol) are drastically
 * different (destroyed roads, military-only traffic, depopulation). We model the
 * undisturbed network so the rest of the country reads correctly.
 *
 * No open per-segment AADT dataset exists for Ukraine. Ukravtodor (Державне
 * агентство автомобільних доріг) publishes open data only for procurement /
 * anti-corruption transparency ("Transparent Infrastructure" portal on
 * data.gov.ua); the count methodology is standardised (ДСТУ 8824:2019,
 * середньодобова інтенсивність руху) but the measured intensities are internal,
 * and wartime collection is impossible across the occupied/frontline south-east.
 * So — like CN/IN/RU/TH — we apply country-tuned class defaults, not a join.
 *
 * ## AADT by OSM class (two-way veh/day) — from docs/about/europe/ua.md
 *
 * | class            | rural  | Kyiv ×2.0 | tier-2 ×1.6 | tier-3 ×1.3 | anchor |
 * |------------------|-------:|----------:|------------:|------------:|--------|
 * | 0 motorway       | 28,000 |    56,000 |      44,800 |      36,400 | M-06 Kyiv-Chop, Kyiv ring approaches |
 * | 1 trunk          | 14,000 |    28,000 |      22,400 |      18,200 | M-routes (single/dual carriageway) |
 * | 2 primary        |  7,000 |    14,000 |      11,200 |       9,100 | H-routes, oblast arterials |
 * | 3 secondary      |  3,500 |     7,000 |       5,600 |       4,550 | inter-raion |
 * | 4 tertiary       |  1,500 |     3,000 |       2,400 |       1,950 | local connectors |
 * | 5 residential    |    600 |     1,200 |         960 |         780 | city streets |
 *
 * Links (10/11/12) get ~half the parent-class flow (ramp traffic).
 *
 * ## City tiers (AADT multiplier vs rural baseline)
 *   Kyiv ×2.0 (capital, ~3M) — tier 1.
 *   ×1.6 regional capitals: Kharkiv, Odesa, Dnipro, Lviv, Zaporizhzhia, Donetsk.
 *   ×1.3 oblast centres (17 cities, incl. Mariupol — pre-war).
 *   No published per-city scalar exists; tiers calibrated by population/role, in
 *   line with docs/about/europe/ua.md. Gentler than RU's ×3.5 Moscow because the
 *   largest Ukrainian city (Kyiv ~3M) is a quarter of Moscow's.
 *
 * ## Vehicle split — heavy share is the single most acoustically important input
 *   Ukraine is a major grain / oilseed / steel exporter: trucks concentrate on the
 *   M-road trunks feeding the Black Sea ports (Odesa, Pivdennyi, Mykolaiv) and the
 *   western EU border crossings. The registered fleet is car-dominated (cold
 *   climate, negligible motorcycle use), so heavy share is high on trunks, low on
 *   local streets — and only ~1% moto everywhere.
 *     motorway/trunk (+links): light 0.71 / medium 0.10 / heavy 0.18 / moto 0.01
 *     primary/secondary (+pl) : light 0.79 / medium 0.10 / heavy 0.10 / moto 0.01
 *     tertiary/residential    : light 0.85 / medium 0.10 / heavy 0.04 / moto 0.01
 *   18% heavy on trunks matches the CNOSSOS motorway default (19%) and the
 *   grain-export freight reality without the long-haul transit share that pushes
 *   Russia's toll corridors to ~27%.
 *
 * Sources: Ukravtodor open-data portal (procurement only — no counts), ДСТУ
 * 8824:2019 count methodology, pre-war fleet/population statistics. Full list +
 * URLs in data/enrichment/2026/ua/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-ua.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_UA_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { inBbox } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCoastalCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_UA_NATIONAL_ROADS


// UA coarse bbox [minLat,minLon,maxLat,maxLon] — fed to iterateCountryHexes (skips
// the rest of the planet) and re-checked per midpoint. Includes Crimea (south to
// ~44.4°N) — internationally Ukrainian, modelled at pre-war baseline. The
// makeCoastalCountryGate('UA') polygon is the real filter; this box is only a cheap
// pre-scan over neighbours (RU/BY/PL/SK/HU/RO/MD).
const UA_HEX_BBOX: [number, number, number, number] = [44.0, 22.0, 52.5, 40.5]

// ── City tiers (AADT multiplier vs rural baseline) ──
// half = bbox half-extent in degrees around the centroid. Kyiv wider to cover the
// ring + suburbs; regional/oblast ~city extent.
interface City { name: string; lat: number; lon: number; mult: number; half: number }

const CITIES: City[] = [
  // Tier 1 — capital (×2.0)
  { name: 'Kyiv',            lat: 50.45, lon: 30.52, mult: 2.0, half: 0.30 },
  // Tier 2 — regional capitals (×1.6)
  { name: 'Kharkiv',         lat: 49.99, lon: 36.23, mult: 1.6, half: 0.18 },
  { name: 'Odesa',           lat: 46.48, lon: 30.72, mult: 1.6, half: 0.18 },
  { name: 'Dnipro',          lat: 48.46, lon: 35.04, mult: 1.6, half: 0.18 },
  { name: 'Lviv',            lat: 49.84, lon: 24.03, mult: 1.6, half: 0.18 },
  { name: 'Zaporizhzhia',    lat: 47.84, lon: 35.14, mult: 1.6, half: 0.18 },
  { name: 'Donetsk',         lat: 48.02, lon: 37.80, mult: 1.6, half: 0.18 },
  // Tier 3 — oblast centres (×1.3)
  { name: 'Mykolaiv',        lat: 46.97, lon: 31.99, mult: 1.3, half: 0.12 },
  { name: 'Vinnytsia',       lat: 49.23, lon: 28.47, mult: 1.3, half: 0.12 },
  { name: 'Poltava',         lat: 49.59, lon: 34.55, mult: 1.3, half: 0.12 },
  { name: 'Chernihiv',       lat: 51.49, lon: 31.29, mult: 1.3, half: 0.12 },
  { name: 'Zhytomyr',        lat: 50.25, lon: 28.66, mult: 1.3, half: 0.12 },
  { name: 'Sumy',            lat: 50.91, lon: 34.80, mult: 1.3, half: 0.12 },
  { name: 'Khmelnytskyi',    lat: 49.42, lon: 26.99, mult: 1.3, half: 0.12 },
  { name: 'Cherkasy',        lat: 49.44, lon: 32.06, mult: 1.3, half: 0.12 },
  { name: 'Ivano-Frankivsk', lat: 48.92, lon: 24.71, mult: 1.3, half: 0.12 },
  { name: 'Ternopil',        lat: 49.55, lon: 25.59, mult: 1.3, half: 0.12 },
  { name: 'Rivne',           lat: 50.62, lon: 26.25, mult: 1.3, half: 0.12 },
  { name: 'Uzhhorod',        lat: 48.62, lon: 22.30, mult: 1.3, half: 0.12 },
  { name: 'Lutsk',           lat: 50.75, lon: 25.34, mult: 1.3, half: 0.12 },
  { name: 'Chernivtsi',      lat: 48.29, lon: 25.94, mult: 1.3, half: 0.12 },
  { name: 'Kropyvnytskyi',   lat: 48.51, lon: 32.26, mult: 1.3, half: 0.12 },
  { name: 'Kremenchuk',      lat: 49.07, lon: 33.42, mult: 1.3, half: 0.12 },
  { name: 'Mariupol',        lat: 47.10, lon: 37.55, mult: 1.3, half: 0.12 },
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
  0: 28000, 10: 14000, // motorway + motorway_link
  1: 14000, 11: 7000,  // trunk + trunk_link
  2: 7000,  12: 3500,  // primary + primary_link
  3: 3500,             // secondary
  4: 1500,             // tertiary
  5: 600,              // residential
}
const UA_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

/** Heavy-vehicle share by road class — grain/steel freight concentrates on trunks. */
function heavyShare(cls: number): number {
  if (cls === 0 || cls === 1 || cls === 10 || cls === 11) return 0.18 // motorway/trunk + links
  if (cls === 2 || cls === 3 || cls === 12) return 0.10               // primary/secondary + primary_link
  return 0.04                                                          // tertiary/residential
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
  console.log(`=== UA Roads Enrichment — Ukraine-tuned CNOSSOS class defaults, pre-war baseline (${YEAR}) ===\n`)

  // Created here (not module scope): makeCoastalCountryGate may download+convert the CGAZ
  // boundary file on a fresh host. UA has 7 land neighbours (RU/BY/PL/SK/HU/RO/MD),
  // several being enriched in parallel right now — a rectangular bbox alone would
  // stamp Ukrainian AADT onto Russian/Polish/Romanian roads. The polygon gate stops it.
  const inUA = makeCoastalCountryGate('UA')

  const hexDirs = iterateCountryHexes(H3R4_DIR, UA_HEX_BBOX)
  console.log(`  UA-bbox hexes with roads.arrow: ${hexDirs.length}\n`)

  let totalRoads = 0, enriched = 0, hexesUpdated = 0, preserved = 0, outsideUA = 0
  const byClass: Record<number, number> = {}
  const byMult: Record<string, number> = {}
  let sumHeavy = 0, sumAll = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        // Cheap gates first; the polygon gate (inUA) is the expensive call — last.
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        const base = BASE_AADT[row.roadClass]
        if (base === undefined) return null
        if (!inBbox(row.midLat, row.midLon, UA_HEX_BBOX)) return null
        if (!inUA(row.midLat, row.midLon)) { outsideUA++; return null }

        const mult = cityMultiplier(row.midLat, row.midLon)
        const aadt = base * mult
        const s = splitVehicles(aadt, row.roadClass)
        return { ...s, sourceId: MY_SOURCE_ID }
      },
      (row, _i, applied) => {
        enriched++
        byClass[row.roadClass] = (byClass[row.roadClass] || 0) + 1
        const m = cityMultiplier(row.midLat, row.midLon)
        const key = m === 2.0 ? 'Kyiv ×2.0' : m === 1.6 ? 'regional ×1.6' : m === 1.3 ? 'oblast ×1.3' : 'rural ×1.0'
        byMult[key] = (byMult[key] || 0) + 1
        sumHeavy += applied.heavy
        sumAll += applied.light + applied.medium + applied.heavy
      },
      UA_COVERAGE,
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
  console.log(`  Outside UA polygon:      ${outsideUA.toLocaleString()}`)
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
  console.log(`\n  Mean heavy share (light+med+heavy): ${(100 * sumHeavy / Math.max(sumAll, 1)).toFixed(1)}%  (expect ~4-18% by class mix)`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
