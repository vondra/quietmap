/**
 * Enrich KE roads.arrow with Kenya-tuned CNOSSOS class defaults.
 *
 * Kenya publishes ZERO open per-segment AADT. KeNHA / KURA / KeRRA expose only
 * MangoGIS viewer dashboards (road class / condition / surface, no volumes) and
 * the Kenya Open Data Initiative (opendata.go.ke) has been dormant since ~2015.
 * HOTOSM Kenya Roads (HDX) is an OSM re-export already in our baseline. So — like
 * EG/CN/IN/RU — we apply country-tuned class defaults, not a per-segment join.
 *
 * The defaults are anchored to Kenya's real geography, not guessed:
 *
 * ## AADT by OSM class (two-way veh/day)
 *
 * | class            | rural baseline | Tier-1 ×2.0 | Tier-2 ×1.4 |
 * |------------------|---------------:|------------:|------------:|
 * | 0 motorway       | 30,000         | 60,000      | 42,000      |
 * | 1 trunk (A/B)    | 10,000         | 20,000      | 14,000      |
 * | 2 primary        |  5,000         | 10,000      |  7,000      |
 * | 3 secondary      |  2,500         |  5,000      |  3,500      |
 * | 4 tertiary       |  1,200         |  2,400      |  1,680      |
 * | 5 residential    |    600         |  1,200      |    840      |
 *
 * Links (10/11/12) get ~half the parent-class flow (ramp traffic). The only
 * motorway-class road is the 27 km Nairobi Expressway (tolled, JKIA↔Westlands,
 * opened 2022) — everything else tops out at trunk (Class A/B highways).
 *
 * ## City multipliers
 *   Nairobi (~4.9 M, East Africa's financial hub) + Mombasa (~1.2 M, Indian Ocean
 *   port) ×2.0 (Tier-1). 18 county/regional centres ×1.4 (Tier-2). No per-city
 *   count is published; tiers calibrated by eye to population + corridor role.
 *
 * ## Vehicle split — by REGION (heavy share dominates acoustically)
 *   Kenya's urban transport runs on matatus (shared 14-seat minibuses → CNOSSOS
 *   "medium", bus-like acoustic profile) and boda-bodas (motorcycle taxis → "moto",
 *   ~25 % of urban traffic). The Mombasa↔Nairobi A10/Mombasa-Road container corridor
 *   carries the bulk of East Africa's port freight to the Embakasi ICD and on to
 *   Uganda — ~35 % heavy trucks, worth 3-8 dB over the corridor that matters most.
 *     tier-1 (Nairobi/Mombasa)  light .50 / med .15 / heavy .10 / moto .25
 *     tier-2 cities             light .52 / med .15 / heavy .10 / moto .23
 *     container corridor (A10)  light .45 / med .08 / heavy .35 / moto .12
 *     rural                     light .55 / med .10 / heavy .20 / moto .15
 *
 * Sources: KNBS 2019 census populations, KeNHA Northern/Mombasa corridor freight
 * reporting, NEMA Noise Regs 2009 context. Full list in
 * data/enrichment/2026/ke/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-ke.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_KE_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCoastalCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_KE_NATIONAL_ROADS


// KE coarse bbox [minLat,minLon,maxLat,maxLon] — cheap pre-scan; the makeCoastalCountryGate('KE')
// polygon is the real filter, so this box only needs to enclose Kenya. Overlaps
// Tanzania / Uganda / South Sudan / Ethiopia / Somalia, all gated out by the polygon.
const KE_HEX_BBOX: [number, number, number, number] = [-4.7, 33.9, 5.5, 41.9]

// ── City tiers (AADT multiplier vs rural baseline) ──
interface City { name: string; lat: number; lon: number; tier: 1 | 2; half: number }

const CITIES: City[] = [
  // Tier 1 ×2.0 — wide boxes cover the metro + immediate satellite sprawl.
  { name: 'Nairobi', lat: -1.286, lon: 36.817, tier: 1, half: 0.30 },
  { name: 'Mombasa', lat: -4.043, lon: 39.668, tier: 1, half: 0.18 },
  // Tier 2 ×1.4 — county capitals + regional centres.
  { name: 'Nakuru',   lat: -0.303, lon: 36.080, tier: 2, half: 0.10 },
  { name: 'Eldoret',  lat:  0.514, lon: 35.270, tier: 2, half: 0.10 },
  { name: 'Kisumu',   lat: -0.092, lon: 34.768, tier: 2, half: 0.10 },
  { name: 'Thika',    lat: -1.033, lon: 37.069, tier: 2, half: 0.08 },
  { name: 'Ruiru',    lat: -1.146, lon: 36.961, tier: 2, half: 0.06 },
  { name: 'Kiambu',   lat: -1.171, lon: 36.835, tier: 2, half: 0.06 },
  { name: 'Machakos', lat: -1.519, lon: 37.265, tier: 2, half: 0.08 },
  { name: 'Naivasha', lat: -0.717, lon: 36.431, tier: 2, half: 0.08 },
  { name: 'Kitale',   lat:  1.015, lon: 35.006, tier: 2, half: 0.08 },
  { name: 'Malindi',  lat: -3.219, lon: 40.117, tier: 2, half: 0.08 },
  { name: 'Kakamega', lat:  0.282, lon: 34.752, tier: 2, half: 0.08 },
  { name: 'Kisii',    lat: -0.681, lon: 34.767, tier: 2, half: 0.08 },
  { name: 'Embu',     lat: -0.539, lon: 37.457, tier: 2, half: 0.08 },
  { name: 'Meru',     lat:  0.047, lon: 37.649, tier: 2, half: 0.08 },
  { name: 'Nyeri',    lat: -0.420, lon: 36.947, tier: 2, half: 0.08 },
  { name: 'Garissa',  lat: -0.453, lon: 39.646, tier: 2, half: 0.08 },
  { name: 'Lamu',     lat: -2.272, lon: 40.902, tier: 2, half: 0.06 },
  { name: 'Kilifi',   lat: -3.630, lon: 39.849, tier: 2, half: 0.08 },
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.tier
  }
  return 0
}

// ── Mombasa↔Nairobi container corridor [lon,lat] — high HGV share (~35%) ──
// A10 / Mombasa Road, parallel to SGR Phase 1: port freight from Mombasa to the
// Embakasi ICD and on toward Uganda (Northern Corridor). ~480 km of empty Tsavo
// between the two metros runs heavy with container trucks.
const CONTAINER_CORRIDOR: [number, number][] = [
  [39.668, -4.043], [38.556, -3.396], [38.169, -2.690], [37.825, -2.281],
  [37.461, -2.103], [36.980, -1.450], [36.900, -1.340], [36.817, -1.286],
]

function nearContainerCorridor(lat: number, lon: number): boolean {
  return pointToPolylineDist(lat, lon, CONTAINER_CORRIDOR) <= 15000
}

// ── Region → vehicle split (fractions sum to 1.0) ──
type Region = 'tier1' | 'tier2' | 'corridor' | 'rural'
interface Split { light: number; medium: number; heavy: number; moto: number }
const SPLITS: Record<Region, Split> = {
  tier1:    { light: 0.50, medium: 0.15, heavy: 0.10, moto: 0.25 },
  tier2:    { light: 0.52, medium: 0.15, heavy: 0.10, moto: 0.23 },
  corridor: { light: 0.45, medium: 0.08, heavy: 0.35, moto: 0.12 },
  rural:    { light: 0.55, medium: 0.10, heavy: 0.20, moto: 0.15 },
}

/** Region for the vehicle split: cities first (urban matatu/boda mix dominates the
 *  metro surface streets), then the container corridor on its long rural stretches,
 *  then generic rural. */
function region(lat: number, lon: number, tier: 0 | 1 | 2): Region {
  if (tier === 1) return 'tier1'
  if (tier === 2) return 'tier2'
  if (nearContainerCorridor(lat, lon)) return 'corridor'
  return 'rural'
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes) ──
const BASE_AADT: Record<number, number> = {
  0: 30000, 10: 15000, // motorway + motorway_link (Nairobi Expressway only)
  1: 10000, 11: 5000,  // trunk + trunk_link (Class A/B)
  2: 5000,  12: 2500,  // primary + primary_link
  3: 2500,             // secondary
  4: 1200,             // tertiary
  5: 600,              // residential
}
const KE_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

function splitVehicles(aadt: number, s: Split) {
  return {
    light: Math.round(aadt * s.light),
    medium: Math.round(aadt * s.medium),
    heavy: Math.round(aadt * s.heavy),
    moto: Math.round(aadt * s.moto),
  }
}

async function main() {
  console.log(`=== KE Roads Enrichment — Kenya-tuned CNOSSOS class defaults (${YEAR}) ===\n`)

  // Polygon gate stops KE AADT bleeding onto Tanzania / Uganda / South Sudan /
  // Ethiopia / Somalia roads the rectangular bbox overlaps.
  const inKE = makeCoastalCountryGate('KE')

  const hexDirs = iterateCountryHexes(H3R4_DIR, KE_HEX_BBOX)
  console.log(`  KE-bbox hexes with roads.arrow: ${hexDirs.length}\n`)

  let totalRoads = 0, enriched = 0, hexesUpdated = 0, preserved = 0, outsideKE = 0
  const byClass: Record<number, number> = {}
  const byRegion: Record<string, number> = {}
  let sumHeavy = 0, sumAll = 0, sumMoto = 0, sumWithMoto = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        // Cheap gates first; the polygon gate (inKE) is the expensive call — last.
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        const base = BASE_AADT[row.roadClass]
        if (base === undefined) return null
        if (!inBbox(row.midLat, row.midLon, KE_HEX_BBOX)) return null
        if (!inKE(row.midLat, row.midLon)) { outsideKE++; return null }

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
      KE_COVERAGE,
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
  console.log(`  Outside KE polygon:      ${outsideKE.toLocaleString()}`)
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
  console.log(`\n  Mean heavy share (light+med+heavy): ${(100 * sumHeavy / Math.max(sumAll, 1)).toFixed(1)}%  (10% metro .. 35% container corridor)`)
  console.log(`  Mean moto share (incl. moto):       ${(100 * sumMoto / Math.max(sumWithMoto, 1)).toFixed(1)}%  (high — boda-boda taxis)`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
