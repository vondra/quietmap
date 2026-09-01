/**
 * Enrich IR roads.arrow with Iran-tuned CNOSSOS class defaults.
 *
 * Iran publishes ZERO open per-segment AADT. RAH (Road, Housing & Urban
 * Development Ministry) and its research arm (BHRC) put out no machine-readable
 * traffic census; sanctions + data isolation keep what little exists offline
 * (PDF yearbooks only). HOTOSM/OSM is geometry, already our baseline. So — like
 * CN/IN/RU/EG — we apply country-tuned class defaults, not a per-segment join.
 *
 * The defaults are anchored to Iran's real geography, not guessed:
 *
 * ## AADT by OSM class (two-way veh/day) = rural baseline × city tier
 *
 * | class         | rural  | Tier-1 ×2.5 | Tier-2 ×1.8 | Tier-3 ×1.4 |
 * |---------------|-------:|------------:|------------:|------------:|
 * | 0 motorway    | 50,000 | 125,000     | 90,000      | 70,000      |
 * | 1 trunk       | 20,000 |  50,000     | 36,000      | 28,000      |
 * | 2 primary     | 11,000 |  27,500     | 19,800      | 15,400      |
 * | 3 secondary   |  5,500 |  13,750     |  9,900      |  7,700      |
 * | 4 tertiary    |  2,500 |   6,250     |  4,500      |  3,500      |
 * | 5 residential |    900 |   2,250     |  1,620      |  1,260      |
 *
 * Links (10/11/12) get ~half the parent-class flow (ramp traffic).
 *
 * ## City tiers
 *   Tier-1 ×2.5 — Tehran (~16 M; Tehran–Karaj Freeway is among the world's most
 *     congested short motorways, Hemmat/Chamran expressways gridlocked daily).
 *   Tier-2 ×1.8 — Isfahan, Mashhad (world's 2nd-holiest Shia city, 20 M+
 *     pilgrims/yr), Tabriz, Shiraz, Karaj (Tehran satellite).
 *   Tier-3 ×1.4 — 20 provincial capitals (Ahvaz, Kerman, Qom, Urmia, …).
 *
 * ## Vehicle split — by REGION, not just class (heavy share dominates acoustically)
 *   Iran has very high car ownership (IKCO+SAIPA build ~1 M Peugeot-derived cars/yr
 *   under sanctions), so a modest moto share but a big heavy share on freight roads.
 *   Getting heavy share right is worth 3-8 dB on the corridors that matter.
 *     tier-1 (Tehran)            light .65 / med .10 / heavy .15 / moto .10
 *     tier-2/3 cities            light .65 / med .08 / heavy .19 / moto .08
 *     rural two-lane highway     light .55 / med .05 / heavy .33 / moto .07
 *     Freeway / آزادراه          light .74 / med .03 / heavy .21 / moto .02
 *     South Pars freight corr.   light .40 / med .04 / heavy .52 / moto .04
 *
 * The South Pars corridor (Asaluyeh ↔ Shiraz, plus the Asaluyeh ↔ Bushehr coast)
 * hauls petrochemicals out of the world's largest gas field — ~52% heavy, the
 * loudest road mix in the country.
 *
 * Sources: Statistical Centre of Iran city populations, RAH freeway network
 * (~2,400 km آزادراه), South Pars logistics reporting. Full derivation in
 * data/enrichment/2026/ir/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-ir.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_IR_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCoastalCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_IR_NATIONAL_ROADS


// IR coarse bbox [minLat,minLon,maxLat,maxLon] — cheap pre-scan over neighbours
// (Iraq/Turkey/Armenia/Azerbaijan/Turkmenistan/Afghanistan/Pakistan + the Gulf
// states). makeCoastalCountryGate('IR') is the real filter, so this box only needs to
// enclose Iran incl. the Caspian coast and the Persian Gulf islands.
const IR_HEX_BBOX: [number, number, number, number] = [25.0, 44.0, 39.8, 63.5]

// ── City tiers (AADT multiplier vs rural baseline) ──
interface City { name: string; lat: number; lon: number; tier: 1 | 2 | 3; half: number }

const CITIES: City[] = [
  // Tier 1 ×2.5 — Greater Tehran (box wide enough for the metro + near suburbs;
  // Karaj keeps its own Tier-2 box just to the west).
  { name: 'Tehran', lat: 35.70, lon: 51.42, tier: 1, half: 0.35 },
  // Tier 2 ×1.8 — top regional cities.
  { name: 'Mashhad', lat: 36.30, lon: 59.57, tier: 2, half: 0.18 },
  { name: 'Isfahan', lat: 32.65, lon: 51.67, tier: 2, half: 0.18 },
  { name: 'Karaj',   lat: 35.84, lon: 50.99, tier: 2, half: 0.13 },
  { name: 'Shiraz',  lat: 29.59, lon: 52.58, tier: 2, half: 0.15 },
  { name: 'Tabriz',  lat: 38.08, lon: 46.29, tier: 2, half: 0.15 },
  // Tier 3 ×1.4 — 20 provincial capitals.
  { name: 'Ahvaz',        lat: 31.32, lon: 48.67, tier: 3, half: 0.10 },
  { name: 'Qom',          lat: 34.64, lon: 50.88, tier: 3, half: 0.10 },
  { name: 'Kermanshah',   lat: 34.31, lon: 47.07, tier: 3, half: 0.10 },
  { name: 'Urmia',        lat: 37.55, lon: 45.07, tier: 3, half: 0.09 },
  { name: 'Rasht',        lat: 37.28, lon: 49.58, tier: 3, half: 0.09 },
  { name: 'Kerman',       lat: 30.28, lon: 57.08, tier: 3, half: 0.09 },
  { name: 'Hamadan',      lat: 34.80, lon: 48.51, tier: 3, half: 0.09 },
  { name: 'Arak',         lat: 34.09, lon: 49.69, tier: 3, half: 0.09 },
  { name: 'Yazd',         lat: 31.90, lon: 54.37, tier: 3, half: 0.09 },
  { name: 'Ardabil',      lat: 38.25, lon: 48.29, tier: 3, half: 0.09 },
  { name: 'Bandar Abbas', lat: 27.18, lon: 56.28, tier: 3, half: 0.09 },
  { name: 'Zahedan',      lat: 29.50, lon: 60.86, tier: 3, half: 0.09 },
  { name: 'Sanandaj',     lat: 35.31, lon: 46.99, tier: 3, half: 0.09 },
  { name: 'Zanjan',       lat: 36.68, lon: 48.49, tier: 3, half: 0.09 },
  { name: 'Gorgan',       lat: 36.84, lon: 54.44, tier: 3, half: 0.09 },
  { name: 'Bushehr',      lat: 28.92, lon: 50.84, tier: 3, half: 0.09 },
  { name: 'Khorramabad',  lat: 33.49, lon: 48.36, tier: 3, half: 0.09 },
  { name: 'Sari',         lat: 36.57, lon: 53.06, tier: 3, half: 0.09 },
  { name: 'Birjand',      lat: 32.87, lon: 59.22, tier: 3, half: 0.09 },
  { name: 'Bojnurd',      lat: 37.47, lon: 57.33, tier: 3, half: 0.09 },
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 | 3 {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.tier
  }
  return 0
}

// ── South Pars freight corridor [lon,lat] — ~52% heavy ──
// Petrochemicals leave the Asaluyeh / Pars Special Economic Zone two ways: inland
// to Shiraz (then the national network) and along the Gulf coast toward Bushehr.
const SOUTH_PARS_INLAND: [number, number][] = [[52.61, 27.48], [52.06, 27.83], [51.55, 28.40], [51.21, 29.27], [52.53, 29.59]]
const SOUTH_PARS_COAST: [number, number][] = [[52.61, 27.48], [51.55, 28.30], [50.84, 28.92]]
const SOUTH_PARS = [SOUTH_PARS_INLAND, SOUTH_PARS_COAST]

function nearSouthPars(lat: number, lon: number): boolean {
  for (const line of SOUTH_PARS) if (pointToPolylineDist(lat, lon, line) <= 20000) return true
  return false
}

// ── Region → vehicle split (fractions sum to 1.0) ──
type Region = 'tier1' | 'tier2' | 'rural' | 'freeway' | 'southpars'
interface Split { light: number; medium: number; heavy: number; moto: number }
const SPLITS: Record<Region, Split> = {
  tier1:     { light: 0.65, medium: 0.10, heavy: 0.15, moto: 0.10 },
  tier2:     { light: 0.65, medium: 0.08, heavy: 0.19, moto: 0.08 },
  rural:     { light: 0.55, medium: 0.05, heavy: 0.33, moto: 0.07 },
  freeway:   { light: 0.74, medium: 0.03, heavy: 0.21, moto: 0.02 },
  southpars: { light: 0.40, medium: 0.04, heavy: 0.52, moto: 0.04 },
}

/** Region for the vehicle split. Cities first (urban mix dominates even where a
 *  freight road passes through, e.g. Shiraz), then the South Pars freight haul,
 *  then bare motorways (آزادراه), else rural two-lane highway. */
function region(lat: number, lon: number, roadClass: number, tier: 0 | 1 | 2 | 3): Region {
  if (tier === 1) return 'tier1'
  if (tier === 2 || tier === 3) return 'tier2'
  if (nearSouthPars(lat, lon)) return 'southpars'
  if (roadClass === 0 || roadClass === 10) return 'freeway' // motorway + link = آزادراه
  return 'rural'
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes) ──
const BASE_AADT: Record<number, number> = {
  0: 50000, 10: 25000, // motorway + motorway_link
  1: 20000, 11: 10000, // trunk + trunk_link
  2: 11000, 12: 5500,  // primary + primary_link
  3: 5500,             // secondary
  4: 2500,             // tertiary
  5: 900,              // residential
}
const IR_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

function splitVehicles(aadt: number, s: Split) {
  return {
    light: Math.round(aadt * s.light),
    medium: Math.round(aadt * s.medium),
    heavy: Math.round(aadt * s.heavy),
    moto: Math.round(aadt * s.moto),
  }
}

async function main() {
  console.log(`=== IR Roads Enrichment — Iran-tuned CNOSSOS class defaults (${YEAR}) ===\n`)

  // makeCoastalCountryGate may download+convert the CGAZ boundary on a fresh host. Iran
  // borders 7 countries + faces 6 Gulf states across the water; the polygon gate
  // stops IR AADT bleeding onto neighbour roads the rectangular bbox overlaps.
  const inIR = makeCoastalCountryGate('IR')

  const hexDirs = iterateCountryHexes(H3R4_DIR, IR_HEX_BBOX)
  console.log(`  IR-bbox hexes with roads.arrow: ${hexDirs.length}\n`)

  let totalRoads = 0, enriched = 0, hexesUpdated = 0, preserved = 0, outsideIR = 0
  const byClass: Record<number, number> = {}
  const byRegion: Record<string, number> = {}
  let sumHeavy = 0, sumAll = 0, sumMoto = 0, sumWithMoto = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        // Cheap gates first; the polygon gate (inIR) is the expensive call — last.
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        const base = BASE_AADT[row.roadClass]
        if (base === undefined) return null
        if (!inBbox(row.midLat, row.midLon, IR_HEX_BBOX)) return null
        if (!inIR(row.midLat, row.midLon)) { outsideIR++; return null }

        const tier = cityTier(row.midLat, row.midLon)
        const mult = tier === 1 ? 2.5 : tier === 2 ? 1.8 : tier === 3 ? 1.4 : 1.0
        const aadt = base * mult
        const reg = region(row.midLat, row.midLon, row.roadClass, tier)
        const s = splitVehicles(aadt, SPLITS[reg])
        return { ...s, sourceId: MY_SOURCE_ID }
      },
      (row, _i, applied) => {
        enriched++
        byClass[row.roadClass] = (byClass[row.roadClass] || 0) + 1
        const reg = region(row.midLat, row.midLon, row.roadClass, cityTier(row.midLat, row.midLon))
        byRegion[reg] = (byRegion[reg] || 0) + 1
        sumHeavy += applied.heavy
        sumAll += applied.light + applied.medium + applied.heavy
        sumMoto += applied.moto
        sumWithMoto += applied.light + applied.medium + applied.heavy + applied.moto
      },
      IR_COVERAGE,
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
  console.log(`  Outside IR polygon:      ${outsideIR.toLocaleString()}`)
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
  console.log(`\n  Mean heavy share (light+med+heavy): ${(100 * sumHeavy / Math.max(sumAll, 1)).toFixed(1)}%  (15% Tehran .. 52% South Pars freight)`)
  console.log(`  Mean moto share (incl. moto):       ${(100 * sumMoto / Math.max(sumWithMoto, 1)).toFixed(1)}%`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
