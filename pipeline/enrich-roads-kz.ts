/**
 * Enrich KZ roads.arrow with Kazakhstan-tuned CNOSSOS class defaults.
 *
 * No open per-segment AADT dataset exists for Kazakhstan. KazAvtoyol (the national
 * roads operator) and MTRDI publish no traffic counts — same situation as RU / CN /
 * IN, where the national authority keeps (or never collects) per-segment AADT. So we
 * apply country-tuned CNOSSOS class defaults rather than a per-segment join.
 *
 * The defaults are anchored to real Kazakh geography, not guessed: KZ is the world's
 * 9th-largest country (2.72M km²) with only ~20M people (7.4/km²), so most of the
 * network is very sparse intercity steppe road. Almost all population and traffic
 * concentrate on the Almaty / Astana / Shymkent corridor; freight concentrates on
 * two heavy-industry corridors (Ekibastuz coal ~100 Mtpa, Caspian Tengiz/Kashagan oil).
 *
 * ## AADT by OSM class (two-way veh/day) — docs "Rural" column ×tier multiplier
 *
 * | class          | rural  | tier-1 (×2.0) | tier-2 (×1.4) |
 * |----------------|-------:|--------------:|--------------:|
 * | 0 motorway     | 20,000 | 40,000        | 28,000        |
 * | 1 trunk        |  8,000 | 16,000        | 11,200        |
 * | 2 primary      |  4,000 |  8,000        |  5,600        |
 * | 3 secondary    |  2,000 |  4,000        |  2,800        |
 * | 4 tertiary     |    800 |  1,600        |  1,120        |
 * | 5 residential  |    350 |    700        |    490        |
 *
 * Links (10/11/12) get ~half the parent-class rural flow (ramp traffic), matching the
 * RU link convention.
 *
 * ## City tiers (AADT multiplier vs rural baseline)
 *   Tier-1 ×2.0: Almaty (~2M, former capital), Astana (~1.3M, capital).
 *   Tier-2 ×1.4: 14 regional cities (Shymkent, Karaganda, Aktobe, Atyrau, Aktau,
 *   Pavlodar, Kostanay, Oskemen, Semey, Taraz, Petropavl, Turkestan, Kyzylorda, Uralsk).
 *   No published per-city scalar exists; calibrated to KZ's Almaty/Astana dominance.
 *
 * ## Vehicle split — heavy share is the single most acoustically important input
 *   The docs split tables are keyed by *tier*, NOT by road class (unlike RU, which keys
 *   the split by class). We implement this faithfully: the light/medium/heavy/moto split
 *   depends on the tier the road's midpoint falls in, and is class-independent within a
 *   tier. This is a defensible modelling choice — KZ's heavy-vehicle stream is driven by
 *   where you are (sparse rural intercity freight vs dense metro commuter traffic vs an
 *   industrial freight corridor) far more than by the road's OSM class.
 *
 *   | tier                          | light | medium | heavy | moto |
 *   |-------------------------------|------:|-------:|------:|-----:|
 *   | tier-1 (Almaty/Astana)        |  0.72 |  0.06  |  0.18 | 0.04 |
 *   | tier-2                        |  0.68 |  0.05  |  0.24 | 0.03 |
 *   | rural                         |  0.55 |  0.03  |  0.40 | 0.02 |
 *   | freight corridor (Ekibastuz / |  0.35 |  0.02  |  0.62 | 0.01 |
 *   |   Caspian oil)                |       |        |       |      |
 *
 *   Very LOW moto share (1-4%) — extreme continental climate (-30°C northern winters);
 *   Russian-influenced car culture (Russian + Japanese imports dominant).
 *
 *   The freight-corridor split (62% heavy) is applied to roads whose midpoint falls in
 *   the Ekibastuz/Pavlodar coal box or the Caspian Atyrau↔Tengiz↔Aktau oil zone, AND
 *   whose class is 0-2 (motorway/trunk/primary) + their links. We restrict the corridor
 *   override to the high-order network on purpose: the 62%-heavy stream is the long-haul
 *   coal/oil trunk flow, not the residential/tertiary streets inside those towns (those
 *   keep their tier split). Tier-1/tier-2 metro membership takes precedence over the
 *   corridor override where they overlap (none do today, but the ordering is explicit).
 *
 * Sources: docs/about/asia/kz.md "Road traffic" section. Full provenance in
 * data/enrichment/2026/kz/provenance-roads.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-kz.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_KZ_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { inBbox } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCoastalCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_KZ_NATIONAL_ROADS


// KZ coarse bbox [minLat,minLon,maxLat,maxLon] — fed to iterateCountryHexes (skips
// the rest of the planet) and re-checked per midpoint. KZ spans ~40.5–55.5°N,
// ~46.5–87.4°E; the box is padded slightly. The makeCoastalCountryGate('KZ') polygon is the
// real filter — KZ borders RU/CN/KG/UZ/TM and this box overlaps all five, so the box
// alone would stamp Kazakh AADT onto neighbour roads that lack their own enrichment.
const KZ_HEX_BBOX: [number, number, number, number] = [40.0, 46.0, 56.0, 88.0]

// ── City tiers (AADT multiplier vs rural baseline) ──
// half = bbox half-extent in degrees around the centroid. Tier-1 wider (0.30) to cover
// the ring roads + inner suburbs; tier-2 ~city extent (0.18).
interface City { name: string; lat: number; lon: number; mult: number; half: number }

const CITIES: City[] = [
  // Tier 1 (×2.0)
  { name: 'Almaty', lat: 43.24, lon: 76.92, mult: 2.0, half: 0.30 },
  { name: 'Astana', lat: 51.13, lon: 71.43, mult: 2.0, half: 0.30 },
  // Tier 2 (×1.4)
  { name: 'Shymkent',   lat: 42.32, lon: 69.59, mult: 1.4, half: 0.18 },
  { name: 'Karaganda',  lat: 49.80, lon: 73.10, mult: 1.4, half: 0.18 },
  { name: 'Aktobe',     lat: 50.28, lon: 57.21, mult: 1.4, half: 0.18 },
  { name: 'Atyrau',     lat: 47.12, lon: 51.88, mult: 1.4, half: 0.18 },
  { name: 'Aktau',      lat: 43.65, lon: 51.16, mult: 1.4, half: 0.18 },
  { name: 'Pavlodar',   lat: 52.29, lon: 76.97, mult: 1.4, half: 0.18 },
  { name: 'Kostanay',   lat: 53.21, lon: 63.62, mult: 1.4, half: 0.18 },
  { name: 'Oskemen',    lat: 49.95, lon: 82.61, mult: 1.4, half: 0.18 }, // Ust-Kamenogorsk
  { name: 'Semey',      lat: 50.41, lon: 80.23, mult: 1.4, half: 0.18 },
  { name: 'Taraz',      lat: 42.90, lon: 71.39, mult: 1.4, half: 0.18 },
  { name: 'Petropavl',  lat: 54.87, lon: 69.15, mult: 1.4, half: 0.18 },
  { name: 'Turkestan',  lat: 43.30, lon: 68.25, mult: 1.4, half: 0.18 },
  { name: 'Kyzylorda',  lat: 44.85, lon: 65.51, mult: 1.4, half: 0.18 },
  { name: 'Uralsk',     lat: 51.23, lon: 51.37, mult: 1.4, half: 0.18 },
]

/** City tier multiplier (1.0 = rural) for the road midpoint. */
function cityMultiplier(lat: number, lon: number): number {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.mult
  }
  return 1.0
}

// ── Freight corridors (elevated-HGV vehicle split, 62% heavy) ──
// Ekibastuz/Pavlodar coal box + Caspian Atyrau↔Tengiz↔Aktau oil zone, as small bboxes.
// Membership is tested on the road midpoint; the 62%-heavy split is applied only to the
// high-order trunk network (classes 0-2 + their links) inside these boxes — that long-
// haul coal/oil flow, not the in-town streets.
const FREIGHT_ZONES: ReadonlyArray<[number, number, number, number]> = [
  [51.3, 74.8, 52.6, 77.3], // Ekibastuz / Pavlodar coal box  [minLat,minLon,maxLat,maxLon]
  [43.4, 51.0, 47.6, 54.2], // Caspian oil zone Atyrau↔Tengiz↔Aktau
]
const FREIGHT_CLASSES: ReadonlySet<number> = new Set([0, 1, 2, 10, 11, 12])

function inFreightCorridor(lat: number, lon: number, cls: number): boolean {
  if (!FREIGHT_CLASSES.has(cls)) return false
  return FREIGHT_ZONES.some(z => inBbox(lat, lon, z))
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes), rural baseline ──
const BASE_AADT: Record<number, number> = {
  0: 20000, 10: 10000, // motorway + motorway_link
  1: 8000,  11: 4000,  // trunk + trunk_link
  2: 4000,  12: 2000,  // primary + primary_link
  3: 2000,             // secondary
  4: 800,              // tertiary
  5: 350,              // residential
}
const KZ_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

/** Vehicle-split fractions keyed by tier (docs: split depends on tier, not class). */
type Split = { light: number; medium: number; heavy: number; moto: number }
const TIER1_SPLIT:   Split = { light: 0.72, medium: 0.06, heavy: 0.18, moto: 0.04 }
const TIER2_SPLIT:   Split = { light: 0.68, medium: 0.05, heavy: 0.24, moto: 0.03 }
const RURAL_SPLIT:   Split = { light: 0.55, medium: 0.03, heavy: 0.40, moto: 0.02 }
const FREIGHT_SPLIT: Split = { light: 0.35, medium: 0.02, heavy: 0.62, moto: 0.01 }

/** Pick the tier split. Freight corridor wins on the high-order network; otherwise
 *  metro tier (by multiplier) wins; else rural. Returns the split + a tier label. */
function tierSplit(lat: number, lon: number, cls: number, mult: number): { split: Split; tier: string } {
  if (inFreightCorridor(lat, lon, cls)) return { split: FREIGHT_SPLIT, tier: 'freight ×0.62hgv' }
  if (mult === 2.0) return { split: TIER1_SPLIT, tier: 'tier-1 ×2.0' }
  if (mult === 1.4) return { split: TIER2_SPLIT, tier: 'tier-2 ×1.4' }
  return { split: RURAL_SPLIT, tier: 'rural ×1.0' }
}

function splitVehicles(aadt: number, split: Split) {
  return {
    light: Math.round(aadt * split.light),
    medium: Math.round(aadt * split.medium),
    heavy: Math.round(aadt * split.heavy),
    moto: Math.round(aadt * split.moto),
  }
}

async function main() {
  console.log(`=== KZ Roads Enrichment — Kazakhstan-tuned CNOSSOS class defaults (${YEAR}) ===\n`)

  // Created here (not module scope): makeCoastalCountryGate may download+convert the CGAZ
  // boundary file on a fresh host. KZ borders RU/CN/KG/UZ/TM; a rectangular bbox alone
  // would stamp Kazakh AADT onto Russian/Chinese/Kyrgyz/Uzbek/Turkmen roads that lack
  // their own national enrichment — the polygon gate prevents it.
  const inKZ = makeCoastalCountryGate('KZ')

  const hexDirs = iterateCountryHexes(H3R4_DIR, KZ_HEX_BBOX)
  console.log(`  KZ-bbox hexes with roads.arrow: ${hexDirs.length}\n`)

  let totalRoads = 0, enriched = 0, hexesUpdated = 0, preserved = 0, outsideKZ = 0
  const byClass: Record<number, number> = {}
  const byTier: Record<string, number> = {}
  let sumHeavy = 0, sumAll = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        // Cheap gates first; the polygon gate (inKZ) is the expensive call — last.
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        const base = BASE_AADT[row.roadClass]
        if (base === undefined) return null
        if (!inBbox(row.midLat, row.midLon, KZ_HEX_BBOX)) return null
        if (!inKZ(row.midLat, row.midLon)) { outsideKZ++; return null }

        const mult = cityMultiplier(row.midLat, row.midLon)
        const aadt = base * mult
        const { split } = tierSplit(row.midLat, row.midLon, row.roadClass, mult)
        const s = splitVehicles(aadt, split)
        return { ...s, sourceId: MY_SOURCE_ID }
      },
      (row, _i, applied) => {
        enriched++
        byClass[row.roadClass] = (byClass[row.roadClass] || 0) + 1
        const mult = cityMultiplier(row.midLat, row.midLon)
        const { tier } = tierSplit(row.midLat, row.midLon, row.roadClass, mult)
        byTier[tier] = (byTier[tier] || 0) + 1
        sumHeavy += applied.heavy
        sumAll += applied.light + applied.medium + applied.heavy
      },
      KZ_COVERAGE,
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
  console.log(`  Outside KZ polygon:      ${outsideKZ.toLocaleString()}`)
  console.log(`  Enriched:                ${enriched.toLocaleString()} (${(100 * enriched / Math.max(totalRoads, 1)).toFixed(1)}%)`)
  console.log(`  Hexes updated:           ${hexesUpdated}/${hexDirs.length}`)
  console.log(`\n  By road class:`)
  for (const cls of Object.keys(byClass).map(Number).sort((a, b) => a - b)) {
    console.log(`    ${CLASS_NAME[cls] ?? cls}: ${byClass[cls].toLocaleString()}`)
  }
  console.log(`\n  By tier:`)
  for (const [k, v] of Object.entries(byTier).sort((a, b) => b[1] - a[1])) {
    console.log(`    ${k}: ${v.toLocaleString()}`)
  }
  console.log(`\n  Mean heavy share (light+med+heavy): ${(100 * sumHeavy / Math.max(sumAll, 1)).toFixed(1)}%  (expect ~18-40% by tier mix, much higher in corridors)`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
