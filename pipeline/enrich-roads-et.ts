/**
 * Enrich ET roads.arrow with Ethiopia-tuned CNOSSOS class defaults.
 *
 * The Ethiopian Roads Authority (ERA) publishes ZERO open per-segment AADT — its
 * site is corporate HTML/PDF (RSDP progress reports, no GIS), and there is no
 * national open-data portal with traffic counts. So — like EG/KE/NG/DZ/IR — we
 * apply country-tuned class defaults over OSM geometry, gated by
 * makeCoastalCountryGate('ET'), not a per-segment join.
 *
 * Defaults are anchored to Ethiopia's real geography (docs/about/africa/et.md):
 *
 * ## AADT by OSM class (two-way veh/day)
 *
 * | class            | rural baseline | Tier-1 ×2.0 | Tier-2 ×1.4 |
 * |------------------|---------------:|------------:|------------:|
 * | 0 motorway       | 30,000         | 60,000      | 42,000      |
 * | 1 trunk (A-route)| 10,000         | 20,000      | 14,000      |
 * | 2 primary        |  5,000         | 10,000      |  7,000      |
 * | 3 secondary      |  2,500         |  5,000      |  3,500      |
 * | 4 tertiary       |  1,200         |  2,400      |  1,680      |
 * | 5 residential    |    600         |  1,200      |    840      |
 *
 * Links (10/11/12) get ~half the parent-class flow (ramp traffic). The only
 * motorway-class roads are the Addis–Adama Expressway (Ethiopia's first 6-lane
 * expressway, 85 km, 2014) and the Addis Ababa Ring Road — everything else tops
 * out at trunk (A-routes A1..A6).
 *
 * ## City multipliers
 *   Addis Ababa (~5 M, 2,350 m — Africa's highest major capital) ×2.0 (Tier-1).
 *   18 regional centres ×1.4 (Tier-2). No per-city count is published; tiers
 *   calibrated by eye to population + corridor role.
 *
 * ## Vehicle split — by REGION (heavy share dominates acoustically)
 *   Ethiopia's urban transport runs on blue-and-white minibus taxis (CNOSSOS
 *   "medium", bus-like) and Bajaj tuktuks ("moto"). Motorcycle share is LOWER
 *   than Kenya/Nigeria (~13 % urban, not 25 %). The Addis↔Djibouti A1 is the sea
 *   outlet for landlocked Ethiopia — container trucks to/from Doraleh port make
 *   it ~45 % heavy, worth 3-8 dB over the corridor that matters most.
 *     tier-1 (Addis Ababa)      light .60 / med .15 / heavy .12 / moto .13
 *     tier-2 cities             light .62 / med .12 / heavy .14 / moto .12
 *     Addis–Djibouti corridor   light .40 / med .08 / heavy .45 / moto .07
 *     rural                     light .58 / med .10 / heavy .24 / moto .08
 *
 * Sources: 2007 census + UN projections for populations, Ethio-Djibouti corridor
 * freight context (Djibouti handles ~95 % of Ethiopian trade), ES 3985:2009 noise
 * standard context. Full list in data/enrichment/2026/et/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-et.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_ET_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCoastalCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_ET_NATIONAL_ROADS


// ET coarse bbox [minLat,minLon,maxLat,maxLon] — cheap pre-scan; makeCoastalCountryGate('ET')
// is the real filter, so this box only needs to enclose Ethiopia. Overlaps
// Djibouti / Somalia / Eritrea / Sudan / South Sudan / Kenya, all gated by the polygon.
const ET_HEX_BBOX: [number, number, number, number] = [3.4, 32.9, 14.9, 48.0]

// ── City tiers (AADT multiplier vs rural baseline) ──
interface City { name: string; lat: number; lon: number; tier: 1 | 2; half: number }

const CITIES: City[] = [
  // Tier 1 ×2.0 — Addis metro + Ring Road + satellite sprawl (Burayu/Sebeta/Legetafo).
  { name: 'Addis Ababa', lat: 9.030, lon: 38.740, tier: 1, half: 0.25 },
  // Tier 2 ×1.4 — regional capitals + major towns.
  { name: 'Dire Dawa',     lat: 9.593,  lon: 41.866, tier: 2, half: 0.10 },
  { name: 'Adama',         lat: 8.540,  lon: 39.270, tier: 2, half: 0.10 },
  { name: 'Mekelle',       lat: 13.497, lon: 39.477, tier: 2, half: 0.09 },
  { name: 'Gondar',        lat: 12.603, lon: 37.452, tier: 2, half: 0.08 },
  { name: 'Bahir Dar',     lat: 11.594, lon: 37.391, tier: 2, half: 0.08 },
  { name: 'Hawassa',       lat: 7.062,  lon: 38.478, tier: 2, half: 0.08 },
  { name: 'Dessie',        lat: 11.133, lon: 39.633, tier: 2, half: 0.07 },
  { name: 'Jimma',         lat: 7.667,  lon: 36.833, tier: 2, half: 0.07 },
  { name: 'Shashamane',    lat: 7.200,  lon: 38.600, tier: 2, half: 0.07 },
  { name: 'Dilla',         lat: 6.410,  lon: 38.310, tier: 2, half: 0.06 },
  { name: 'Debre Markos',  lat: 10.333, lon: 37.717, tier: 2, half: 0.06 },
  { name: 'Debre Birhan',  lat: 9.680,  lon: 39.533, tier: 2, half: 0.06 },
  { name: 'Assela',        lat: 7.950,  lon: 39.133, tier: 2, half: 0.06 },
  { name: 'Nekemte',       lat: 9.083,  lon: 36.550, tier: 2, half: 0.06 },
  { name: 'Kombolcha',     lat: 11.083, lon: 39.733, tier: 2, half: 0.06 },
  { name: 'Harar',         lat: 9.311,  lon: 42.128, tier: 2, half: 0.06 },
  { name: 'Arba Minch',    lat: 6.033,  lon: 37.550, tier: 2, half: 0.06 },
  { name: 'Wukro',         lat: 13.789, lon: 39.600, tier: 2, half: 0.05 },
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.tier
  }
  return 0
}

// ── Addis↔Djibouti freight corridors [lon,lat] — high HGV share (~45%) ──
// Landlocked Ethiopia routes ~95 % of its trade through Djibouti's Doraleh
// Container Terminal. Two truck routes carry it: the Afar/Galafi route (the
// dominant container-truck path through the lowlands) and the Dire Dawa route
// (parallel to the electrified EDR railway). A receiver near either runs heavy.
// Vertices trace the real A1 closely (≤~80 km spacing) so the long highway
// segments' midpoints stay inside the 15 km buffer — a coarse chord across the
// Afar valley left the main trunk mis-classified as rural (24% vs 45% HGV).
const CORRIDOR_GALAFI: [number, number][] = [
  [38.740, 9.030], [39.110, 8.590], [39.270, 8.540], [39.920, 8.900],
  [40.170, 8.990], [40.550, 9.600], [40.650, 10.170], [40.720, 10.800],
  [40.770, 11.410], [40.990, 11.790], [41.400, 11.650], [41.750, 11.580],
]
const CORRIDOR_DIRE_DAWA: [number, number][] = [
  [40.170, 8.990], [40.450, 9.100], [40.750, 9.240], [41.100, 9.220],
  [41.450, 9.410], [41.866, 9.593], [42.130, 9.750], [42.400, 9.900],
]

function nearCorridor(lat: number, lon: number): boolean {
  return pointToPolylineDist(lat, lon, CORRIDOR_GALAFI) <= 15000 ||
         pointToPolylineDist(lat, lon, CORRIDOR_DIRE_DAWA) <= 15000
}

// ── Region → vehicle split (fractions sum to 1.0) ──
type Region = 'tier1' | 'tier2' | 'corridor' | 'rural'
interface Split { light: number; medium: number; heavy: number; moto: number }
const SPLITS: Record<Region, Split> = {
  tier1:    { light: 0.60, medium: 0.15, heavy: 0.12, moto: 0.13 },
  tier2:    { light: 0.62, medium: 0.12, heavy: 0.14, moto: 0.12 },
  corridor: { light: 0.40, medium: 0.08, heavy: 0.45, moto: 0.07 },
  rural:    { light: 0.58, medium: 0.10, heavy: 0.24, moto: 0.08 },
}

/** Region for the vehicle split: cities first (urban minibus/Bajaj mix dominates
 *  metro surface streets), then the Djibouti freight corridor on rural stretches,
 *  then generic rural. */
function region(lat: number, lon: number, tier: 0 | 1 | 2): Region {
  if (tier === 1) return 'tier1'
  if (tier === 2) return 'tier2'
  if (nearCorridor(lat, lon)) return 'corridor'
  return 'rural'
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes) ──
const BASE_AADT: Record<number, number> = {
  0: 30000, 10: 15000, // motorway + motorway_link (Addis–Adama Expressway, Ring Road)
  1: 10000, 11: 5000,  // trunk + trunk_link (A-routes A1..A6)
  2: 5000,  12: 2500,  // primary + primary_link
  3: 2500,             // secondary
  4: 1200,             // tertiary
  5: 600,              // residential
}
const ET_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

function splitVehicles(aadt: number, s: Split) {
  return {
    light: Math.round(aadt * s.light),
    medium: Math.round(aadt * s.medium),
    heavy: Math.round(aadt * s.heavy),
    moto: Math.round(aadt * s.moto),
  }
}

async function main() {
  console.log(`=== ET Roads Enrichment — Ethiopia-tuned CNOSSOS class defaults (${YEAR}) ===\n`)

  // Polygon gate stops ET AADT bleeding onto Djibouti / Somalia / Eritrea / Sudan /
  // South Sudan / Kenya roads the rectangular bbox overlaps.
  const inET = makeCoastalCountryGate('ET')

  const hexDirs = iterateCountryHexes(H3R4_DIR, ET_HEX_BBOX)
  console.log(`  ET-bbox hexes with roads.arrow: ${hexDirs.length}\n`)

  let totalRoads = 0, enriched = 0, hexesUpdated = 0, preserved = 0, outsideET = 0
  const byClass: Record<number, number> = {}
  const byRegion: Record<string, number> = {}
  let sumHeavy = 0, sumAll = 0, sumMoto = 0, sumWithMoto = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        // Cheap gates first; the polygon gate (inET) is the expensive call — last.
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        const base = BASE_AADT[row.roadClass]
        if (base === undefined) return null
        if (!inBbox(row.midLat, row.midLon, ET_HEX_BBOX)) return null
        if (!inET(row.midLat, row.midLon)) { outsideET++; return null }

        const tier = cityTier(row.midLat, row.midLon)
        const mult = tier === 1 ? 2.0 : tier === 2 ? 1.4 : 1.0
        const aadt = base * mult
        const reg = region(row.midLat, row.midLon, tier)
        const s = splitVehicles(aadt, SPLITS[reg])
        return { ...s, sourceId: MY_SOURCE_ID }
      },
      (row, _i, applied) => {
        enriched++
        byClass[row.roadClass] = (byClass[row.roadClass] || 0) + 1
        const reg = region(row.midLat, row.midLon, cityTier(row.midLat, row.midLon))
        byRegion[reg] = (byRegion[reg] || 0) + 1
        sumHeavy += applied.heavy
        sumAll += applied.light + applied.medium + applied.heavy
        sumMoto += applied.moto
        sumWithMoto += applied.light + applied.medium + applied.heavy + applied.moto
      },
      ET_COVERAGE,
    )
    totalRoads += r.rows
    if (r.updated) hexesUpdated++

    const elapsed = Date.now() - startTime
    if (elapsed > 10_000 && hi % 50 === 0) {
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
  console.log(`  Outside ET polygon:      ${outsideET.toLocaleString()}`)
  console.log(`  Enriched:                ${enriched.toLocaleString()} (${(100 * enriched / Math.max(totalRoads, 1)).toFixed(1)}%)`)
  console.log(`  Hexes updated:           ${hexesUpdated}/${hexDirs.length}`)
  console.log(`\n  By road class:`)
  for (const cls of Object.keys(byClass).map(Number).sort((a, b) => a - b)) {
    console.log(`    ${CLASS_NAME[cls] ?? cls}: ${byClass[cls].toLocaleString()}`)
  }
  console.log(`\n  By region:`)
  for (const [k, v] of Object.entries(byRegion).sort((a, b) => b[1] - a[1])) {
    console.log(`    ${k}: ${v.toLocaleString()}`)
  }
  console.log(`\n  Mean heavy share (light+med+heavy): ${(100 * sumHeavy / Math.max(sumAll, 1)).toFixed(1)}%  (12% metro .. 45% Djibouti corridor)`)
  console.log(`  Mean moto share (incl. moto):       ${(100 * sumMoto / Math.max(sumWithMoto, 1)).toFixed(1)}%  (Bajaj — lower than KE/NG)`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
