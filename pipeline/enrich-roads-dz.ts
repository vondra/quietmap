/**
 * Enrich DZ roads.arrow with Algeria-tuned CNOSSOS class defaults.
 *
 * Algeria publishes ZERO open per-segment AADT: the Ministère des Travaux
 * Publics (MTP), Agence de Développement de l'Autoroute (ADA/ANA) and Wilaya
 * portals are corporate HTML only — no machine-readable counts, no open road
 * network. So — like EG/CN/IN/RU — we apply country-tuned class defaults over
 * the OSM geometry, gated to Algerian soil by the country polygon, not a
 * per-segment join.
 *
 * The defaults are anchored to Algeria's real geography, not guessed:
 *
 * ## AADT by OSM class (two-way veh/day)
 *
 * | class            | rural baseline | Tier-1 ×2.0 | Tier-2 ×1.4 |
 * |------------------|---------------:|------------:|------------:|
 * | 0 motorway       | 35,000         | 70,000      | 49,000      |
 * | 1 trunk          | 12,000         | 24,000      | 16,800      |
 * | 2 primary        |  6,000         | 12,000      |  8,400      |
 * | 3 secondary      |  3,000         |  6,000      |  4,200      |
 * | 4 tertiary       |  1,500         |  3,000      |  2,100      |
 * | 5 residential    |    700         |  1,400      |    980      |
 *
 * Links (10/11/12) get ~half the parent-class flow (ramp traffic).
 *
 * ## City multipliers
 *   Grand Alger ×2.0 (Tier-1) — the ~4 M Algiers metro (Africa's 4th largest
 *   metropolitan area: Algiers + Bab Ezzouar + El Harrach + Dar El Beïda). 27
 *   Tier-2 wilaya capitals + port/oil cities ×1.4. Algeria is Mediterranean in
 *   traffic pattern — higher baseline than sub-Saharan Africa, lower motorcycle
 *   share than Nigeria/Egypt. No per-city scalar is published; tiers calibrated
 *   by eye to city population + the Autoroute Est-Ouest's known coastal loading.
 *
 * ## Vehicle split — by REGION, not just class (heavy share dominates acoustically)
 *   Algeria's mix swings hard by region: the coastal Autoroute Est-Ouest is
 *   car-dominated (~17% heavy) while the Hassi R'Mel/Messaoud oil-gas corridor
 *   runs ~52% heavy (tanker + oilfield-logistics trucks). Getting heavy share
 *   right is worth 3-8 dB on the corridors that matter.
 *     tier-1 (Grand Alger)   light .66 / med .12 / heavy .13 / moto .09
 *     tier-2 cities          light .66 / med .10 / heavy .16 / moto .08
 *     Autoroute Est-Ouest    light .74 / med .05 / heavy .17 / moto .04
 *     oil-gas corridor       light .38 / med .05 / heavy .52 / moto .05
 *     rural (RN network)     light .58 / med .07 / heavy .29 / moto .06
 *
 * Sources: ONS population, Autoroute Est-Ouest (A1, ~1,216 km) + Trans-Sahara
 * RN1/RN3 freight geography, Sonatrach oil-gas field locations. Full derivation
 * in data/enrichment/2026/dz/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-dz.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_DZ_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCoastalCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_DZ_NATIONAL_ROADS


// DZ coarse bbox [minLat,minLon,maxLat,maxLon] — cheap pre-scan over neighbours
// (Morocco/Tunisia/Libya/Niger/Mali/Mauritania); makeCoastalCountryGate('DZ') is the real
// filter, so this box only needs to enclose Algeria incl. the deep Sahara south.
const DZ_HEX_BBOX: [number, number, number, number] = [18.9, -8.7, 37.1, 12.0]

// ── City tiers (AADT multiplier vs rural baseline) ──
interface City { name: string; lat: number; lon: number; tier: 1 | 2; half: number }

const CITIES: City[] = [
  // Tier 1 ×2.0 — box wide enough to cover the Algiers conurbation (Bab Ezzouar,
  // El Harrach, Dar El Beïda, Hussein Dey).
  { name: 'Grand Alger', lat: 36.75, lon: 3.06, tier: 1, half: 0.25 },
  // Tier 2 ×1.4 — wilaya capitals + ports + Sahara oil/gas hubs.
  { name: 'Oran',            lat: 35.70, lon: -0.63, tier: 2, half: 0.14 },
  { name: 'Constantine',     lat: 36.36, lon: 6.61,  tier: 2, half: 0.12 },
  { name: 'Annaba',          lat: 36.90, lon: 7.77,  tier: 2, half: 0.10 },
  { name: 'Blida',           lat: 36.47, lon: 2.83,  tier: 2, half: 0.10 },
  { name: 'Batna',           lat: 35.55, lon: 6.17,  tier: 2, half: 0.10 },
  { name: 'Djelfa',          lat: 34.67, lon: 3.26,  tier: 2, half: 0.10 },
  { name: 'Setif',           lat: 36.19, lon: 5.41,  tier: 2, half: 0.10 },
  { name: 'Sidi Bel Abbes',  lat: 35.19, lon: -0.63, tier: 2, half: 0.10 },
  { name: 'Biskra',          lat: 34.85, lon: 5.73,  tier: 2, half: 0.10 },
  { name: 'Tebessa',         lat: 35.40, lon: 8.12,  tier: 2, half: 0.10 },
  { name: 'Tlemcen',         lat: 34.88, lon: -1.32, tier: 2, half: 0.10 },
  { name: 'Bejaia',          lat: 36.75, lon: 5.08,  tier: 2, half: 0.10 },
  { name: 'Tiaret',          lat: 35.37, lon: 1.32,  tier: 2, half: 0.10 },
  { name: 'Bechar',          lat: 31.62, lon: -2.22, tier: 2, half: 0.10 },
  { name: 'Skikda',          lat: 36.88, lon: 6.91,  tier: 2, half: 0.10 },
  { name: 'Chlef',           lat: 36.17, lon: 1.33,  tier: 2, half: 0.10 },
  { name: 'Mostaganem',      lat: 35.93, lon: 0.09,  tier: 2, half: 0.10 },
  { name: 'Ouargla',         lat: 31.95, lon: 5.32,  tier: 2, half: 0.10 },
  { name: 'Ghardaia',        lat: 32.48, lon: 3.67,  tier: 2, half: 0.10 },
  { name: 'Laghouat',        lat: 33.80, lon: 2.88,  tier: 2, half: 0.10 },
  { name: 'Hassi Messaoud',  lat: 31.68, lon: 6.07,  tier: 2, half: 0.10 },
  { name: "Hassi R'Mel",     lat: 32.93, lon: 3.27,  tier: 2, half: 0.10 },
  { name: 'Adrar',           lat: 27.87, lon: -0.29, tier: 2, half: 0.10 },
  { name: 'Tamanrasset',     lat: 22.79, lon: 5.52,  tier: 2, half: 0.10 },
  { name: 'Tizi Ouzou',      lat: 36.72, lon: 4.05,  tier: 2, half: 0.10 },
  { name: 'El Oued',         lat: 33.37, lon: 6.86,  tier: 2, half: 0.10 },
  { name: 'Boumerdes',       lat: 36.77, lon: 3.48,  tier: 2, half: 0.10 },
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.tier
  }
  return 0
}

// ── Corridor polylines [lon,lat] — matched by min distance to the whole line ──
// Hassi R'Mel gas hub → Ghardaïa → Ouargla → Hassi Messaoud oil capital → In
// Amenas: the RN3 + oilfield-logistics spine carrying tanker/heavy freight.
const OILGAS_CORRIDOR: [number, number][] = [
  [3.27, 32.93], [3.67, 32.48], [5.32, 31.95], [6.07, 31.68], [9.55, 28.05],
]
// Autoroute Est-Ouest (A1, ~1,216 km): Tunisia border ↔ Annaba ↔ Constantine ↔
// Sétif ↔ Algiers ↔ Blida ↔ Chlef ↔ Oran ↔ Tlemcen ↔ Morocco border. Coastal,
// car-dominated — lower heavy share than the interior RN network.
const AUTOROUTE_EW: [number, number][] = [
  [8.24, 36.78], [7.77, 36.90], [6.61, 36.36], [5.41, 36.19], [3.48, 36.77],
  [3.06, 36.75], [2.83, 36.47], [1.33, 36.17], [-0.63, 35.70], [-1.32, 34.88], [-1.75, 34.80],
]

const nearOilGas = (lat: number, lon: number) => pointToPolylineDist(lat, lon, OILGAS_CORRIDOR) <= 25000
const nearAutoroute = (lat: number, lon: number) => pointToPolylineDist(lat, lon, AUTOROUTE_EW) <= 12000

// ── Region → vehicle split (fractions sum to 1.0) ──
type Region = 'tier1' | 'tier2' | 'autoroute' | 'oilgas' | 'rural'
interface Split { light: number; medium: number; heavy: number; moto: number }
const SPLITS: Record<Region, Split> = {
  tier1:     { light: 0.66, medium: 0.12, heavy: 0.13, moto: 0.09 },
  tier2:     { light: 0.66, medium: 0.10, heavy: 0.16, moto: 0.08 },
  autoroute: { light: 0.74, medium: 0.05, heavy: 0.17, moto: 0.04 },
  oilgas:    { light: 0.38, medium: 0.05, heavy: 0.52, moto: 0.05 },
  rural:     { light: 0.58, medium: 0.07, heavy: 0.29, moto: 0.06 },
}

/** Region for the vehicle split: cities first, then the oil-gas freight corridor
 *  (deep south), then the coastal Autoroute Est-Ouest, else the rural RN network. */
function region(lat: number, lon: number, tier: 0 | 1 | 2): Region {
  if (tier === 1) return 'tier1'
  if (tier === 2) return 'tier2'
  if (nearOilGas(lat, lon)) return 'oilgas'
  if (nearAutoroute(lat, lon)) return 'autoroute'
  return 'rural'
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes) ──
const BASE_AADT: Record<number, number> = {
  0: 35000, 10: 17500, // motorway + motorway_link
  1: 12000, 11: 6000,  // trunk + trunk_link
  2: 6000,  12: 3000,  // primary + primary_link
  3: 3000,             // secondary
  4: 1500,             // tertiary
  5: 700,              // residential
}
const DZ_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

function splitVehicles(aadt: number, s: Split) {
  return {
    light: Math.round(aadt * s.light),
    medium: Math.round(aadt * s.medium),
    heavy: Math.round(aadt * s.heavy),
    moto: Math.round(aadt * s.moto),
  }
}

async function main() {
  console.log(`=== DZ Roads Enrichment — Algeria-tuned CNOSSOS class defaults (${YEAR}) ===\n`)

  // makeCoastalCountryGate may download+convert the CGAZ boundary on a fresh host. The
  // polygon gate stops DZ AADT bleeding onto Moroccan/Tunisian/Libyan/Sahelian
  // roads the rectangular bbox overlaps.
  const inDZ = makeCoastalCountryGate('DZ')

  const hexDirs = iterateCountryHexes(H3R4_DIR, DZ_HEX_BBOX)
  console.log(`  DZ-bbox hexes with roads.arrow: ${hexDirs.length}\n`)

  let totalRoads = 0, enriched = 0, hexesUpdated = 0, preserved = 0, outsideDZ = 0
  const byClass: Record<number, number> = {}
  const byRegion: Record<string, number> = {}
  let sumHeavy = 0, sumAll = 0, sumMoto = 0, sumWithMoto = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        // Cheap gates first; the polygon gate (inDZ) is the expensive call — last.
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        const base = BASE_AADT[row.roadClass]
        if (base === undefined) return null
        if (!inBbox(row.midLat, row.midLon, DZ_HEX_BBOX)) return null
        if (!inDZ(row.midLat, row.midLon)) { outsideDZ++; return null }

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
      DZ_COVERAGE,
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
  console.log(`  Outside DZ polygon:      ${outsideDZ.toLocaleString()}`)
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
  console.log(`\n  Mean heavy share (light+med+heavy): ${(100 * sumHeavy / Math.max(sumAll, 1)).toFixed(1)}%  (13% Grand Alger .. 52% oil-gas corridor)`)
  console.log(`  Mean moto share (incl. moto):       ${(100 * sumMoto / Math.max(sumWithMoto, 1)).toFixed(1)}%  (low — Mediterranean mix, not sub-Saharan)`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
