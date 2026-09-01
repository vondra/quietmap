/**
 * Enrich PE roads.arrow with MTC Provías Red Vial + dIMD + Peruvian CNOSSOS.
 *
 * Sources:
 *   - **Red Vial Nacional 2024 MTC PROVIAS** (community mirror `mincetur_osdn_2`):
 *       services6.arcgis.com/G8JFnqCHKQ9vb8YW/.../Red_Vial_Nacional_2024_MTC_PROVIAS
 *     7,340 polylines with rich attributes:
 *       cCodRuta (e.g. PE-1N, PE-1S, PE-3N, PE-22)
 *       cNomRuta (full route name)
 *       cClasifica:
 *         LONGITUDINAL DE LA COSTA — Panamericana N/S (618 segments)
 *         LONGITUDINAL DE LA SIERRA — Andean spine (980)
 *         LONGITUDINAL DE LA SELVA — Amazon spine (486)
 *         TRANSVERSAL — east-west cross routes (2,269)
 *         RAMAL — branches (2,255)
 *         VARIANTE — alternate routes (732)
 *       cRegion — COSTA (1,544) / SIERRA (4,459) / SELVA (1,337)
 *       cSuperfici — surface code (1/11=paved primary, 2/4=unpaved, 9=other)
 *       cPeajes — toll concession flag ('Concesión' for 1,244 segments)
 *       dIMD — Índice Medio Diario (AADT) — **866 segments have non-zero values**
 *       dNroCarril — number of lanes (0/1/2/3/4/5/6/7)
 *       dAncCalzad — calzada width
 *       dVelProTra — average speed
 *
 *   - **Red Vial Departamental** (`semillero_admin` mirror):
 *       services.arcgis.com/gafQrINhKg5HqHyr/.../Red_Vial_Departamental
 *     3,150 provincial road polylines (no IMD)
 *
 * ## Peruvian AADT defaults (fallback when no IMD match)
 *
 *   | OSM class | Costa | Sierra | Selva | Tier-1 (×2.0) | Tier-2 (×1.4) |
 *   |---|---:|---:|---:|---:|---:|
 *   | 0 motorway (autopista, Panam toll) | 35,000 | 15,000 | 8,000 | 70,000 | 49,000 |
 *   | 1 trunk (Panam / LONGITUDINAL)      | 12,000 |  5,000 | 2,500 | 24,000 | 16,800 |
 *   | 2 primary (TRANSVERSAL paved)        |  6,000 |  2,500 | 1,200 | 12,000 |  8,400 |
 *   | 3 secondary                          |  3,000 |  1,500 |   800 |  6,000 |  4,200 |
 *   | 4 tertiary                           |  1,500 |    700 |   400 |  3,000 |  2,100 |
 *   | 5 residential                        |    700 |    400 |   200 |  1,400 |    980 |
 *
 * ## MTC Red Vial spatial-match AADT (by classification + peaje)
 *
 *   Concesión + LONGITUDINAL DE LA COSTA (Panam toll)     → 20,000
 *   Concesión + other LONGITUDINAL                         → 10,000
 *   LONGITUDINAL DE LA COSTA paved (Panamericana)          → 12,000
 *   LONGITUDINAL DE LA SIERRA / DE LA SELVA paved          →  6,000
 *   TRANSVERSAL paved                                      →  4,000
 *   RAMAL / VARIANTE paved                                 →  2,500
 *   Unpaved                                                →  1,200
 *
 * ## Peruvian vehicle split
 *
 * Peru has moderate motorcycle share (~15-20% urban, less than CO/VN/BR-BR).
 * Heavy share is elevated on mining corridors (Panamericana Sur, Sierra
 * highways to mines, Carretera Central) and intercity freight.
 *
 *   Tier-1 (Lima):        light 65% / medium 6% / heavy 14% / moto 15%
 *   Tier-2:               light 68% / medium 6% / heavy 12% / moto 14%
 *   Costa rural:          light 60% / medium 8% / heavy 22% / moto 10%
 *   Sierra (Andes):       light 50% / medium 10% / heavy 30% / moto 10%
 *   Selva (Amazon):       light 60% / medium 8% / heavy 22% / moto 10%
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-pe.ts
 */

import { readFileSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_PE_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_PE_NATIONAL_ROADS

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/pe`)

const PE_BBOX: [number, number, number, number] = [-18.4, -82.0, -0.0, -68.5]

const EXCLUDE_ZONES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Ecuador', bbox: [-5.0, -82.0, 1.5, -75.0] },
  { name: 'Colombia', bbox: [-4.3, -75.0, 1.5, -66.5] },
  { name: 'Brazil', bbox: [-18.4, -75.0, -1.0, -66.5] },
  { name: 'Bolivia', bbox: [-22.9, -70.0, -9.5, -57.5] },
  { name: 'Chile', bbox: [-22.0, -75.0, -17.5, -68.5] },
]

// Tier-1: Lima/Callao (only)
const TIER1_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Lima / Callao', bbox: [-12.30, -77.20, -11.80, -76.80] },
]

// Tier-2 cities (×1.4)
const TIER2_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Arequipa',    bbox: [-16.50, -71.65, -16.30, -71.45] },
  { name: 'Trujillo',    bbox: [-8.20, -79.10, -8.00, -78.95] },
  { name: 'Chiclayo',    bbox: [-6.82, -79.92, -6.70, -79.78] },
  { name: 'Piura',       bbox: [-5.25, -80.68, -5.15, -80.58] },
  { name: 'Iquitos',     bbox: [-3.80, -73.30, -3.70, -73.20] },
  { name: 'Cusco',       bbox: [-13.60, -72.00, -13.45, -71.85] },
  { name: 'Chimbote',    bbox: [-9.15, -78.60, -9.05, -78.50] },
  { name: 'Huancayo',    bbox: [-12.10, -75.25, -12.00, -75.15] },
  { name: 'Tacna',       bbox: [-18.05, -70.30, -17.95, -70.20] },
  { name: 'Juliaca',     bbox: [-15.55, -70.15, -15.45, -70.05] },
  { name: 'Ica',         bbox: [-14.15, -75.80, -14.00, -75.65] },
  { name: 'Cajamarca',   bbox: [-7.20, -78.55, -7.10, -78.45] },
  { name: 'Pucallpa',    bbox: [-8.45, -74.60, -8.35, -74.50] },
  { name: 'Sullana',     bbox: [-4.92, -80.72, -4.85, -80.65] },
  { name: 'Ayacucho',    bbox: [-13.20, -74.27, -13.13, -74.20] },
  { name: 'Chincha Alta', bbox: [-13.47, -76.15, -13.40, -76.08] },
  { name: 'Huánuco',     bbox: [-9.98, -76.27, -9.90, -76.20] },
  { name: 'Tarapoto',    bbox: [-6.52, -76.40, -6.45, -76.33] },
  { name: 'Puno',        bbox: [-15.85, -70.05, -15.78, -69.98] },
  { name: 'Tumbes',      bbox: [-3.60, -80.48, -3.53, -80.42] },
  { name: 'Huaraz',      bbox: [-9.56, -77.56, -9.48, -77.50] },
  { name: 'Jaén',        bbox: [-5.73, -78.82, -5.67, -78.78] },
  { name: 'Huacho',      bbox: [-11.12, -77.63, -11.05, -77.57] },
  { name: 'Pisco',       bbox: [-13.73, -76.22, -13.68, -76.17] },
]

// Mining corridor (Moquegua/Tacna/Arequipa SPCC Toquepala/Cuajone/Ilo)
const MINING_COASTAL_BBOX: [number, number, number, number] = [-18.4, -73.0, -15.5, -70.0]

// Sierra mining regions (Ancash, Cajamarca, Apurímac, Cusco highlands)
const MINING_SIERRA_BBOXES: Array<[number, number, number, number]> = [
  [-11.0, -77.8, -8.5, -76.0],   // Ancash (Antamina, Cerro de Pasco)
  [-8.0, -79.0, -6.0, -77.5],    // Cajamarca (Yanacocha)
  [-15.0, -73.5, -13.0, -71.5],  // Apurímac/Cusco (Las Bambas, Antapaccay, Constancia)
]

function inAnyZone(lat: number, lon: number): boolean {
  for (const z of EXCLUDE_ZONES) if (inBbox(lat, lon, z.bbox)) return true
  return false
}
function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  for (const c of TIER1_CITIES) if (inBbox(lat, lon, c.bbox)) return 1
  for (const c of TIER2_CITIES) if (inBbox(lat, lon, c.bbox)) return 2
  return 0
}
function inMiningRegion(lat: number, lon: number): boolean {
  if (inBbox(lat, lon, MINING_COASTAL_BBOX)) return true
  for (const b of MINING_SIERRA_BBOXES) if (inBbox(lat, lon, b)) return true
  return false
}

// Rough region classification for class defaults
function peRegion(lat: number, lon: number): 'costa' | 'sierra' | 'selva' {
  // Selva: east of ~-76 in north, east of ~-73 in south, plus whole NE quadrant
  if (lon > -73) return 'selva'
  if (lon > -75 && lat < -8) return 'sierra'
  if (lon > -76 && lat > -4) return 'selva'
  // Costa: west of ~-77.5, south of ~-4
  if (lon < -77.5 && lat < -4) return 'costa'
  // Default to sierra for highlands
  if (lon >= -77.5 && lon <= -73 && lat < -4) return 'sierra'
  return 'costa'
}

interface MtcFeat {
  coords: [number, number][]
  dIMD: number
  clasifica: string
  superficie: string
  peaje: string
}

function loadMtcRoads(): MtcFeat[] {
  const path = resolve(CACHE_DIR, 'roads-nacional-mtc.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: MtcFeat[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g) continue
    let coords: [number, number][] = []
    if (g.type === 'LineString') coords = g.coordinates
    else if (g.type === 'MultiLineString') {
      for (const part of g.coordinates) coords.push(...part)
    }
    if (coords.length < 2) continue
    const p = f.properties || {}
    out.push({
      coords,
      dIMD: typeof p.dIMD === 'number' ? p.dIMD : 0,
      clasifica: (p.cClasifica || '').toString().trim(),
      superficie: (p.cSuperfici || '').toString().trim(),
      peaje: (p.cPeajes || '').toString().trim(),
    })
  }
  return out
}

function loadDepartamental(): MtcFeat[] {
  const path = resolve(CACHE_DIR, 'roads-departamental.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: MtcFeat[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g) continue
    let coords: [number, number][] = []
    if (g.type === 'LineString') coords = g.coordinates
    else if (g.type === 'MultiLineString') {
      for (const part of g.coordinates) coords.push(...part)
    }
    if (coords.length < 2) continue
    const p = f.properties || {}
    out.push({
      coords,
      dIMD: 0,
      clasifica: 'DEPARTAMENTAL',
      superficie: (p.SUPERFIC || '').toString().trim(),
      peaje: '',
    })
  }
  return out
}

function buildGrid(features: MtcFeat[]): Map<string, MtcFeat[]> {
  const grid = new Map<string, MtcFeat[]>()
  for (const feat of features) {
    const seen = new Set<string>()
    for (const [lon, lat] of feat.coords) {
      const key = `${Math.floor(lat * 100)}_${Math.floor(lon * 100)}`
      if (!seen.has(key)) {
        seen.add(key)
        if (!grid.has(key)) grid.set(key, [])
        grid.get(key)!.push(feat)
      }
    }
  }
  return grid
}

function nearestRoad(midLat: number, midLon: number, grid: Map<string, MtcFeat[]>, radiusM: number): MtcFeat | null {
  const reach = Math.max(1, Math.ceil(radiusM / 1000))
  const baseLat = Math.floor(midLat * 100)
  const baseLon = Math.floor(midLon * 100)
  let best: MtcFeat | null = null
  let bestDist = radiusM
  for (let dy = -reach; dy <= reach; dy++) {
    for (let dx = -reach; dx <= reach; dx++) {
      const cell = grid.get(`${baseLat + dy}_${baseLon + dx}`)
      if (!cell) continue
      for (const feat of cell) {
        const d = pointToPolylineDist(midLat, midLon, feat.coords)
        if (d < bestDist) { bestDist = d; best = feat }
      }
    }
  }
  return best
}

const CLASS_AADT_COSTA: Record<number, number> = {
  0: 35000, 1: 12000, 2: 6000, 3: 3000, 4: 1500, 5: 700, 6: 300,
}
const CLASS_AADT_SIERRA: Record<number, number> = {
  0: 15000, 1: 5000, 2: 2500, 3: 1500, 4: 700, 5: 400, 6: 200,
}
const CLASS_AADT_SELVA: Record<number, number> = {
  0: 8000, 1: 2500, 2: 1200, 3: 800, 4: 400, 5: 200, 6: 100,
}

function mtcAadt(feat: MtcFeat): number {
  // Use dIMD when available
  if (feat.dIMD > 0) return feat.dIMD
  const paved = feat.superficie === '1' || feat.superficie === '11'
  if (!paved) return 1200
  const concession = /concesi/i.test(feat.peaje)
  if (concession && /LONGITUDINAL DE LA COSTA/i.test(feat.clasifica)) return 20000
  if (concession) return 10000
  if (/LONGITUDINAL DE LA COSTA/i.test(feat.clasifica)) return 12000
  if (/LONGITUDINAL DE LA (SIERRA|SELVA)/i.test(feat.clasifica)) return 6000
  if (/TRANSVERSAL/i.test(feat.clasifica)) return 4000
  if (/RAMAL|VARIANTE|DEPARTAMENTAL/i.test(feat.clasifica)) return 2500
  return 2000
}

function tierMultiplier(tier: 0 | 1 | 2): number {
  return tier === 1 ? 2.0 : tier === 2 ? 1.4 : 1.0
}

function splitVehicles(aadt: number, tier: 0 | 1 | 2, region: string, mining: boolean): { light: number; medium: number; heavy: number; moto: number } {
  if (tier === 1) {
    return {
      light: Math.round(aadt * 0.65),
      medium: Math.round(aadt * 0.06),
      heavy: Math.round(aadt * 0.14),
      moto: Math.round(aadt * 0.15),
    }
  }
  if (tier === 2) {
    return {
      light: Math.round(aadt * 0.68),
      medium: Math.round(aadt * 0.06),
      heavy: Math.round(aadt * 0.12),
      moto: Math.round(aadt * 0.14),
    }
  }
  if (mining) {
    // Mining corridor — extreme heavy share
    return {
      light: Math.round(aadt * 0.45),
      medium: Math.round(aadt * 0.08),
      heavy: Math.round(aadt * 0.38),
      moto: Math.round(aadt * 0.09),
    }
  }
  if (region === 'sierra') {
    return {
      light: Math.round(aadt * 0.50),
      medium: Math.round(aadt * 0.10),
      heavy: Math.round(aadt * 0.30),
      moto: Math.round(aadt * 0.10),
    }
  }
  // Costa / Selva rural
  return {
    light: Math.round(aadt * 0.60),
    medium: Math.round(aadt * 0.08),
    heavy: Math.round(aadt * 0.22),
    moto: Math.round(aadt * 0.10),
  }
}

async function main() {
  console.log(`=== PE Roads Enrichment — MTC Red Vial + dIMD + Peruvian CNOSSOS (${YEAR}) ===\n`)

  const mtcRoads = loadMtcRoads()
  const withImd = mtcRoads.filter(r => r.dIMD > 0).length
  console.log(`  MTC Red Vial Nacional loaded: ${mtcRoads.length}`)
  console.log(`    with dIMD (real AADT):      ${withImd}`)

  const departamental = loadDepartamental()
  console.log(`  Red Vial Departamental:       ${departamental.length}`)

  const allRoads = [...mtcRoads, ...departamental]
  const grid = buildGrid(allRoads)
  console.log(`  Grid cells: ${grid.size}\n`)

  const hexDirs = iterateCountryHexes(H3R4_DIR, PE_BBOX)
  console.log(`  PE-bbox hexes with roads.arrow: ${hexDirs.length}`)

  let totalRoads = 0, excluded = 0, alreadyEnriched = 0
  let matchedImd = 0, matchedRvn = 0, matchedClass = 0
  let hexesUpdated = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        // Fast-exit before the expensive grid match when a higher-priority dataset
        // already owns the row (writeRoadAadt re-checks the gate — this only saves work).
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { alreadyEnriched++; return null }

        const midLat = row.midLat
        const midLon = row.midLon

        if (!inBbox(midLat, midLon, PE_BBOX)) return null
        if (inAnyZone(midLat, midLon)) { excluded++; return null }

        const tier = cityTier(midLat, midLon)
        const mult = tierMultiplier(tier)
        const cls = row.roadClass
        const region = peRegion(midLat, midLon)
        const mining = inMiningRegion(midLat, midLon)

        let aadt = 0
        // 1. MTC/Red Vial spatial match (only class ≤ 2)
        if (cls <= 2) {
          const near = nearestRoad(midLat, midLon, grid, 500)
          if (near) {
            if (near.dIMD > 0) {
              aadt = near.dIMD * mult
              matchedImd++
            } else {
              aadt = mtcAadt(near) * mult
              matchedRvn++
            }
          }
        }
        // 2. Peruvian regional class default
        if (aadt === 0) return null  // A.1: unmatched → source_id=0 → engine country-tier cascade

        const split = splitVehicles(aadt, tier, region, mining && tier === 0)
        // A small-but-real dIMD (raw MTC GeoJSON float, no integer floor) can round
        // every class to zero: the mining split's dominant share is only 45% (<50%),
        // so aadt=1 alone gives light=round(0.45)=0 across all four classes. The
        // #31.4 writer guard rejects an all-zero payload under this MEASURED id —
        // skip rather than fabricate a "surveyed as zero" claim.
        if (split.light + split.medium + split.heavy + split.moto === 0) return null
        return {
          light: split.light, medium: split.medium,
          heavy: split.heavy, moto: split.moto, sourceId: MY_SOURCE_ID,
        }
      },
    )
    totalRoads += r.rows
    if (r.updated) hexesUpdated++

    const elapsed = Date.now() - startTime
    if (elapsed > 10_000 && hi % 50 === 0) {
      console.log(`  [${(elapsed / 1000).toFixed(0)}s] ${hi + 1}/${hexDirs.length}, ${matchedImd} IMD + ${matchedRvn} RVN + ${matchedClass} class`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  Total roads scanned:        ${totalRoads.toLocaleString()}`)
  console.log(`  Already enriched (skip):    ${alreadyEnriched.toLocaleString()}`)
  console.log(`  Excluded (neighbours):      ${excluded.toLocaleString()}`)
  console.log(`  Matched by dIMD (real):     ${matchedImd.toLocaleString()}`)
  console.log(`  Matched by MTC class:       ${matchedRvn.toLocaleString()}`)
  console.log(`  Matched by class default:   ${matchedClass.toLocaleString()}`)
  const tot = matchedImd + matchedRvn + matchedClass
  console.log(`  Total enriched:             ${tot.toLocaleString()} (${(100 * tot / Math.max(totalRoads, 1)).toFixed(2)}%)`)
  console.log(`  Hexes updated:              ${hexesUpdated}/${hexDirs.length}`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
