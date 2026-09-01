/**
 * Enrich TR roads.arrow with Turkey-tuned CNOSSOS class defaults.
 *
 * KGM (Karayolları Genel Müdürlüğü, the national road authority) publishes traffic
 * volumes only as printed annual yearbooks — no open per-segment AADT, no API, no
 * WFS (re-confirmed 2026-04). So — like CN/IN/RU/EG — we apply country-tuned class
 * defaults over the OSM geometry, gated to Turkish soil by the CGAZ country polygon
 * (the bbox blankets Greece, Bulgaria, Georgia, Armenia, Iran, Iraq and Syria).
 *
 * The numbers below mirror docs/about/asia/tr.md. AADT is two-way veh/day.
 *
 * ## AADT by OSM road_class (rural baseline)
 *
 * | class         | rural  | Istanbul ×2.5 | Ankara/İzmir ×1.8 | Tier-3 ×1.4 |
 * |---------------|-------:|--------------:|------------------:|------------:|
 * | 0 motorway    | 55,000 | 137,500       | 99,000            | 77,000      |
 * | 1 trunk       | 22,000 |  55,000       | 39,600            | 30,800      |
 * | 2 primary     | 12,000 |  30,000       | 21,600            | 16,800      |
 * | 3 secondary   |  6,000 |  15,000       | 10,800            |  8,400      |
 * | 4 tertiary    |  3,000 |   7,500       |  5,400            |  4,200      |
 * | 5 residential |  1,000 |   2,500       |  1,800            |  1,400      |
 *
 * Links (10/11/12) get half the parent-class flow (ramp traffic). Turkey's O-road
 * motorways are among Europe's busiest; the rural-motorway 55k anchors the O-4
 * Istanbul↔Ankara / O-1·O-2 Istanbul-ring corridors once the city ×2.5 lands.
 *
 * ## City tiers (AADT multiplier)
 *   Tier-1 ×2.5 — Istanbul (~16 M, straddles the Bosphorus, Europe + Asia).
 *   Tier-2 ×1.8 — Ankara (capital), İzmir (Aegean).
 *   Tier-3 ×1.4 — 19 cities (Bursa, Antalya, Adana, Gaziantep, Konya, Kayseri,
 *                 Mersin, Eskişehir, Diyarbakır, Samsun, Trabzon, Şanlıurfa,
 *                 Malatya, Erzurum, Van, Denizli, Manisa, Kocaeli, Sakarya).
 *
 * ## Vehicle split — by (road class × urbanity); heavy share dominates acoustically
 *   Turkey runs a European-style mix (moderate ~7-8% moto, high car ownership) but
 *   a very large international TIR truck fleet on its inter-urban D-roads, so the
 *   rural trunk/primary heavy share is high (~32%), and higher still (~35%) on the
 *   D-400 Mediterranean transit corridor feeding the Syria/Iraq border crossings.
 *     motorway (O-roads)        light .72 / med .04 / heavy .22 / moto .02
 *     rural trunk/primary       light .55 / med .06 / heavy .32 / moto .07
 *     D-400 Mediterranean trunk light .55 / med .05 / heavy .35 / moto .05
 *     Istanbul (Tier-1) urban   light .62 / med .12 / heavy .18 / moto .08
 *     Ankara/İzmir + Tier-3     light .64 / med .10 / heavy .19 / moto .07
 *     minor rural (sec/ter/res) light .70 / med .07 / heavy .13 / moto .10
 *   "med" carries the dolmuş (shared route minibus) fleet — iconic Turkish transport.
 *
 * Sources: KGM road-network context, TÜİK city populations, doc table above. Full
 * derivation in data/enrichment/2026/tr/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-tr.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_TR_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCoastalCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_TR_NATIONAL_ROADS


// TR coarse bbox [minLat,minLon,maxLat,maxLon] — cheap pre-scan; the CGAZ
// makeCoastalCountryGate('TR') polygon is the real filter (Greece/Bulgaria to the west,
// Georgia/Armenia/Iran/Iraq/Syria to the east all overlap this box).
const TR_HEX_BBOX: [number, number, number, number] = [35.8, 25.6, 42.2, 44.8]

// ── City tiers (AADT multiplier vs rural baseline) ──
interface City { name: string; lat: number; lon: number; tier: 1 | 2 | 3; half: number }

const CITIES: City[] = [
  // Tier-1 ×2.5 — Istanbul box spans both banks of the Bosphorus.
  { name: 'Istanbul', lat: 41.05, lon: 28.95, tier: 1, half: 0.45 },
  // Tier-2 ×1.8 — capital + Aegean metropolis.
  { name: 'Ankara', lat: 39.93, lon: 32.86, tier: 2, half: 0.32 },
  { name: 'İzmir',  lat: 38.42, lon: 27.14, tier: 2, half: 0.30 },
  // Tier-3 ×1.4 — 19 cities.
  { name: 'Bursa',     lat: 40.19, lon: 29.06, tier: 3, half: 0.20 },
  { name: 'Antalya',   lat: 36.90, lon: 30.70, tier: 3, half: 0.18 },
  { name: 'Adana',     lat: 37.00, lon: 35.32, tier: 3, half: 0.18 },
  { name: 'Gaziantep', lat: 37.07, lon: 37.38, tier: 3, half: 0.18 },
  { name: 'Konya',     lat: 37.87, lon: 32.48, tier: 3, half: 0.18 },
  { name: 'Kayseri',   lat: 38.73, lon: 35.48, tier: 3, half: 0.15 },
  { name: 'Mersin',    lat: 36.81, lon: 34.63, tier: 3, half: 0.15 },
  { name: 'Eskişehir', lat: 39.78, lon: 30.52, tier: 3, half: 0.15 },
  { name: 'Diyarbakır',lat: 37.91, lon: 40.24, tier: 3, half: 0.15 },
  { name: 'Samsun',    lat: 41.29, lon: 36.33, tier: 3, half: 0.15 },
  { name: 'Trabzon',   lat: 41.00, lon: 39.72, tier: 3, half: 0.13 },
  { name: 'Şanlıurfa', lat: 37.16, lon: 38.80, tier: 3, half: 0.15 },
  { name: 'Malatya',   lat: 38.36, lon: 38.31, tier: 3, half: 0.13 },
  { name: 'Erzurum',   lat: 39.90, lon: 41.27, tier: 3, half: 0.13 },
  { name: 'Van',       lat: 38.49, lon: 43.38, tier: 3, half: 0.13 },
  { name: 'Denizli',   lat: 37.78, lon: 29.09, tier: 3, half: 0.15 },
  { name: 'Manisa',    lat: 38.61, lon: 27.43, tier: 3, half: 0.13 },
  { name: 'Kocaeli',   lat: 40.77, lon: 29.92, tier: 3, half: 0.18 }, // Ford Otosan / petrochemical
  { name: 'Sakarya',   lat: 40.78, lon: 30.40, tier: 3, half: 0.15 }, // Toyota Turkey
]

/** Highest city tier whose box contains the point (Tier-1 wins over overlaps). */
function cityTier(lat: number, lon: number): 0 | 1 | 2 | 3 {
  let best: 0 | 1 | 2 | 3 = 0
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) {
      if (best === 0 || c.tier < best) best = c.tier
    }
  }
  return best
}

// ── D-400 Mediterranean transit corridor [lon,lat] — İzmir↔Antalya↔Mersin↔Adana↔
//    Gaziantep. Carries Turkey's TIR freight to the Syria/Iraq border (~35% heavy). ──
const D400_WEST: [number, number][] = [
  [27.14, 38.42], [27.84, 37.85], [28.36, 37.21], [29.10, 36.80], [30.13, 36.88], [30.70, 36.89],
]
const D400_EAST: [number, number][] = [
  [30.70, 36.89], [31.99, 36.55], [32.83, 36.07], [33.94, 36.38], [34.64, 36.81],
  [35.33, 37.00], [36.17, 37.07], [37.06, 37.06], [37.38, 37.07],
]
function nearD400(lat: number, lon: number): boolean {
  return pointToPolylineDist(lat, lon, D400_WEST) <= 15000 ||
         pointToPolylineDist(lat, lon, D400_EAST) <= 15000
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes); links get ~half ──
const BASE_AADT: Record<number, number> = {
  0: 55000, 10: 27500, // motorway + motorway_link (O-roads)
  1: 22000, 11: 11000, // trunk + trunk_link (D-routes)
  2: 12000, 12: 6000,  // primary + primary_link
  3: 6000,             // secondary
  4: 3000,             // tertiary
  5: 1000,             // residential
}
const TR_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

// ── Vehicle splits (fractions sum to 1.0) ──
interface Split { light: number; medium: number; heavy: number; moto: number }
const MOTORWAY: Split  = { light: 0.72, medium: 0.04, heavy: 0.22, moto: 0.02 }
const RURAL_MAJOR: Split = { light: 0.55, medium: 0.06, heavy: 0.32, moto: 0.07 }
const D400: Split      = { light: 0.55, medium: 0.05, heavy: 0.35, moto: 0.05 }
const ISTANBUL: Split  = { light: 0.62, medium: 0.12, heavy: 0.18, moto: 0.08 }
const URBAN: Split     = { light: 0.64, medium: 0.10, heavy: 0.19, moto: 0.07 } // Ankara/İzmir + Tier-3
const RURAL_MINOR: Split = { light: 0.70, medium: 0.07, heavy: 0.13, moto: 0.10 }

/** Vehicle split by road class × urbanity. Heavy share is the dominant lever, so
 *  motorways (O-roads) and inter-urban D-roads (trunk/primary) keep the TIR-heavy
 *  profiles even inside a city box; minor rural roads stay local/low-heavy. */
function splitFor(roadClass: number, tier: 0 | 1 | 2 | 3, lat: number, lon: number): Split {
  if (roadClass === 0 || roadClass === 10) return MOTORWAY
  const isMajor = roadClass === 1 || roadClass === 11 || roadClass === 2 || roadClass === 12
  if (tier === 0 && isMajor) return nearD400(lat, lon) ? D400 : RURAL_MAJOR
  if (tier === 1) return ISTANBUL
  if (tier === 2 || tier === 3) return URBAN
  return RURAL_MINOR
}

function splitVehicles(aadt: number, s: Split) {
  return {
    light: Math.round(aadt * s.light),
    medium: Math.round(aadt * s.medium),
    heavy: Math.round(aadt * s.heavy),
    moto: Math.round(aadt * s.moto),
  }
}

async function main() {
  console.log(`=== TR Roads Enrichment — Turkey-tuned CNOSSOS class defaults (${YEAR}) ===\n`)

  const inTR = makeCoastalCountryGate('TR')

  const hexDirs = iterateCountryHexes(H3R4_DIR, TR_HEX_BBOX)
  console.log(`  TR-bbox hexes with roads.arrow: ${hexDirs.length}\n`)

  let totalRoads = 0, enriched = 0, hexesUpdated = 0, preserved = 0, outsideTR = 0
  const byClass: Record<number, number> = {}
  let sumHeavy = 0, sumAll = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        const base = BASE_AADT[row.roadClass]
        if (base === undefined) return null
        if (!inBbox(row.midLat, row.midLon, TR_HEX_BBOX)) return null
        if (!inTR(row.midLat, row.midLon)) { outsideTR++; return null }

        const tier = cityTier(row.midLat, row.midLon)
        const mult = tier === 1 ? 2.5 : tier === 2 ? 1.8 : tier === 3 ? 1.4 : 1.0
        const aadt = base * mult
        const s = splitVehicles(aadt, splitFor(row.roadClass, tier, row.midLat, row.midLon))
        return { ...s, sourceId: MY_SOURCE_ID }
      },
      (row, _i, applied) => {
        enriched++
        byClass[row.roadClass] = (byClass[row.roadClass] || 0) + 1
        sumHeavy += applied.heavy
        sumAll += applied.light + applied.medium + applied.heavy
      },
      TR_COVERAGE,
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
  console.log(`  Outside TR polygon:      ${outsideTR.toLocaleString()}`)
  console.log(`  Enriched:                ${enriched.toLocaleString()} (${(100 * enriched / Math.max(totalRoads, 1)).toFixed(1)}%)`)
  console.log(`  Hexes updated:           ${hexesUpdated}/${hexDirs.length}`)
  console.log(`\n  By road class:`)
  for (const cls of Object.keys(byClass).map(Number).sort((a, b) => a - b)) {
    console.log(`    ${CLASS_NAME[cls] ?? cls}: ${byClass[cls].toLocaleString()}`)
  }
  console.log(`\n  Mean heavy share (light+med+heavy): ${(100 * sumHeavy / Math.max(sumAll, 1)).toFixed(1)}%  (18% Istanbul .. 35% D-400 transit)`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
