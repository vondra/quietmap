/**
 * Enrich CD roads.arrow with DR-Congo-tuned CNOSSOS class defaults.
 *
 * DRC publishes ZERO open per-segment AADT. The Office des Routes and the
 * Ministère des Infrastructures expose only corporate HTML; the Cellule
 * Infrastructures / Agence Congolaise des Grands Travaux dashboards carry no
 * volumes. Only ~3,000 km of ~153,000 km is paved. So — like KE / NG / EG —
 * we apply country-tuned class defaults anchored to DRC geography, not a
 * per-segment join. Values are 'proxy' (classification-based, not counted).
 *
 * ## AADT by OSM class (two-way veh/day) — docs/about/africa/cd.md
 *
 * | class            | rural baseline | Tier-1 ×2.5 | Tier-2 ×1.4 |
 * |------------------|---------------:|------------:|------------:|
 * | 0 motorway       | 15,000         | 37,500      | 21,000      |  (none in DRC)
 * | 1 trunk (RN)     |  5,000         | 12,500      |  7,000      |
 * | 2 primary        |  2,500         |  6,250      |  3,500      |
 * | 3 secondary      |  1,200         |  3,000      |  1,680      |
 * | 4 tertiary       |    500         |  1,250      |    700      |
 * | 5 residential    |    250         |    625      |    350      |
 *
 * Links (10/11/12) get ~half the parent-class flow. DRC has no motorway-class
 * road; class 0 only fires on the rare OSM mis-tag and is kept for safety.
 *
 * ## City multipliers
 *   Kinshasa (~17 M metro — Africa's 3rd-largest city, extreme congestion on a
 *   minimal network) ×2.5 (Tier-1). 10 regional centres ×1.4 (Tier-2):
 *   Lubumbashi, Mbuji-Mayi, Kisangani, Kananga, Bukavu, Goma, Likasi, Kolwezi,
 *   Matadi, Kikwit. No per-city count is published; tiers calibrated by eye to
 *   population + corridor role.
 *
 * ## Vehicle split — by REGION (heavy share dominates acoustically)
 *   Kinshasa runs on informal minibuses (taxi-bus / "esprit de mort" →
 *   CNOSSOS "medium") and a fast-growing moto-taxi ("wewa") fleet; the eastern
 *   Kivu cities (Goma/Bukavu) are extremely motorcycle-heavy. The two corridors
 *   that carry DRC's truck-km are import freight Matadi↔Kinshasa (RN1, every
 *   container for the capital lands at Matadi port) and the Katanga copperbelt
 *   Kolwezi→Likasi→Lubumbashi→Kasumbalesa (cobalt/copper export trucking to the
 *   Zambian border) — both ~40 % heavy, worth 3-8 dB over the corridors that
 *   matter most.
 *     tier-1 (Kinshasa)        light .50 / med .17 / heavy .08 / moto .25
 *     tier-2 cities            light .50 / med .15 / heavy .12 / moto .23
 *     freight corridor         light .42 / med .08 / heavy .40 / moto .10
 *     rural                    light .55 / med .10 / heavy .20 / moto .15
 *
 * Sources: docs/about/africa/cd.md, World Bank DRC roads context, Katanga
 * mining-export corridor reporting. Full list in
 * data/enrichment/2026/cd/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-cd.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_CD_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCoastalCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_CD_NATIONAL_ROADS


// CD coarse bbox [minLat,minLon,maxLat,maxLon] — cheap pre-scan; the
// makeCoastalCountryGate('CD') polygon is the real filter, so this box only needs to
// enclose the DRC. Overlaps CG / CF / SS / UG / RW / BI / TZ / ZM / AO, all
// gated out by the polygon.
const CD_HEX_BBOX: [number, number, number, number] = [-13.5, 12.0, 5.5, 31.5]

// ── City tiers (AADT multiplier vs rural baseline) ──
interface City { name: string; lat: number; lon: number; tier: 1 | 2; half: number }

const CITIES: City[] = [
  // Tier 1 ×2.5 — Kinshasa metro + immediate sprawl (E along Blvd Lumumba to N'Djili).
  { name: 'Kinshasa', lat: -4.325, lon: 15.322, tier: 1, half: 0.30 },
  // Tier 2 ×1.4 — provincial capitals + mining centres.
  { name: 'Lubumbashi', lat: -11.660, lon: 27.480, tier: 2, half: 0.12 },
  { name: 'Mbuji-Mayi', lat:  -6.150, lon: 23.600, tier: 2, half: 0.10 },
  { name: 'Kisangani',  lat:   0.520, lon: 25.200, tier: 2, half: 0.10 },
  { name: 'Kananga',    lat:  -5.900, lon: 22.420, tier: 2, half: 0.09 },
  { name: 'Bukavu',     lat:  -2.510, lon: 28.840, tier: 2, half: 0.07 },
  { name: 'Goma',       lat:  -1.660, lon: 29.220, tier: 2, half: 0.07 },
  { name: 'Likasi',     lat: -10.980, lon: 26.730, tier: 2, half: 0.07 },
  { name: 'Kolwezi',    lat: -10.720, lon: 25.470, tier: 2, half: 0.07 },
  { name: 'Matadi',     lat:  -5.820, lon: 13.450, tier: 2, half: 0.07 },
  { name: 'Kikwit',     lat:  -5.040, lon: 18.820, tier: 2, half: 0.07 },
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.tier
  }
  return 0
}

// ── Heavy-truck freight corridors [lon,lat] — matched by min distance to line ──
// RN1 Matadi↔Kinshasa: the DRC's economic lifeline — every container for the
// ~17 M capital lands at Matadi (no rail capacity), trucked ~350 km up RN1.
const MATADI_KINSHASA_RN1: [number, number][] = [
  [13.45, -5.82], [13.92, -5.70], [14.44, -5.56], [15.10, -5.13], [15.31, -4.33],
]
// Katanga copperbelt Kolwezi→Likasi→Lubumbashi→Kasumbalesa: cobalt/copper export
// trucking to the Zambian border (DRC produces ~70 % of world cobalt).
const KATANGA_COPPERBELT: [number, number][] = [
  [25.47, -10.72], [26.73, -10.98], [27.48, -11.66], [27.79, -12.22],
]

function nearFreightCorridor(lat: number, lon: number): boolean {
  return pointToPolylineDist(lat, lon, MATADI_KINSHASA_RN1) <= 15000 ||
         pointToPolylineDist(lat, lon, KATANGA_COPPERBELT) <= 15000
}

// ── Region → vehicle split (fractions sum to 1.0) ──
type Region = 'tier1' | 'tier2' | 'corridor' | 'rural'
interface Split { light: number; medium: number; heavy: number; moto: number }
const SPLITS: Record<Region, Split> = {
  tier1:    { light: 0.50, medium: 0.17, heavy: 0.08, moto: 0.25 },
  tier2:    { light: 0.50, medium: 0.15, heavy: 0.12, moto: 0.23 },
  corridor: { light: 0.42, medium: 0.08, heavy: 0.40, moto: 0.10 },
  rural:    { light: 0.55, medium: 0.10, heavy: 0.20, moto: 0.15 },
}

/** Region for the vehicle split: cities first (urban minibus/wewa mix dominates
 *  the metro surface streets), then the freight corridors on their long rural
 *  stretches, then generic rural. */
function region(lat: number, lon: number, tier: 0 | 1 | 2): Region {
  if (tier === 1) return 'tier1'
  if (tier === 2) return 'tier2'
  if (nearFreightCorridor(lat, lon)) return 'corridor'
  return 'rural'
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes) ──
const BASE_AADT: Record<number, number> = {
  0: 15000, 10: 7500, // motorway + motorway_link (none in DRC; safety only)
  1: 5000,  11: 2500, // trunk + trunk_link (Routes Nationales)
  2: 2500,  12: 1250, // primary + primary_link
  3: 1200,            // secondary
  4: 500,             // tertiary
  5: 250,             // residential
}
const CD_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

function splitVehicles(aadt: number, s: Split) {
  return {
    light: Math.round(aadt * s.light),
    medium: Math.round(aadt * s.medium),
    heavy: Math.round(aadt * s.heavy),
    moto: Math.round(aadt * s.moto),
  }
}

async function main() {
  console.log(`=== CD Roads Enrichment — DR-Congo-tuned CNOSSOS class defaults (${YEAR}) ===\n`)

  // Polygon gate stops CD AADT bleeding onto CG / CF / SS / UG / RW / BI / TZ /
  // ZM / AO roads the rectangular bbox overlaps.
  const inCD = makeCoastalCountryGate('CD')

  const hexDirs = iterateCountryHexes(H3R4_DIR, CD_HEX_BBOX)
  console.log(`  CD-bbox hexes with roads.arrow: ${hexDirs.length}\n`)

  let totalRoads = 0, enriched = 0, hexesUpdated = 0, preserved = 0, outsideCD = 0
  const byClass: Record<number, number> = {}
  const byRegion: Record<string, number> = {}
  let sumHeavy = 0, sumAll = 0, sumMoto = 0, sumWithMoto = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        // Cheap gates first; the polygon gate (inCD) is the expensive call — last.
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        const base = BASE_AADT[row.roadClass]
        if (base === undefined) return null
        if (!inBbox(row.midLat, row.midLon, CD_HEX_BBOX)) return null
        if (!inCD(row.midLat, row.midLon)) { outsideCD++; return null }

        const tier = cityTier(row.midLat, row.midLon)
        const mult = tier === 1 ? 2.5 : tier === 2 ? 1.4 : 1.0
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
      CD_COVERAGE,
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
  console.log(`  Outside CD polygon:      ${outsideCD.toLocaleString()}`)
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
  console.log(`\n  Mean heavy share (light+med+heavy): ${(100 * sumHeavy / Math.max(sumAll, 1)).toFixed(1)}%  (8% Kinshasa .. 40% freight corridor)`)
  console.log(`  Mean moto share (incl. moto):       ${(100 * sumMoto / Math.max(sumWithMoto, 1)).toFixed(1)}%  (high — wewa moto-taxis, Kivu cities)`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
