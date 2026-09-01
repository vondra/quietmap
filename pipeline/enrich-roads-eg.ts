/**
 * Enrich EG roads.arrow with Egypt-tuned CNOSSOS class defaults.
 *
 * Egypt publishes ZERO open per-segment AADT and every government portal is
 * dead or auth-gated (re-confirmed in the 2026-04 research, unchanged since):
 *   MOT, GARBLT (road authority), NREA, NAT (tunnels/Cairo Metro), Ministry of
 *   Petroleum, EGAS, ENR (→ Vaadin login), CAPMAS (HTML SPA), EgyptERA — all
 *   TCP-timeout / connection-refused / no machine-readable API. HOTOSM Egypt
 *   Roads (HDX) is just an OSM re-export, already in our baseline. So — like
 *   CN/IN/RU — we apply country-tuned class defaults, not a per-segment join.
 *
 * The defaults are anchored to Egypt's real geography, not guessed:
 *
 * ## AADT by OSM class (two-way veh/day)
 *
 * | class            | rural baseline | Tier-1 ×2.5 | Tier-2 ×1.5 |
 * |------------------|---------------:|------------:|------------:|
 * | 0 motorway       | 40,000         | 100,000     | 60,000      |
 * | 1 trunk          | 15,000         | 37,500      | 22,500      |
 * | 2 primary        |  7,000         | 17,500      | 10,500      |
 * | 3 secondary      |  3,500         |  8,750      |  5,250      |
 * | 4 tertiary       |  1,500         |  3,750      |  2,250      |
 * | 5 residential    |    700         |  1,750      |  1,050      |
 *
 * Links (10/11/12) get ~half the parent-class flow (ramp traffic).
 *
 * ## City multipliers
 *   Greater Cairo / Giza / Alexandria ×2.5 (Tier-1) — note the 2.5, NOT the usual
 *   2.0: 22 M people are packed into the narrow Nile Valley and the Cairo Ring
 *   Road (El Tareeq El Da'ery) + 26th July Corridor regularly exceed 200,000 AADT
 *   in peak sections, among the most congested urban highways in the world. 21
 *   Tier-2 governorate capitals + canal/tourism cities ×1.5. Calibrated by eye to
 *   the published Ring Road counts; no per-city scalar is published.
 *
 * ## Vehicle split — by REGION, not just class (heavy share dominates acoustically)
 *   Egypt's mix swings hard by region: Cairo runs a huge motorcycle/tuktuk share
 *   (~27%) while the Suez/Red-Sea desert freight corridors run ~30% heavy trucks.
 *   Getting heavy share right is worth 3-8 dB on the corridors that matter.
 *     tier-1 (Cairo/Alex)   light .55 / med .08 / heavy .10 / moto .27
 *     tier-2 cities         light .60 / med .08 / heavy .12 / moto .20
 *     desert freight roads  light .52 / med .08 / heavy .30 / moto .10
 *     Upper Egypt (S Nile)  light .55 / med .10 / heavy .20 / moto .15
 *     rural Delta (N)       light .58 / med .08 / heavy .20 / moto .14
 *
 * Sources: CAPMAS 2017 census populations, Cairo Ring Road congestion reporting,
 * EEAA Law 4/1994 noise context. Full list in data/enrichment/2026/eg/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-eg.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_EG_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCoastalCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_EG_NATIONAL_ROADS


// EG coarse bbox [minLat,minLon,maxLat,maxLon] — cheap pre-scan over neighbours
// (Libya/Sudan/Israel/Gaza/Saudi/Jordan); the makeCoastalCountryGate('EG') polygon is the
// real filter, so this box only needs to enclose Egypt incl. Sinai + Halaib triangle.
const EG_HEX_BBOX: [number, number, number, number] = [22.0, 24.7, 31.7, 36.9]

// ── City tiers (AADT multiplier vs rural baseline) ──
interface City { name: string; lat: number; lon: number; tier: 1 | 2; half: number }

const CITIES: City[] = [
  // Tier 1 ×2.5 — Cairo box is wide enough to also cover Giza, New Cairo, 6th October.
  { name: 'Greater Cairo', lat: 30.05, lon: 31.24, tier: 1, half: 0.45 },
  { name: 'Alexandria',    lat: 31.20, lon: 29.92, tier: 1, half: 0.30 },
  // Tier 2 ×1.5 — governorate capitals + Suez-Canal + Red-Sea/Sinai tourism cities.
  { name: 'Port Said',        lat: 31.26, lon: 32.30, tier: 2, half: 0.12 },
  { name: 'Suez',             lat: 29.97, lon: 32.55, tier: 2, half: 0.12 },
  { name: 'Ismailia',         lat: 30.60, lon: 32.27, tier: 2, half: 0.12 },
  { name: 'Luxor',            lat: 25.70, lon: 32.64, tier: 2, half: 0.12 },
  { name: 'Aswan',            lat: 24.09, lon: 32.90, tier: 2, half: 0.12 },
  { name: 'Hurghada',         lat: 27.26, lon: 33.81, tier: 2, half: 0.12 },
  { name: 'Sharm El Sheikh',  lat: 27.91, lon: 34.33, tier: 2, half: 0.12 },
  { name: 'Tanta',            lat: 30.79, lon: 31.00, tier: 2, half: 0.12 },
  { name: 'Mansoura',         lat: 31.04, lon: 31.38, tier: 2, half: 0.12 },
  { name: 'Mahalla el-Kubra', lat: 30.97, lon: 31.17, tier: 2, half: 0.12 },
  { name: 'Asyut',            lat: 27.18, lon: 31.18, tier: 2, half: 0.12 },
  { name: 'Sohag',            lat: 26.56, lon: 31.69, tier: 2, half: 0.12 },
  { name: 'Damanhur',         lat: 31.03, lon: 30.47, tier: 2, half: 0.12 },
  { name: 'Zagazig',          lat: 30.59, lon: 31.50, tier: 2, half: 0.12 },
  { name: 'Faiyum',           lat: 29.31, lon: 30.84, tier: 2, half: 0.12 },
  { name: 'Minya',            lat: 28.10, lon: 30.75, tier: 2, half: 0.12 },
  { name: 'Beni Suef',        lat: 29.07, lon: 31.10, tier: 2, half: 0.12 },
  { name: 'Qena',             lat: 26.16, lon: 32.72, tier: 2, half: 0.12 },
  { name: 'Kafr el-Sheikh',   lat: 31.11, lon: 30.94, tier: 2, half: 0.12 },
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.tier
  }
  return 0
}

// ── Desert freight corridors [lon,lat] — high HGV share (~30%) ──
// Cairo↔Alex Desert Road, Cairo↔Suez, Cairo↔Ain Sokhna, and the Red Sea coast road
// (Suez↔Hurghada↔Marsa Alam) carry Egypt's truck freight across empty desert.
const DESERT_ROAD: [number, number][] = [[31.05, 30.05], [30.70, 30.30], [30.30, 30.60], [29.95, 30.95]]
const SUEZ_ROAD: [number, number][] = [[31.45, 30.00], [31.90, 29.98], [32.40, 29.97]]
const SOKHNA_ROAD: [number, number][] = [[31.50, 29.95], [31.90, 29.78], [32.34, 29.60]]
const RED_SEA_ROAD: [number, number][] = [[32.50, 29.90], [33.00, 28.70], [33.50, 27.70], [33.81, 27.26], [34.40, 25.90], [35.10, 24.30]]
const FREIGHT_CORRIDORS = [DESERT_ROAD, SUEZ_ROAD, SOKHNA_ROAD, RED_SEA_ROAD]

function nearFreightCorridor(lat: number, lon: number): boolean {
  for (const line of FREIGHT_CORRIDORS) if (pointToPolylineDist(lat, lon, line) <= 15000) return true
  return false
}

// ── Region → vehicle split (fractions sum to 1.0) ──
type Region = 'tier1' | 'tier2' | 'desert' | 'upper' | 'delta'
interface Split { light: number; medium: number; heavy: number; moto: number }
const SPLITS: Record<Region, Split> = {
  tier1:  { light: 0.55, medium: 0.08, heavy: 0.10, moto: 0.27 },
  tier2:  { light: 0.60, medium: 0.08, heavy: 0.12, moto: 0.20 },
  desert: { light: 0.52, medium: 0.08, heavy: 0.30, moto: 0.10 },
  upper:  { light: 0.55, medium: 0.10, heavy: 0.20, moto: 0.15 },
  delta:  { light: 0.58, medium: 0.08, heavy: 0.20, moto: 0.14 },
}

/** Region for the vehicle split: cities first, then freight corridors, then the
 *  Upper-Egypt (south of ~Faiyum) vs Delta (north) split at the Cairo latitude. */
function region(lat: number, lon: number, tier: 0 | 1 | 2): Region {
  if (tier === 1) return 'tier1'
  if (tier === 2) return 'tier2'
  if (nearFreightCorridor(lat, lon)) return 'desert'
  return lat < 29.5 ? 'upper' : 'delta'
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes) ──
const BASE_AADT: Record<number, number> = {
  0: 40000, 10: 20000, // motorway + motorway_link
  1: 15000, 11: 7500,  // trunk + trunk_link
  2: 7000,  12: 3500,  // primary + primary_link
  3: 3500,             // secondary
  4: 1500,             // tertiary
  5: 700,              // residential
}
const EG_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

function splitVehicles(aadt: number, s: Split) {
  return {
    light: Math.round(aadt * s.light),
    medium: Math.round(aadt * s.medium),
    heavy: Math.round(aadt * s.heavy),
    moto: Math.round(aadt * s.moto),
  }
}

async function main() {
  console.log(`=== EG Roads Enrichment — Egypt-tuned CNOSSOS class defaults (${YEAR}) ===\n`)

  // makeCoastalCountryGate may download+convert the CGAZ boundary on a fresh host. Egypt
  // borders Libya/Sudan/Israel/Gaza and is a short hop from Saudi/Jordan; the
  // polygon gate stops EG AADT bleeding onto neighbour roads the rectangular bbox
  // overlaps (Sinai is wedged between Israel, Gaza, Saudi and Jordan).
  const inEG = makeCoastalCountryGate('EG')

  const hexDirs = iterateCountryHexes(H3R4_DIR, EG_HEX_BBOX)
  console.log(`  EG-bbox hexes with roads.arrow: ${hexDirs.length}\n`)

  let totalRoads = 0, enriched = 0, hexesUpdated = 0, preserved = 0, outsideEG = 0
  const byClass: Record<number, number> = {}
  const byRegion: Record<string, number> = {}
  let sumHeavy = 0, sumAll = 0, sumMoto = 0, sumWithMoto = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        // Cheap gates first; the polygon gate (inEG) is the expensive call — last.
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        const base = BASE_AADT[row.roadClass]
        if (base === undefined) return null
        if (!inBbox(row.midLat, row.midLon, EG_HEX_BBOX)) return null
        if (!inEG(row.midLat, row.midLon)) { outsideEG++; return null }

        const tier = cityTier(row.midLat, row.midLon)
        const mult = tier === 1 ? 2.5 : tier === 2 ? 1.5 : 1.0
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
      EG_COVERAGE,
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
  console.log(`  Outside EG polygon:      ${outsideEG.toLocaleString()}`)
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
  console.log(`\n  Mean heavy share (light+med+heavy): ${(100 * sumHeavy / Math.max(sumAll, 1)).toFixed(1)}%  (10% Cairo .. 30% desert freight)`)
  console.log(`  Mean moto share (incl. moto):       ${(100 * sumMoto / Math.max(sumWithMoto, 1)).toFixed(1)}%  (high — Cairo tuktuk/motorcycle)`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
