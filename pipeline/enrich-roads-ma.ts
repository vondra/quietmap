/**
 * Enrich MA roads.arrow with Morocco-tuned CNOSSOS class defaults.
 *
 * Morocco publishes ZERO open per-segment AADT. Every gov portal is dead or
 * paywalled (MTPNET / Équipement — connection refused; ADM toll operator — no
 * API; data.gov.ma — Drupal/PDF, no GIS). ADM runs ~1,800 km of tolled
 * autoroutes (Africa's longest network) yet publishes no toll-booth counts. So
 * — like DZ/EG/TR — we apply country-tuned class defaults over the OSM geometry,
 * gated to Moroccan soil by the CGAZ country polygon, not a per-segment join.
 *
 * The gate is MA ∪ EH: it includes the Western Sahara provinces Morocco
 * administers (Laâyoune, Dakhla), matching the coverage of enrich-industrial-ma.ts
 * — CGAZ codes Western Sahara as a separate ESH feature, so an MA-only gate would
 * silently drop the whole Atlantic south.
 *
 * The defaults are anchored to Morocco's real geography, not guessed:
 *
 * ## AADT by OSM class (two-way veh/day)
 *
 * | class            | rural baseline | Tier-1 ×2.0 | Tier-2 ×1.4 |
 * |------------------|---------------:|------------:|------------:|
 * | 0 motorway       | 30,000         | 60,000      | 42,000      |
 * | 1 trunk          | 12,000         | 24,000      | 16,800      |
 * | 2 primary        |  6,000         | 12,000      |  8,400      |
 * | 3 secondary      |  3,000         |  6,000      |  4,200      |
 * | 4 tertiary       |  1,500         |  3,000      |  2,100      |
 * | 5 residential    |    700         |  1,400      |    980      |
 *
 * Motorway baseline is 30,000 (vs DZ's 35,000): MA autoroutes are tolled, so
 * heavy freight diverts to the free RN trunk network — lower mainline flow than
 * Algeria's toll-free A1. Links (10/11/12) get ~half the parent-class flow.
 *
 * ## City multipliers
 *   Tier-1 ×2.0 (5 metros): Casablanca (~4 M, economic capital), Rabat (~1.8 M,
 *   political capital), Marrakech (tourism), Fez (ancient), Tangier (Strait +
 *   Tanger Med port). Tier-2 ×1.4 (20 cities): provincial capitals + ports. No
 *   per-city scalar is published; tiers calibrated by eye to population.
 *
 * ## Vehicle split — by REGION, not just class (heavy share dominates acoustically)
 *   The OCP phosphate triangle (Khouribga mine ↔ Jorf Lasfar / Safi export ports
 *   — the world's largest phosphate operation, ~40 Mtpa) runs ~35% heavy on its
 *   haul roads. Moroccan cities carry a high moped/scooter share (~15-17% moto).
 *     tier-1 (5 metros)      light .65 / med .06 / heavy .12 / moto .17
 *     tier-2 cities          light .65 / med .08 / heavy .12 / moto .15
 *     phosphate haul roads   light .50 / med .08 / heavy .35 / moto .07
 *     rural (RN network)     light .62 / med .08 / heavy .20 / moto .10
 *   The phosphate split applies to the rural haul roads (tier 0) only — city
 *   cores keep their tier split so urban residential streets aren't tagged 35%
 *   truck (same city-wins-over-corridor precedence as enrich-roads-dz.ts).
 *
 * Sources: HCP population, OCP phosphate corridor geography (Khouribga ↔ Jorf
 * Lasfar ↔ Benguerir/Youssoufia ↔ Safi), ADM autoroute network. Full derivation
 * in data/enrichment/2026/ma/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-ma.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_MA_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCoastalCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_MA_NATIONAL_ROADS


// MA coarse bbox [minLat,minLon,maxLat,maxLon] — cheap pre-scan over neighbours
// (Algeria/Mauritania/Spain). Encloses Morocco proper (to Tangier ~35.9 N) plus
// the Western Sahara provinces south to ~20.8 N. The MA ∪ EH polygon gate is the
// real filter; this box only needs to enclose the union.
const MA_HEX_BBOX: [number, number, number, number] = [20.7, -17.3, 36.1, -0.9]

// ── City tiers (AADT multiplier vs rural baseline) ──
interface City { name: string; lat: number; lon: number; tier: 1 | 2; half: number }

const CITIES: City[] = [
  // Tier 1 ×2.0 — the 5 metros. Boxes wide enough to cover the conurbations
  // (Casablanca + Mohammedia, Rabat + Salé + Témara).
  { name: 'Casablanca', lat: 33.57, lon: -7.59, tier: 1, half: 0.22 },
  { name: 'Rabat-Salé',  lat: 34.02, lon: -6.82, tier: 1, half: 0.18 },
  { name: 'Marrakech',   lat: 31.63, lon: -8.01, tier: 1, half: 0.16 },
  { name: 'Fez',         lat: 34.04, lon: -5.00, tier: 1, half: 0.14 },
  { name: 'Tangier',     lat: 35.77, lon: -5.80, tier: 1, half: 0.16 },
  // Tier 2 ×1.4 — provincial capitals + ports (20 cities).
  { name: 'Meknes',      lat: 33.90, lon: -5.55, tier: 2, half: 0.11 },
  { name: 'Oujda',       lat: 34.68, lon: -1.91, tier: 2, half: 0.11 },
  { name: 'Kenitra',     lat: 34.26, lon: -6.58, tier: 2, half: 0.10 },
  { name: 'Tetouan',     lat: 35.58, lon: -5.37, tier: 2, half: 0.10 },
  { name: 'Agadir',      lat: 30.42, lon: -9.60, tier: 2, half: 0.12 },
  { name: 'Nador',       lat: 35.17, lon: -2.93, tier: 2, half: 0.10 },
  { name: 'Safi',        lat: 32.30, lon: -9.24, tier: 2, half: 0.10 },
  { name: 'El Jadida',   lat: 33.23, lon: -8.50, tier: 2, half: 0.10 },
  { name: 'Khouribga',   lat: 32.88, lon: -6.91, tier: 2, half: 0.10 },
  { name: 'Beni Mellal', lat: 32.34, lon: -6.36, tier: 2, half: 0.10 },
  { name: 'Taza',        lat: 34.21, lon: -4.01, tier: 2, half: 0.10 },
  { name: 'Khemisset',   lat: 33.82, lon: -6.07, tier: 2, half: 0.10 },
  { name: 'Laayoune',    lat: 27.15, lon: -13.20, tier: 2, half: 0.12 },
  { name: 'Mohammedia',  lat: 33.69, lon: -7.38, tier: 2, half: 0.08 },
  { name: 'Settat',      lat: 33.00, lon: -7.62, tier: 2, half: 0.10 },
  { name: 'Larache',     lat: 35.19, lon: -6.16, tier: 2, half: 0.10 },
  { name: 'Ouarzazate',  lat: 30.92, lon: -6.89, tier: 2, half: 0.10 },
  { name: 'Taourirt',    lat: 34.41, lon: -2.89, tier: 2, half: 0.10 },
  { name: 'Essaouira',   lat: 31.51, lon: -9.77, tier: 2, half: 0.10 },
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.tier
  }
  return 0
}

// ── OCP phosphate haul corridors [lon,lat] — matched by min distance to the line ──
// Khouribga (world's largest phosphate mine) feeds two export complexes by a
// slurry pipeline + rail + truck haulage. The connecting RN/RP roads carry a
// heavy-truck share found nowhere else in Morocco's interior.
//   north: Khouribga → Jorf Lasfar fertilizer port (via El Jadida)
const PHOSPHATE_JORF: [number, number][] = [
  [-6.91, 32.88], [-7.60, 33.00], [-8.50, 33.23], [-8.63, 33.13],
]
//   south: Khouribga → Benguerir → Youssoufia → Safi phosphate port
const PHOSPHATE_SAFI: [number, number][] = [
  [-6.91, 32.88], [-7.95, 32.24], [-8.53, 32.25], [-9.24, 32.30],
]
const nearPhosphate = (lat: number, lon: number) =>
  pointToPolylineDist(lat, lon, PHOSPHATE_JORF) <= 15000 ||
  pointToPolylineDist(lat, lon, PHOSPHATE_SAFI) <= 15000

// ── Region → vehicle split (fractions sum to 1.0) ──
type Region = 'tier1' | 'tier2' | 'phosphate' | 'rural'
interface Split { light: number; medium: number; heavy: number; moto: number }
const SPLITS: Record<Region, Split> = {
  tier1:     { light: 0.65, medium: 0.06, heavy: 0.12, moto: 0.17 },
  tier2:     { light: 0.65, medium: 0.08, heavy: 0.12, moto: 0.15 },
  phosphate: { light: 0.50, medium: 0.08, heavy: 0.35, moto: 0.07 },
  rural:     { light: 0.62, medium: 0.08, heavy: 0.20, moto: 0.10 },
}

/** Region for the vehicle split: cities win first (so urban streets aren't tagged
 *  35% truck), then the rural phosphate haul roads, else the rural RN network. */
function region(lat: number, lon: number, tier: 0 | 1 | 2): Region {
  if (tier === 1) return 'tier1'
  if (tier === 2) return 'tier2'
  if (nearPhosphate(lat, lon)) return 'phosphate'
  return 'rural'
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes) ──
const BASE_AADT: Record<number, number> = {
  0: 30000, 10: 15000, // motorway + motorway_link
  1: 12000, 11: 6000,  // trunk + trunk_link
  2: 6000,  12: 3000,  // primary + primary_link
  3: 3000,             // secondary
  4: 1500,             // tertiary
  5: 700,              // residential
}
const MA_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

function splitVehicles(aadt: number, s: Split) {
  return {
    light: Math.round(aadt * s.light),
    medium: Math.round(aadt * s.medium),
    heavy: Math.round(aadt * s.heavy),
    moto: Math.round(aadt * s.moto),
  }
}

async function main() {
  console.log(`=== MA Roads Enrichment — Morocco-tuned CNOSSOS class defaults (${YEAR}) ===\n`)

  // makeCoastalCountryGate may download+convert the CGAZ boundary on a fresh host. The
  // MA ∪ EH polygon gate stops MA AADT bleeding onto Algerian/Mauritanian/Spanish
  // roads the rectangular bbox overlaps, while still covering Western Sahara
  // (a separate ESH polygon) the way enrich-industrial-ma.ts does.
  const inMA = makeCoastalCountryGate('MA')
  const inEH = makeCoastalCountryGate('EH')
  const inCountry = (lat: number, lon: number) => inMA(lat, lon) || inEH(lat, lon)

  const hexDirs = iterateCountryHexes(H3R4_DIR, MA_HEX_BBOX)
  console.log(`  MA-bbox hexes with roads.arrow: ${hexDirs.length}\n`)

  let totalRoads = 0, enriched = 0, hexesUpdated = 0, preserved = 0, outsideMA = 0
  const byClass: Record<number, number> = {}
  const byRegion: Record<string, number> = {}
  let sumHeavy = 0, sumAll = 0, sumMoto = 0, sumWithMoto = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        // Cheap gates first; the polygon gate (inCountry) is the expensive call — last.
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        const base = BASE_AADT[row.roadClass]
        if (base === undefined) return null
        if (!inBbox(row.midLat, row.midLon, MA_HEX_BBOX)) return null
        if (!inCountry(row.midLat, row.midLon)) { outsideMA++; return null }

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
      MA_COVERAGE,
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
  console.log(`  Outside MA∪EH polygon:   ${outsideMA.toLocaleString()}`)
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
  console.log(`\n  Mean heavy share (light+med+heavy): ${(100 * sumHeavy / Math.max(sumAll, 1)).toFixed(1)}%  (12% cities .. 35% phosphate corridor)`)
  console.log(`  Mean moto share (incl. moto):       ${(100 * sumMoto / Math.max(sumWithMoto, 1)).toFixed(1)}%  (high — Moroccan cities are moped-heavy)`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
