/**
 * Enrich NG roads.arrow with Nigeria-tuned CNOSSOS class defaults.
 *
 * No open per-segment AADT exists for Nigeria. Every government portal is a
 * WordPress/HTML brochure with no GIS: FERMA (ferma.gov.ng), the Federal
 * Ministry of Works, FRSC (Federal Road Safety Corps) and LAMATA (Lagos
 * transport authority) publish zero machine-readable traffic counts. GRID3's
 * HDX national-roads layer is an OSM re-export already in our baseline. So —
 * like CN/IN/TH/RU — we apply country-tuned class defaults, not a per-segment
 * join, gated to Nigerian soil by the geoBoundaries CGAZ point-in-polygon test
 * so the rectangular bbox cannot stamp Nigerian AADT onto Benin/Niger/Chad/
 * Cameroon roads that have no national enrichment of their own.
 *
 * ## AADT by OSM class (two-way veh/day, rural baseline × city multiplier)
 *
 * | class          | rural  | Lagos ×2.5 | other Tier-1 ×2.0 | Tier-2 ×1.4 |
 * |----------------|-------:|-----------:|------------------:|------------:|
 * | 0 motorway     | 35,000 |     87,500 |            70,000 |      49,000 |
 * | 1 trunk (A-route)| 12,000 |   30,000 |            24,000 |      16,800 |
 * | 2 primary      |  6,000 |     15,000 |            12,000 |       8,400 |
 * | 3 secondary    |  3,000 |      7,500 |             6,000 |       4,200 |
 * | 4 tertiary     |  1,500 |      3,750 |             3,000 |       2,100 |
 * | 5 residential  |    700 |      1,750 |             1,400 |         980 |
 *
 * Links (10/11/12) get ~half the parent-class flow (ramp traffic).
 *
 * ## City multipliers
 *   Greater Lagos ×2.5 — Africa's largest city (~22M), extreme gridlock on the
 *     Lagos-Ibadan Expressway, Third Mainland Bridge, Apapa-Oshodi (same scalar
 *     we give Greater Cairo for comparable Nile-Valley-style density).
 *   Kano / Abuja (FCT) / Ibadan ×2.0 — the other Tier-1 metros (~3-4M each).
 *   20 Tier-2 regional cities ×1.4 (Port Harcourt, Kaduna, Benin City, …).
 *   No published per-city scalar exists; calibrated by eye to metro density.
 *
 * ## Vehicle split — Nigeria's signature is a very high two-wheeler share
 *   "Okada" motorcycle taxis + "keke napep" tricycles dominate urban transport
 *   (~35% of the urban stream; some Lagos corridors ban okada but they persist).
 *   The exception is the freight spine: the Lagos-Ibadan Expressway + Apapa /
 *   Tin Can container corridor carries ~40% of national container traffic, so it
 *   is truck-heavy (45% heavy) rather than moto-heavy — applied to motorway/
 *   trunk-class roads inside the corridor band.
 *
 *     tier            light medium heavy moto
 *     Tier-1 metro    0.45  0.05   0.15  0.35
 *     Tier-2 city     0.45  0.06   0.14  0.35
 *     rural           0.50  0.08   0.22  0.20
 *     freight corridor 0.35 0.05   0.45  0.15   (motorway/trunk in corridor)
 *
 * Sources: FRSC vehicle-mix context, NBS/UN city populations, public reporting
 * on Lagos-Ibadan/Apapa freight. Full list + URLs in
 * data/enrichment/2026/ng/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-ng.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_NG_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { inBbox } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCoastalCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_NG_NATIONAL_ROADS


// NG coarse bbox [minLat,minLon,maxLat,maxLon] — fed to iterateCountryHexes (skips
// the rest of the planet) and re-checked per midpoint. The makeCoastalCountryGate('NG')
// polygon is the real filter that keeps Benin/Niger/Chad/Cameroon out; this box
// is only a cheap pre-scan over neighbours.
const NG_HEX_BBOX: [number, number, number, number] = [4.0, 2.7, 13.9, 14.7]

// ── City tiers (AADT multiplier vs rural baseline) ──
// half = bbox half-extent in degrees around the centroid. Greater Lagos / FCT
// wider to cover the conurbation + ring; others ~city extent.
interface City { name: string; lat: number; lon: number; mult: number; half: number }

const CITIES: City[] = [
  // Tier 1
  { name: 'Lagos',  lat: 6.50, lon: 3.38, mult: 2.5, half: 0.30 }, // Greater Lagos, ~22M
  { name: 'Kano',   lat: 12.00, lon: 8.52, mult: 2.0, half: 0.18 },
  { name: 'Abuja',  lat: 9.06, lon: 7.49, mult: 2.0, half: 0.22 }, // FCT
  { name: 'Ibadan', lat: 7.38, lon: 3.90, mult: 2.0, half: 0.16 },
  // Tier 2 — 20 regional cities (×1.4)
  { name: 'Port Harcourt', lat: 4.82, lon: 7.04, mult: 1.4, half: 0.12 },
  { name: 'Benin City',    lat: 6.34, lon: 5.62, mult: 1.4, half: 0.12 },
  { name: 'Kaduna',        lat: 10.52, lon: 7.44, mult: 1.4, half: 0.12 },
  { name: 'Jos',           lat: 9.90, lon: 8.89, mult: 1.4, half: 0.12 },
  { name: 'Maiduguri',     lat: 11.83, lon: 13.15, mult: 1.4, half: 0.12 },
  { name: 'Enugu',         lat: 6.45, lon: 7.50, mult: 1.4, half: 0.12 },
  { name: 'Onitsha',       lat: 6.15, lon: 6.79, mult: 1.4, half: 0.12 },
  { name: 'Aba',           lat: 5.11, lon: 7.37, mult: 1.4, half: 0.12 },
  { name: 'Ilorin',        lat: 8.50, lon: 4.55, mult: 1.4, half: 0.12 },
  { name: 'Abeokuta',      lat: 7.16, lon: 3.35, mult: 1.4, half: 0.12 },
  { name: 'Zaria',         lat: 11.07, lon: 7.71, mult: 1.4, half: 0.12 },
  { name: 'Warri',         lat: 5.52, lon: 5.75, mult: 1.4, half: 0.12 },
  { name: 'Sokoto',        lat: 13.06, lon: 5.24, mult: 1.4, half: 0.12 },
  { name: 'Oyo',           lat: 7.85, lon: 3.93, mult: 1.4, half: 0.12 },
  { name: 'Akure',         lat: 7.25, lon: 5.20, mult: 1.4, half: 0.12 },
  { name: 'Bauchi',        lat: 10.31, lon: 9.84, mult: 1.4, half: 0.12 },
  { name: 'Calabar',       lat: 4.98, lon: 8.34, mult: 1.4, half: 0.12 },
  { name: 'Ogbomosho',     lat: 8.13, lon: 4.25, mult: 1.4, half: 0.12 },
  { name: 'Osogbo',        lat: 7.77, lon: 4.56, mult: 1.4, half: 0.12 },
  { name: 'Lokoja',        lat: 7.80, lon: 6.74, mult: 1.4, half: 0.12 },
]

// Tier-1 listed first, so an overlap resolves to the higher multiplier.
function cityMultiplier(lat: number, lon: number): number {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.mult
  }
  return 1.0
}

// ── Lagos-Ibadan / Apapa container-freight corridor ──
// ~40% of national container traffic queues through Apapa/Tin Can ports and the
// Lagos-Ibadan Expressway. The truck-heavy split is applied only to motorway/
// trunk-class roads inside these bands (ordinary Lagos arterials keep the
// moto-heavy metro split).
const FREIGHT_ZONES: [number, number, number, number][] = [
  [6.40, 3.28, 6.60, 3.45], // Apapa / Tin Can / Oshodi port-industrial Lagos
  [6.58, 3.34, 7.45, 3.98], // Lagos-Ibadan Expressway band (Lagos → Sagamu → Ibadan)
]
function inFreightCorridor(lat: number, lon: number): boolean {
  for (const b of FREIGHT_ZONES) if (inBbox(lat, lon, b)) return true
  return false
}
const FREIGHT_CLASSES: ReadonlySet<number> = new Set([0, 1, 10, 11]) // motorway/trunk + links

// ── AADT defaults by OSM road_class (engine inputs.rs codes) ──
const BASE_AADT: Record<number, number> = {
  0: 35000, 10: 17500, // motorway + motorway_link
  1: 12000, 11: 6000,  // trunk + trunk_link (Federal A-routes)
  2: 6000,  12: 3000,  // primary + primary_link
  3: 3000,             // secondary
  4: 1500,             // tertiary
  5: 700,              // residential
}
const NG_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

interface Split { light: number; medium: number; heavy: number; moto: number }
const TIER1: Split   = { light: 0.45, medium: 0.05, heavy: 0.15, moto: 0.35 }
const TIER2: Split   = { light: 0.45, medium: 0.06, heavy: 0.14, moto: 0.35 }
const RURAL: Split   = { light: 0.50, medium: 0.08, heavy: 0.22, moto: 0.20 }
const FREIGHT: Split = { light: 0.35, medium: 0.05, heavy: 0.45, moto: 0.15 }

function splitFor(mult: number, cls: number, freight: boolean): Split {
  if (freight && FREIGHT_CLASSES.has(cls)) return FREIGHT
  if (mult >= 2.0) return TIER1
  if (mult >= 1.4) return TIER2
  return RURAL
}

function applySplit(aadt: number, s: Split) {
  return {
    light: Math.round(aadt * s.light),
    medium: Math.round(aadt * s.medium),
    heavy: Math.round(aadt * s.heavy),
    moto: Math.round(aadt * s.moto),
  }
}

async function main() {
  console.log(`=== NG Roads Enrichment — Nigeria-tuned CNOSSOS class defaults (${YEAR}) ===\n`)

  const inNG = makeCoastalCountryGate('NG')
  const hexDirs = iterateCountryHexes(H3R4_DIR, NG_HEX_BBOX)
  console.log(`  NG-bbox hexes with roads.arrow: ${hexDirs.length}\n`)

  let totalRoads = 0, enriched = 0, hexesUpdated = 0, preserved = 0, outsideNG = 0, freightRows = 0
  const byClass: Record<number, number> = {}
  const byMult: Record<string, number> = {}
  let sumHeavy = 0, sumMoto = 0, sumAll = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        // Cheap gates first; the polygon gate (inNG) is the expensive call — last.
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        const base = BASE_AADT[row.roadClass]
        if (base === undefined) return null
        if (!inBbox(row.midLat, row.midLon, NG_HEX_BBOX)) return null
        if (!inNG(row.midLat, row.midLon)) { outsideNG++; return null }

        const mult = cityMultiplier(row.midLat, row.midLon)
        const freight = inFreightCorridor(row.midLat, row.midLon)
        const aadt = base * mult
        const s = applySplit(aadt, splitFor(mult, row.roadClass, freight))
        return { ...s, sourceId: MY_SOURCE_ID }
      },
      (row, _i, applied) => {
        enriched++
        byClass[row.roadClass] = (byClass[row.roadClass] || 0) + 1
        const m = cityMultiplier(row.midLat, row.midLon)
        const freight = inFreightCorridor(row.midLat, row.midLon) && FREIGHT_CLASSES.has(row.roadClass)
        const key = freight ? 'freight-corridor' : m === 2.5 ? 'Lagos ×2.5' : m === 2.0 ? 'Tier-1 ×2.0'
          : m === 1.4 ? 'Tier-2 ×1.4' : 'rural ×1.0'
        byMult[key] = (byMult[key] || 0) + 1
        if (freight) freightRows++
        sumHeavy += applied.heavy
        sumMoto += applied.moto
        sumAll += applied.light + applied.medium + applied.heavy + applied.moto
      },
      NG_COVERAGE,
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
  console.log(`  Outside NG polygon:      ${outsideNG.toLocaleString()}`)
  console.log(`  Enriched:                ${enriched.toLocaleString()} (${(100 * enriched / Math.max(totalRoads, 1)).toFixed(1)}%)`)
  console.log(`  Freight-corridor rows:   ${freightRows.toLocaleString()}`)
  console.log(`  Hexes updated:           ${hexesUpdated}/${hexDirs.length}`)
  console.log(`\n  By road class:`)
  for (const cls of Object.keys(byClass).map(Number).sort((a, b) => a - b)) {
    console.log(`    ${CLASS_NAME[cls] ?? cls}: ${byClass[cls].toLocaleString()}`)
  }
  console.log(`\n  By tier:`)
  for (const [k, v] of Object.entries(byMult).sort((a, b) => b[1] - a[1])) {
    console.log(`    ${k}: ${v.toLocaleString()}`)
  }
  console.log(`\n  Mean heavy share: ${(100 * sumHeavy / Math.max(sumAll, 1)).toFixed(1)}%, mean moto share: ${(100 * sumMoto / Math.max(sumAll, 1)).toFixed(1)}%`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
