/**
 * Enrich IQ roads.arrow with Iraq-tuned CNOSSOS class defaults.
 *
 * Iraq publishes ZERO open per-segment AADT. The Ministry of Construction,
 * Housing & Public Municipalities (roads) and the Ministry of Transport keep no
 * machine-readable traffic census; two decades of war (2003 invasion, 2014-17
 * ISIS occupation of the north) wrecked both the network and the statistical
 * capacity to count it. OSM/HOTOSM is geometry, already our baseline. So — like
 * IR/EG/CN — we apply country-tuned class defaults gated to Iraqi soil, not a
 * per-segment join.
 *
 * The defaults are anchored to Iraq's real geography, not guessed:
 *
 * ## AADT by OSM class (two-way veh/day) = rural baseline × city tier
 *
 * | class         | rural  | Tier-1 ×2.5 | Tier-2 ×1.6 | Tier-3 ×1.3 |
 * |---------------|-------:|------------:|------------:|------------:|
 * | 0 motorway    | 40,000 | 100,000     | 64,000      | 52,000      |
 * | 1 trunk       | 18,000 |  45,000     | 28,800      | 23,400      |
 * | 2 primary     |  9,000 |  22,500     | 14,400      | 11,700      |
 * | 3 secondary   |  4,500 |  11,250     |  7,200      |  5,850      |
 * | 4 tertiary    |  2,000 |   5,000     |  3,200      |  2,600      |
 * | 5 residential |    800 |   2,000     |  1,280      |  1,040      |
 *
 * Links (10/11/12) get ~half the parent-class flow (ramp traffic).
 *
 * ## City tiers
 *   Tier-1 ×2.5 — Baghdad (~8 M; the Tigris bisects the city, expressways such as
 *     Muhammad al-Qasim and the Airport Rd are gridlocked daily — one of the
 *     Middle East's largest, most congested metros).
 *   Tier-2 ×1.6 — Basra (oil capital + Iraq's only deep-water port, Shatt al-Arab),
 *     Erbil and Sulaymaniyah (the booming KRG Kurdistan cities with the country's
 *     most reliable grid + traffic).
 *   Tier-3 ×1.3 — 14 provincial capitals (Mosul, Najaf, Karbala, Kirkuk, …).
 *
 * ## Vehicle split — by REGION, not just class (heavy share dominates acoustically)
 *   Iraq runs an old, import-heavy car fleet and a very large diesel-truck haul
 *   (oil, cement, reconstruction). Getting heavy share right is worth 3-8 dB on
 *   the corridors that matter.
 *     tier-1 (Baghdad)        light .60 / med .10 / heavy .22 / moto .08
 *     tier-2/3 cities         light .58 / med .08 / heavy .26 / moto .08
 *     rural highway           light .50 / med .05 / heavy .38 / moto .07
 *     Basra oil corridor      light .35 / med .03 / heavy .58 / moto .04
 *
 * The Basra oil corridor (the Rumaila / West Qurna / Zubair super-giant fields →
 * Basra → Faw export terminals) hauls crude, rig kit and reconstruction freight
 * — ~58% heavy, the loudest road mix in the country.
 *
 * Sources: city populations (UN/Iraq CSO estimates), the field/port geography of
 * Iraq's southern oil province, KRG grid+traffic reporting. Full derivation in
 * data/enrichment/2026/iq/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-iq.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_IQ_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCoastalCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_IQ_NATIONAL_ROADS


// IQ coarse bbox [minLat,minLon,maxLat,maxLon] — cheap pre-scan over neighbours
// (Iran/Turkey/Syria/Jordan/Saudi/Kuwait). makeCoastalCountryGate('IQ') is the real
// filter, so this box only needs to enclose Iraq from the Faw peninsula (~29.06)
// up to the Turkish border (~37.4) and from the western desert (~38.7) to the
// Iranian frontier near Basra (~48.6).
const IQ_HEX_BBOX: [number, number, number, number] = [29.0, 38.7, 37.4, 48.6]

// ── City tiers (AADT multiplier vs rural baseline) ──
interface City { name: string; lat: number; lon: number; tier: 1 | 2 | 3; half: number }

const CITIES: City[] = [
  // Tier 1 ×2.5 — Greater Baghdad (~8 M; box wide enough for the metro + Tigris
  // suburbs out to Abu Ghraib / Mada'in).
  { name: 'Baghdad', lat: 33.31, lon: 44.36, tier: 1, half: 0.25 },
  // Tier 2 ×1.6 — oil capital + KRG twin cities.
  { name: 'Basra',        lat: 30.51, lon: 47.78, tier: 2, half: 0.14 },
  { name: 'Erbil',        lat: 36.19, lon: 44.01, tier: 2, half: 0.13 },
  { name: 'Sulaymaniyah', lat: 35.56, lon: 45.43, tier: 2, half: 0.11 },
  // Tier 3 ×1.3 — 14 provincial capitals.
  { name: 'Mosul',      lat: 36.34, lon: 43.13, tier: 3, half: 0.13 },
  { name: 'Najaf',      lat: 32.00, lon: 44.33, tier: 3, half: 0.09 },
  { name: 'Karbala',    lat: 32.61, lon: 44.02, tier: 3, half: 0.09 },
  { name: 'Kirkuk',     lat: 35.47, lon: 44.39, tier: 3, half: 0.10 },
  { name: 'Nasiriyah',  lat: 31.05, lon: 46.26, tier: 3, half: 0.08 },
  { name: 'Hillah',     lat: 32.47, lon: 44.43, tier: 3, half: 0.08 },
  { name: 'Diwaniyah',  lat: 31.99, lon: 44.93, tier: 3, half: 0.08 },
  { name: 'Kut',        lat: 32.51, lon: 45.82, tier: 3, half: 0.08 },
  { name: 'Tikrit',     lat: 34.61, lon: 43.68, tier: 3, half: 0.07 },
  { name: 'Samarra',    lat: 34.20, lon: 43.87, tier: 3, half: 0.07 },
  { name: 'Fallujah',   lat: 33.35, lon: 43.79, tier: 3, half: 0.08 },
  { name: 'Ramadi',     lat: 33.42, lon: 43.31, tier: 3, half: 0.08 },
  { name: 'Amarah',     lat: 31.84, lon: 47.15, tier: 3, half: 0.08 },
  { name: 'Duhok',      lat: 36.87, lon: 42.99, tier: 3, half: 0.08 },
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 | 3 {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.tier
  }
  return 0
}

// ── Basra oil corridor [lon,lat] — ~58% heavy ──
// Crude, rig kit and reconstruction freight move between the southern super-giant
// fields (West Qurna ~31.0N, Rumaila/Zubair ~30.1-30.4N) and Basra → the Faw
// export terminals. The whole southern oil province is heavy-truck dominated.
const BASRA_OIL: [number, number][] = [
  [47.43, 31.00], // West Qurna
  [47.25, 30.20], // Rumaila
  [47.55, 30.40], // Zubair
  [47.83, 30.51], // Basra
  [48.30, 29.97], // toward Faw / Khor al-Zubair port
]

function nearBasraOil(lat: number, lon: number): boolean {
  return pointToPolylineDist(lat, lon, BASRA_OIL) <= 25000
}

// ── Region → vehicle split (fractions sum to 1.0) ──
type Region = 'tier1' | 'tier2' | 'rural' | 'basra'
interface Split { light: number; medium: number; heavy: number; moto: number }
const SPLITS: Record<Region, Split> = {
  tier1: { light: 0.60, medium: 0.10, heavy: 0.22, moto: 0.08 },
  tier2: { light: 0.58, medium: 0.08, heavy: 0.26, moto: 0.08 },
  rural: { light: 0.50, medium: 0.05, heavy: 0.38, moto: 0.07 },
  basra: { light: 0.35, medium: 0.03, heavy: 0.58, moto: 0.04 },
}

/** Region for the vehicle split. Cities first (the urban mix dominates even where
 *  a freight road passes through, e.g. Basra city centre), then the Basra oil
 *  haul, else rural highway. */
function region(lat: number, lon: number, tier: 0 | 1 | 2 | 3): Region {
  if (tier === 1) return 'tier1'
  if (tier === 2 || tier === 3) return 'tier2'
  if (nearBasraOil(lat, lon)) return 'basra'
  return 'rural'
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes) ──
const BASE_AADT: Record<number, number> = {
  0: 40000, 10: 20000, // motorway + motorway_link
  1: 18000, 11: 9000,  // trunk + trunk_link
  2: 9000,  12: 4500,  // primary + primary_link
  3: 4500,             // secondary
  4: 2000,             // tertiary
  5: 800,              // residential
}
const IQ_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

function splitVehicles(aadt: number, s: Split) {
  return {
    light: Math.round(aadt * s.light),
    medium: Math.round(aadt * s.medium),
    heavy: Math.round(aadt * s.heavy),
    moto: Math.round(aadt * s.moto),
  }
}

async function main() {
  console.log(`=== IQ Roads Enrichment — Iraq-tuned CNOSSOS class defaults (${YEAR}) ===\n`)

  // makeCoastalCountryGate may download+convert the CGAZ boundary on a fresh host. The
  // polygon gate stops IQ AADT bleeding onto the roads of the six neighbours the
  // rectangular bbox overlaps (esp. dense SW Iran across the Basra frontier).
  const inIQ = makeCoastalCountryGate('IQ')

  const hexDirs = iterateCountryHexes(H3R4_DIR, IQ_HEX_BBOX)
  console.log(`  IQ-bbox hexes with roads.arrow: ${hexDirs.length}\n`)

  let totalRoads = 0, enriched = 0, hexesUpdated = 0, preserved = 0, outsideIQ = 0
  const byClass: Record<number, number> = {}
  const byRegion: Record<string, number> = {}
  let sumHeavy = 0, sumAll = 0, sumMoto = 0, sumWithMoto = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        // Cheap gates first; the polygon gate (inIQ) is the expensive call — last.
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        const base = BASE_AADT[row.roadClass]
        if (base === undefined) return null
        if (!inBbox(row.midLat, row.midLon, IQ_HEX_BBOX)) return null
        if (!inIQ(row.midLat, row.midLon)) { outsideIQ++; return null }

        const tier = cityTier(row.midLat, row.midLon)
        const mult = tier === 1 ? 2.5 : tier === 2 ? 1.6 : tier === 3 ? 1.3 : 1.0
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
      IQ_COVERAGE,
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
  console.log(`  Outside IQ polygon:      ${outsideIQ.toLocaleString()}`)
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
  console.log(`\n  Mean heavy share (light+med+heavy): ${(100 * sumHeavy / Math.max(sumAll, 1)).toFixed(1)}%  (22% Baghdad .. 58% Basra oil corridor)`)
  console.log(`  Mean moto share (incl. moto):       ${(100 * sumMoto / Math.max(sumWithMoto, 1)).toFixed(1)}%`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
