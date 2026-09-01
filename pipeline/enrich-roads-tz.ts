/**
 * Enrich TZ roads.arrow with Tanzania-tuned CNOSSOS class defaults.
 *
 * Tanzania publishes ZERO open per-segment AADT. TANROADS (trunk/regional) and
 * TARURA (urban/district) expose only WordPress/HTML annual reports and static
 * PDF maps — no machine-readable volume data, no ArcGIS/WFS count service. The
 * National Bureau of Statistics (nbs.go.tz) road-transport tables are national
 * vehicle-registration totals, not per-road counts. So — like the immediate
 * neighbour KE, plus EG/CN/IN/RU — we apply country-tuned class defaults rather
 * than a per-segment join.
 *
 * The defaults are anchored to Tanzania's real geography, not guessed:
 *
 * ## AADT by OSM class (two-way veh/day)
 *
 * | class            | rural baseline | Tier-1 ×2.0 | Tier-2 ×1.4 |
 * |------------------|---------------:|------------:|------------:|
 * | 0 motorway       | 25,000         | 50,000      | 35,000      |
 * | 1 trunk (T-route)| 10,000         | 20,000      | 14,000      |
 * | 2 primary        |  5,000         | 10,000      |  7,000      |
 * | 3 secondary      |  2,500         |  5,000      |  3,500      |
 * | 4 tertiary       |  1,200         |  2,400      |  1,680      |
 * | 5 residential    |    600         |  1,200      |    840      |
 *
 * Links (10/11/12) get ~half the parent-class flow (ramp traffic). Motorway-class
 * roads are rare in TZ (Mbezi Beach Expressway / short Dar dual-carriageways); the
 * national T-route network (TANZAM T5, T1/T2 to Dodoma, T3 to Namanga) is mostly
 * trunk class.
 *
 * ## City multipliers
 *   Dar es Salaam (~7 M, the economic capital and by far the largest city — Dodoma
 *   is the political capital since 1996 but a fraction of the size) ×2.0 (Tier-1).
 *   19 regional centres ×1.4 (Tier-2). No per-city count is published; tiers
 *   calibrated by eye to population + corridor role.
 *
 * ## Vehicle split — by REGION (heavy share dominates acoustically)
 *   Tanzania's urban transport runs on daladalas (shared minibuses → CNOSSOS
 *   "medium", bus-like profile) and boda-bodas (motorcycle taxis → "moto", ~25 %
 *   of urban traffic) — the same matatu/boda mix as Kenya. The TANZAM Highway (T5,
 *   Dar↔Mbeya↔Tunduma) is the trucking spine carrying Zambian/DRC Copperbelt
 *   copper and cobalt to Dar es Salaam port, parallel to the TAZARA railway —
 *   ~40 % heavy, worth 3-8 dB over the corridor that matters most.
 *     tier-1 (Dar es Salaam)    light .50 / med .15 / heavy .10 / moto .25
 *     tier-2 cities             light .52 / med .13 / heavy .12 / moto .23
 *     TANZAM corridor (T5)      light .42 / med .08 / heavy .40 / moto .10
 *     rural                     light .55 / med .10 / heavy .20 / moto .15
 *
 * Sources: NBS 2022 census populations, TANROADS trunk-road network map, TANZAM/
 * TAZARA copper-corridor freight context, NEMC Noise & Vibration Regs 2011 context.
 * Full list in data/enrichment/2026/tz/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-tz.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_TZ_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCoastalCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_TZ_NATIONAL_ROADS


// TZ coarse bbox [minLat,minLon,maxLat,maxLon] — cheap pre-scan; the
// makeCoastalCountryGate('TZ') polygon is the real filter, so this box only needs to
// enclose Tanzania (mainland + Zanzibar/Pemba). Overlaps KE/UG/RW/BI/CD/ZM/MW/MZ,
// all gated out by the polygon.
const TZ_HEX_BBOX: [number, number, number, number] = [-11.8, 29.3, -0.9, 40.5]

// ── City tiers (AADT multiplier vs rural baseline) ──
interface City { name: string; lat: number; lon: number; tier: 1 | 2; half: number }

const CITIES: City[] = [
  // Tier 1 ×2.0 — wide box covers the Dar metro + immediate satellite sprawl.
  { name: 'Dar es Salaam', lat: -6.82, lon: 39.28, tier: 1, half: 0.30 },
  // Tier 2 ×1.4 — regional capitals + major centres (NBS 2022 census).
  { name: 'Dodoma',        lat: -6.173, lon: 35.742, tier: 2, half: 0.12 },
  { name: 'Mwanza',        lat: -2.517, lon: 32.900, tier: 2, half: 0.10 },
  { name: 'Arusha',        lat: -3.387, lon: 36.683, tier: 2, half: 0.10 },
  { name: 'Mbeya',         lat: -8.909, lon: 33.460, tier: 2, half: 0.10 },
  { name: 'Morogoro',      lat: -6.821, lon: 37.661, tier: 2, half: 0.08 },
  { name: 'Tanga',         lat: -5.069, lon: 39.099, tier: 2, half: 0.08 },
  { name: 'Kahama',        lat: -3.838, lon: 32.600, tier: 2, half: 0.07 },
  { name: 'Tabora',        lat: -5.017, lon: 32.800, tier: 2, half: 0.08 },
  { name: 'Zanzibar City', lat: -6.165, lon: 39.199, tier: 2, half: 0.07 },
  { name: 'Kigoma',        lat: -4.877, lon: 29.626, tier: 2, half: 0.07 },
  { name: 'Sumbawanga',    lat: -7.967, lon: 31.617, tier: 2, half: 0.07 },
  { name: 'Kasulu',        lat: -4.578, lon: 30.103, tier: 2, half: 0.06 },
  { name: 'Songea',        lat: -10.683, lon: 35.650, tier: 2, half: 0.07 },
  { name: 'Musoma',        lat: -1.500, lon: 33.800, tier: 2, half: 0.07 },
  { name: 'Iringa',        lat: -7.770, lon: 35.691, tier: 2, half: 0.07 },
  { name: 'Singida',       lat: -4.817, lon: 34.745, tier: 2, half: 0.07 },
  { name: 'Shinyanga',     lat: -3.661, lon: 33.421, tier: 2, half: 0.07 },
  { name: 'Moshi',         lat: -3.350, lon: 37.333, tier: 2, half: 0.07 },
  { name: 'Bukoba',        lat: -1.332, lon: 31.812, tier: 2, half: 0.07 },
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.tier
  }
  return 0
}

// ── TANZAM Highway corridor (T5) [lon,lat] — high HGV share (~40%) ──
// Dar es Salaam ↔ Chalinze ↔ Morogoro ↔ Mikumi ↔ Iringa ↔ Mbeya ↔ Tunduma (Zambia
// border), parallel to the TAZARA railway. Carries Zambian/DRC Copperbelt copper &
// cobalt to Dar es Salaam port and import freight inland — East Africa's main
// southern trucking spine. ~900 km, much of it empty highland between towns.
const TANZAM_CORRIDOR: [number, number][] = [
  [39.28, -6.82],  // Dar es Salaam
  [38.35, -6.64],  // Chalinze
  [37.661, -6.821], // Morogoro
  [37.00, -7.40],  // Mikumi
  [35.691, -7.770], // Iringa
  [34.55, -8.30],  // Makambako
  [33.460, -8.909], // Mbeya
  [32.77, -9.30],  // Tunduma (Zambia border)
]

function nearTanzamCorridor(lat: number, lon: number): boolean {
  return pointToPolylineDist(lat, lon, TANZAM_CORRIDOR) <= 15000
}

// ── Region → vehicle split (fractions sum to 1.0) ──
type Region = 'tier1' | 'tier2' | 'corridor' | 'rural'
interface Split { light: number; medium: number; heavy: number; moto: number }
const SPLITS: Record<Region, Split> = {
  tier1:    { light: 0.50, medium: 0.15, heavy: 0.10, moto: 0.25 },
  tier2:    { light: 0.52, medium: 0.13, heavy: 0.12, moto: 0.23 },
  corridor: { light: 0.42, medium: 0.08, heavy: 0.40, moto: 0.10 },
  rural:    { light: 0.55, medium: 0.10, heavy: 0.20, moto: 0.15 },
}

/** Region for the vehicle split: cities first (urban daladala/boda mix dominates
 *  the metro surface streets), then the TANZAM corridor on its long rural stretches,
 *  then generic rural. */
function region(lat: number, lon: number, tier: 0 | 1 | 2): Region {
  if (tier === 1) return 'tier1'
  if (tier === 2) return 'tier2'
  if (nearTanzamCorridor(lat, lon)) return 'corridor'
  return 'rural'
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes) ──
const BASE_AADT: Record<number, number> = {
  0: 25000, 10: 12500, // motorway + motorway_link (rare: Dar expressways)
  1: 10000, 11: 5000,  // trunk + trunk_link (T-routes)
  2: 5000,  12: 2500,  // primary + primary_link
  3: 2500,             // secondary
  4: 1200,             // tertiary
  5: 600,              // residential
}
const TZ_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

function splitVehicles(aadt: number, s: Split) {
  return {
    light: Math.round(aadt * s.light),
    medium: Math.round(aadt * s.medium),
    heavy: Math.round(aadt * s.heavy),
    moto: Math.round(aadt * s.moto),
  }
}

async function main() {
  console.log(`=== TZ Roads Enrichment — Tanzania-tuned CNOSSOS class defaults (${YEAR}) ===\n`)

  // Polygon gate stops TZ AADT bleeding onto KE/UG/RW/BI/CD/ZM/MW/MZ roads the
  // rectangular bbox overlaps.
  const inTZ = makeCoastalCountryGate('TZ')

  const hexDirs = iterateCountryHexes(H3R4_DIR, TZ_HEX_BBOX)
  console.log(`  TZ-bbox hexes with roads.arrow: ${hexDirs.length}\n`)

  let totalRoads = 0, enriched = 0, hexesUpdated = 0, preserved = 0, outsideTZ = 0
  const byClass: Record<number, number> = {}
  const byRegion: Record<string, number> = {}
  let sumHeavy = 0, sumAll = 0, sumMoto = 0, sumWithMoto = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        // Cheap gates first; the polygon gate (inTZ) is the expensive call — last.
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        const base = BASE_AADT[row.roadClass]
        if (base === undefined) return null
        if (!inBbox(row.midLat, row.midLon, TZ_HEX_BBOX)) return null
        if (!inTZ(row.midLat, row.midLon)) { outsideTZ++; return null }

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
      TZ_COVERAGE,
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
  console.log(`  Outside TZ polygon:      ${outsideTZ.toLocaleString()}`)
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
  console.log(`\n  Mean heavy share (light+med+heavy): ${(100 * sumHeavy / Math.max(sumAll, 1)).toFixed(1)}%  (10% metro .. 40% TANZAM corridor)`)
  console.log(`  Mean moto share (incl. moto):       ${(100 * sumMoto / Math.max(sumWithMoto, 1)).toFixed(1)}%  (high — boda-boda taxis)`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
