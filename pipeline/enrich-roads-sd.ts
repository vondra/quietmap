/**
 * Enrich SD roads.arrow with Sudan-tuned CNOSSOS class defaults.
 *
 * Sudan publishes ZERO open per-segment AADT: the National Highway Authority
 * (NHA), the Ministry of Infrastructure & Transport and the Sudan Roads &
 * Bridges Corporation have no machine-readable counts or open road network, and
 * the civil war (April 2023–, SAF vs RSF) has shut down what little statistical
 * capacity existed. So — like EG/DZ/CN/IN/RU — we apply country-tuned class
 * defaults over the OSM geometry, gated to Sudanese soil by the country polygon,
 * not a per-segment join.
 *
 * The defaults are anchored to Sudan's real geography, not guessed. They sit
 * BELOW Algeria/Egypt: Sudan is far poorer (~16 vehicles/1,000 vs ~160 in DZ),
 * lower motorization, and the war has gutted urban traffic in Khartoum.
 *
 * ## AADT by OSM class (two-way veh/day)
 *
 * | class            | rural baseline | Tier-1 ×2.0 | Tier-2 ×1.4 |
 * |------------------|---------------:|------------:|------------:|
 * | 0 motorway       | 30,000         | 60,000      | 42,000      |
 * | 1 trunk          |  8,000         | 16,000      | 11,200      |
 * | 2 primary        |  3,500         |  7,000      |  4,900      |
 * | 3 secondary      |  1,500         |  3,000      |  2,100      |
 * | 4 tertiary       |    600         |  1,200      |    840      |
 * | 5 residential    |    300         |    600      |    420      |
 *
 * Links (10/11/12) get ~half the parent-class flow (ramp traffic). Class 0
 * (motorway) only really exists as the few Khartoum ring-road / expressway
 * segments — hence the Tier-1 multiplier is what matters for it.
 *
 * ## City multipliers
 *   Greater Khartoum ×2.0 (Tier-1) — the ~6 M tri-city conurbation (Khartoum +
 *   Omdurman + Khartoum North/Bahri at the Blue/White Nile confluence). 7 Tier-2
 *   cities ×1.4: Port Sudan (the only functional port), Kassala, El Obeid (gum
 *   arabic), Wad Madani, Atbara (rail junction), Gedaref (sorghum), El Fasher
 *   (Darfur). No per-city scalar is published; tiers calibrated by eye to city
 *   population. Khartoum's true 2024 traffic is depressed by the war, but the
 *   map represents a structural/typical year, so the ×2.0 stands.
 *
 * ## Vehicle split — by REGION, not just class (heavy share dominates acoustically)
 *   Sudan runs a high rickshaw/motorcycle ("raksha") share in cities and a very
 *   high heavy-truck share on the long-haul freight artery to the coast. Getting
 *   heavy share right is worth 3-8 dB on the corridors that matter.
 *     tier-1 (Khartoum)          light .55 / med .12 / heavy .18 / moto .15
 *     tier-2 cities              light .50 / med .10 / heavy .22 / moto .18
 *     Khartoum–Port Sudan artery light .35 / med .08 / heavy .50 / moto .07
 *     rural (national network)   light .40 / med .08 / heavy .35 / moto .17
 *
 * Sources: CBS Sudan population estimates, Khartoum–Port Sudan highway being the
 * sole import/export land artery (Port Sudan handles ~90% of trade), gum-arabic
 * road freight geography. Full derivation in data/enrichment/2026/sd/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-sd.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_SD_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCoastalCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_SD_NATIONAL_ROADS


// SD coarse bbox [minLat,minLon,maxLat,maxLon] — cheap pre-scan over neighbours
// (Egypt/Libya/Chad/CAR/South Sudan/Ethiopia/Eritrea); makeCoastalCountryGate('SD') is
// the real filter, so this box only needs to enclose post-2011 Sudan.
const SD_HEX_BBOX: [number, number, number, number] = [8.7, 21.8, 22.2, 38.6]

// ── City tiers (AADT multiplier vs rural baseline) ──
interface City { name: string; lat: number; lon: number; tier: 1 | 2; half: number }

const CITIES: City[] = [
  // Tier 1 ×2.0 — box wide enough to cover the whole tri-city Khartoum metro
  // (Khartoum + Omdurman + Khartoum North/Bahri around the Nile confluence).
  { name: 'Greater Khartoum', lat: 15.58, lon: 32.53, tier: 1, half: 0.20 },
  // Tier 2 ×1.4 — state capitals + the port + rail/agricultural hubs.
  { name: 'Port Sudan',  lat: 19.62, lon: 37.22, tier: 2, half: 0.10 },
  { name: 'Kassala',     lat: 15.45, lon: 36.40, tier: 2, half: 0.09 },
  { name: 'El Obeid',    lat: 13.18, lon: 30.22, tier: 2, half: 0.09 },
  { name: 'Wad Madani',  lat: 14.40, lon: 33.52, tier: 2, half: 0.09 },
  { name: 'Atbara',      lat: 17.70, lon: 33.99, tier: 2, half: 0.09 },
  { name: 'Gedaref',     lat: 14.04, lon: 35.38, tier: 2, half: 0.09 },
  { name: 'El Fasher',   lat: 13.63, lon: 25.35, tier: 2, half: 0.09 },
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.tier
  }
  return 0
}

// ── Corridor polyline [lon,lat] — matched by min distance to the whole line ──
// Khartoum → Port Sudan: the sole heavy-freight land artery to the coast. The
// modern paved highway cuts NE across the Butana to Haiya, then north to Port
// Sudan; it carries the tanker/container truck traffic for ~90% of Sudan's trade.
const PORTSUDAN_ARTERY: [number, number][] = [
  [32.53, 15.58], [33.6, 16.1], [35.0, 17.2], [36.32, 18.33], [37.22, 19.62],
]
const nearArtery = (lat: number, lon: number) => pointToPolylineDist(lat, lon, PORTSUDAN_ARTERY) <= 25000

// ── Region → vehicle split (fractions sum to 1.0) ──
type Region = 'tier1' | 'tier2' | 'corridor' | 'rural'
interface Split { light: number; medium: number; heavy: number; moto: number }
const SPLITS: Record<Region, Split> = {
  tier1:    { light: 0.55, medium: 0.12, heavy: 0.18, moto: 0.15 },
  tier2:    { light: 0.50, medium: 0.10, heavy: 0.22, moto: 0.18 },
  corridor: { light: 0.35, medium: 0.08, heavy: 0.50, moto: 0.07 },
  rural:    { light: 0.40, medium: 0.08, heavy: 0.35, moto: 0.17 },
}

/** Region for the vehicle split: cities first (urban mix wins inside the metro
 *  even where the artery passes through), then the Port Sudan freight artery,
 *  else the rural national network. */
function region(lat: number, lon: number, tier: 0 | 1 | 2): Region {
  if (tier === 1) return 'tier1'
  if (tier === 2) return 'tier2'
  if (nearArtery(lat, lon)) return 'corridor'
  return 'rural'
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes) ──
const BASE_AADT: Record<number, number> = {
  0: 30000, 10: 15000, // motorway + motorway_link
  1: 8000,  11: 4000,  // trunk + trunk_link
  2: 3500,  12: 1750,  // primary + primary_link
  3: 1500,             // secondary
  4: 600,              // tertiary
  5: 300,              // residential
}
const SD_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

function splitVehicles(aadt: number, s: Split) {
  return {
    light: Math.round(aadt * s.light),
    medium: Math.round(aadt * s.medium),
    heavy: Math.round(aadt * s.heavy),
    moto: Math.round(aadt * s.moto),
  }
}

async function main() {
  console.log(`=== SD Roads Enrichment — Sudan-tuned CNOSSOS class defaults (${YEAR}) ===\n`)

  // makeCoastalCountryGate may download+convert the CGAZ boundary on a fresh host. The
  // polygon gate stops SD AADT bleeding onto Egyptian/Libyan/Chadian/South
  // Sudanese/Ethiopian/Eritrean roads the rectangular bbox overlaps.
  const inSD = makeCoastalCountryGate('SD')

  const hexDirs = iterateCountryHexes(H3R4_DIR, SD_HEX_BBOX)
  console.log(`  SD-bbox hexes with roads.arrow: ${hexDirs.length}\n`)

  let totalRoads = 0, enriched = 0, hexesUpdated = 0, preserved = 0, outsideSD = 0
  const byClass: Record<number, number> = {}
  const byRegion: Record<string, number> = {}
  let sumHeavy = 0, sumAll = 0, sumMoto = 0, sumWithMoto = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        // Cheap gates first; the polygon gate (inSD) is the expensive call — last.
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        const base = BASE_AADT[row.roadClass]
        if (base === undefined) return null
        if (!inBbox(row.midLat, row.midLon, SD_HEX_BBOX)) return null
        if (!inSD(row.midLat, row.midLon)) { outsideSD++; return null }

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
      SD_COVERAGE,
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
  console.log(`  Outside SD polygon:      ${outsideSD.toLocaleString()}`)
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
  console.log(`\n  Mean heavy share (light+med+heavy): ${(100 * sumHeavy / Math.max(sumAll, 1)).toFixed(1)}%  (18% Khartoum .. 50% Port Sudan artery)`)
  console.log(`  Mean moto share (incl. moto):       ${(100 * sumMoto / Math.max(sumWithMoto, 1)).toFixed(1)}%  (raksha/rickshaw — high in cities)`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
