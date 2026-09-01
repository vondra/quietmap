/**
 * Enrich EC roads.arrow with CONGOPE Red Vial + Ecuadorian CNOSSOS.
 *
 * Sources (community mirrors — MTOP/IGM/ANT gov domains TCP-blocked):
 *   - **Red Vial Estatal** (`cniama_congope` owner):
 *       services6.arcgis.com/pYn2F4v1aESZqj1u/.../Red_Vial_Estatal/FeatureServer/0
 *     711 state-level polylines with CLASIFICAC (ARTERIAL 457 / COLECTORA 254),
 *     ESTADO (BUENO/REGULAR/CIRCULE CON PRECAUCIÓN/MUY BUENO)
 *
 *   - **Red Vial Ecuador** (`lerazo_congope` owner — CONGOPE Consorcio de
 *     Gobiernos Provinciales):
 *       services6.arcgis.com/pYn2F4v1aESZqj1u/.../Red_Vial_Ecuador/FeatureServer/0
 *     28,328 polylines covering national + 24 GAD provincial networks
 *     Fields: PROVINCIA, ADMINISTRA (directa MTOP vs GAD provincial),
 *             TIPO_VIA, TIPO_CALZA, ANCHO_CAL, ESTADO
 *
 *   - **ANT 2030 RVE subset** (`2YA05qh4jhRDhuXH` mirror):
 *       343 polylines with CLASE_VIA, TIPO_DE_SU, NUMERO_DE (lanes)
 *
 * **Ecuador publishes NO per-segment IMD/AADT in machine-readable form.**
 * MTOP/ANT gov portals are TCP-blocked. Rely on class-based defaults plus
 * CONGOPE classification (ARTERIAL/COLECTORA).
 *
 * ## Ecuadorian AADT defaults (Costa/Sierra/Oriente regional variation)
 *
 *   | OSM class | Costa | Sierra | Oriente (Amazon) | Tier-1 (×2.0) | Tier-2 (×1.4) |
 *   |---|---:|---:|---:|---:|---:|
 *   | 0 motorway | 30,000 | 20,000 | 6,000 | 60,000 | 42,000 |
 *   | 1 trunk    | 12,000 |  8,000 | 3,000 | 24,000 | 16,800 |
 *   | 2 primary  |  6,000 |  4,000 | 1,500 | 12,000 |  8,400 |
 *   | 3 secondary|  3,000 |  2,000 |   800 |  6,000 |  4,200 |
 *   | 4 tertiary |  1,500 |  1,000 |   400 |  3,000 |  2,100 |
 *   | 5 residential| 700  |    500 |   200 |  1,400 |    980 |
 *
 * ## CONGOPE Red Vial spatial-match AADT
 *
 *   ARTERIAL paved  → 15,000 (national backbones: E25 Panamericana, E15 coastal)
 *   COLECTORA paved →  6,000 (provincial collectors)
 *   ARTERIAL/COLECTORA unpaved → 2,000
 *   GAD provincial paved (Red Vial Ecuador) → 4,000
 *   Unpaved GAD → 1,200
 *
 * ## Ecuadorian vehicle split
 *
 * Ecuador has moderate motorcycle share (~15-25% urban). Heavy share elevated
 * on oil freight routes (E45 Amazon corridor to Esmeraldas port) and
 * Panamericana (bananas/flowers to Guayaquil).
 *
 *   Tier-1 (Quito/Guayaquil): light 67% / medium 6% / heavy 12% / moto 15%
 *   Tier-2:                   light 68% / medium 6% / heavy 12% / moto 14%
 *   Costa rural:              light 60% / medium 8% / heavy 22% / moto 10%
 *   Sierra:                   light 55% / medium 8% / heavy 27% / moto 10%
 *   Oriente (Amazon):         light 50% / medium 8% / heavy 32% / moto 10%
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-ec.ts
 */

import { readFileSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_EC_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_EC_NATIONAL_ROADS

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/ec`)

// Ecuador bbox — mainland only (Galápagos excluded)
const EC_BBOX: [number, number, number, number] = [-5.0, -81.1, 1.5, -75.0]

// Hex-scan bbox = the same Ecuador mainland box, so iterateCountryHexes skips the
// rest of the planet instead of reading every roads.arrow on Earth. [minLat,minLon,maxLat,maxLon]
const EC_HEX_BBOX: [number, number, number, number] = [-5.0, -81.1, 1.5, -75.0]

const EXCLUDE_ZONES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Colombia', bbox: [0.5, -79.5, 1.5, -75.0] },
  { name: 'Peru', bbox: [-18.4, -81.5, -4.9, -69.0] },
]

// Tier-1 cities (×2.0) — Quito and Guayaquil
const TIER1_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Quito D.M.', bbox: [-0.40, -78.60, -0.05, -78.40] },
  { name: 'Guayaquil', bbox: [-2.35, -80.00, -2.05, -79.75] },
]

// Tier-2 cities (×1.4)
const TIER2_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Cuenca',          bbox: [-2.95, -79.05, -2.83, -78.93] },
  { name: 'Santo Domingo',   bbox: [-0.30, -79.22, -0.20, -79.12] },
  { name: 'Machala',         bbox: [-3.30, -79.98, -3.22, -79.88] },
  { name: 'Durán',           bbox: [-2.22, -79.90, -2.12, -79.80] },
  { name: 'Manta',           bbox: [-0.98, -80.76, -0.90, -80.67] },
  { name: 'Portoviejo',      bbox: [-1.08, -80.48, -1.00, -80.40] },
  { name: 'Ambato',          bbox: [-1.30, -78.64, -1.22, -78.58] },
  { name: 'Loja',            bbox: [-4.05, -79.22, -3.97, -79.17] },
  { name: 'Riobamba',        bbox: [-1.70, -78.68, -1.63, -78.62] },
  { name: 'Esmeraldas',      bbox: [0.94, -79.70, 1.00, -79.63] },
  { name: 'Ibarra',          bbox: [0.32, -78.14, 0.38, -78.08] },
  { name: 'Latacunga',       bbox: [-0.97, -78.63, -0.91, -78.58] },
  { name: 'Milagro',         bbox: [-2.15, -79.63, -2.08, -79.56] },
  { name: 'Babahoyo',        bbox: [-1.83, -79.55, -1.77, -79.48] },
  { name: 'Quevedo',         bbox: [-1.04, -79.50, -0.97, -79.44] },
  { name: 'Lago Agrio',      bbox: [0.07, -76.92, 0.13, -76.86] },
  { name: 'Tulcán',          bbox: [0.79, -77.74, 0.84, -77.70] },
]

// Oil freight corridor (Amazon → SOTE → OCP → Esmeraldas/Balao)
const OIL_ROUTE_BBOXES: Array<[number, number, number, number]> = [
  [0.0, -77.5, 1.0, -76.0],   // Sucumbíos oil fields
  [-0.5, -78.0, 0.5, -77.5],  // SOTE corridor
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
function inOilRoute(lat: number, lon: number): boolean {
  for (const b of OIL_ROUTE_BBOXES) if (inBbox(lat, lon, b)) return true
  return false
}

// EC tri-regional classifier
function ecRegion(lat: number, lon: number): 'costa' | 'sierra' | 'oriente' {
  // Oriente: east of ~-78 (Amazon basin)
  if (lon > -78.0) return 'oriente'
  // Sierra: -78.5 to -79 longitude in the Andes spine, roughly
  if (lon > -79.5 && lon < -77.8) return 'sierra'
  // Costa: west of -79.5
  return 'costa'
}

// Local to EC: census `feat.coords` always has ≥2 vertices (loadRoads drops shorter),
// so this matches the shared spatial.pointToPolylineDist on every call site here.

interface EcFeat {
  coords: [number, number][]
  clasif: string
  tipo: string
  administra: string
  isState: boolean
}

function loadRoads(file: string, isState: boolean, clasifField: string, tipoField: string): EcFeat[] {
  const path = resolve(CACHE_DIR, file)
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: EcFeat[] = []
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
      clasif: (p[clasifField] || '').toString().trim(),
      tipo: (p[tipoField] || '').toString().trim(),
      administra: (p.ADMINISTRA || '').toString().trim(),
      isState,
    })
  }
  return out
}

function buildGrid(features: EcFeat[]): Map<string, EcFeat[]> {
  const grid = new Map<string, EcFeat[]>()
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

function nearestRoad(midLat: number, midLon: number, grid: Map<string, EcFeat[]>, radiusM: number): EcFeat | null {
  const reach = Math.max(1, Math.ceil(radiusM / 1000))
  const baseLat = Math.floor(midLat * 100)
  const baseLon = Math.floor(midLon * 100)
  let best: EcFeat | null = null
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
  0: 30000, 1: 12000, 2: 6000, 3: 3000, 4: 1500, 5: 700, 6: 300,
}
const CLASS_AADT_SIERRA: Record<number, number> = {
  0: 20000, 1: 8000, 2: 4000, 3: 2000, 4: 1000, 5: 500, 6: 250,
}
const CLASS_AADT_ORIENTE: Record<number, number> = {
  0: 6000, 1: 3000, 2: 1500, 3: 800, 4: 400, 5: 200, 6: 100,
}

function ecAadt(feat: EcFeat): number {
  const tipo = (feat.tipo || '').toUpperCase()
  const paved = /ASFAL|PAVI|HORMIG|HORMIGON/i.test(tipo) || tipo === ''  // empty = assume paved
  const unpaved = /LASTRE|TIERRA|EMPEDR|LASTRA/i.test(tipo)
  if (unpaved) return feat.isState ? 2000 : 1200
  if (feat.isState) {
    const clasif = (feat.clasif || '').toUpperCase()
    if (/ARTERIAL/.test(clasif)) return 15000
    if (/COLECTORA/.test(clasif)) return 6000
    return 10000  // state road no classification
  }
  // Provincial GAD
  return paved ? 4000 : 1200
}

function tierMultiplier(tier: 0 | 1 | 2): number {
  return tier === 1 ? 2.0 : tier === 2 ? 1.4 : 1.0
}

function splitVehicles(aadt: number, tier: 0 | 1 | 2, region: string, oilRoute: boolean): { light: number; medium: number; heavy: number; moto: number } {
  if (tier === 1) {
    return {
      light: Math.round(aadt * 0.67),
      medium: Math.round(aadt * 0.06),
      heavy: Math.round(aadt * 0.12),
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
  if (oilRoute) {
    // Oil freight route — extreme heavy share
    return {
      light: Math.round(aadt * 0.45),
      medium: Math.round(aadt * 0.08),
      heavy: Math.round(aadt * 0.37),
      moto: Math.round(aadt * 0.10),
    }
  }
  if (region === 'sierra') {
    return {
      light: Math.round(aadt * 0.55),
      medium: Math.round(aadt * 0.08),
      heavy: Math.round(aadt * 0.27),
      moto: Math.round(aadt * 0.10),
    }
  }
  if (region === 'oriente') {
    return {
      light: Math.round(aadt * 0.50),
      medium: Math.round(aadt * 0.08),
      heavy: Math.round(aadt * 0.32),
      moto: Math.round(aadt * 0.10),
    }
  }
  // Costa rural
  return {
    light: Math.round(aadt * 0.60),
    medium: Math.round(aadt * 0.08),
    heavy: Math.round(aadt * 0.22),
    moto: Math.round(aadt * 0.10),
  }
}

async function main() {
  console.log(`=== EC Roads Enrichment — CONGOPE Red Vial + Ecuadorian CNOSSOS (${YEAR}) ===\n`)

  const stateRoads = loadRoads('roads-estatal.geojson', true, 'CLASIFICAC', '')
  console.log(`  Red Vial Estatal loaded:       ${stateRoads.length}`)

  const ant2030 = loadRoads('roads-ant2030.geojson', true, 'CLASE_VIA', 'TIPO_DE_SU')
  console.log(`  ANT 2030 RVE subset:           ${ant2030.length}`)

  const allRoads = loadRoads('roads-all.geojson', false, 'TIPO_VIA', 'TIPO_CALZA')
  console.log(`  Red Vial Ecuador (comprehensive): ${allRoads.length}`)

  const combined = [...stateRoads, ...ant2030, ...allRoads]
  const grid = buildGrid(combined)
  console.log(`  Grid cells: ${grid.size}\n`)

  const hexDirs = iterateCountryHexes(H3R4_DIR, EC_HEX_BBOX)
  console.log(`  EC-bbox hexes with roads.arrow: ${hexDirs.length}`)

  let totalRoads = 0, excluded = 0, alreadyEnriched = 0
  let matchedCongope = 0
  const matchedClass = 0
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

        if (!inBbox(midLat, midLon, EC_BBOX)) return null
        if (inAnyZone(midLat, midLon)) { excluded++; return null }

        const tier = cityTier(midLat, midLon)
        const mult = tierMultiplier(tier)
        const cls = row.roadClass
        const region = ecRegion(midLat, midLon)
        const oil = inOilRoute(midLat, midLon)

        let aadt = 0
        // 1. CONGOPE Red Vial spatial match (only class ≤ 2)
        if (cls <= 2) {
          const near = nearestRoad(midLat, midLon, grid, 400)
          if (near) {
            aadt = ecAadt(near) * mult
          }
        }
        // 2. Ecuadorian tri-regional class default
        if (aadt === 0) return null  // A.1: unmatched → source_id=0 → engine country-tier cascade

        const split = splitVehicles(aadt, tier, region, oil)
        return {
          light: split.light, medium: split.medium,
          heavy: split.heavy, moto: split.moto, sourceId: MY_SOURCE_ID,
        }
      },
      () => { matchedCongope++ },
    )
    totalRoads += r.rows
    if (r.updated) hexesUpdated++

    const elapsed = Date.now() - startTime
    if (elapsed > 10_000 && hi % 50 === 0) {
      console.log(`  [${(elapsed / 1000).toFixed(0)}s] ${hi + 1}/${hexDirs.length}, ${matchedCongope} CONGOPE + ${matchedClass} class`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  Total roads scanned:        ${totalRoads.toLocaleString()}`)
  console.log(`  Already enriched (skip):    ${alreadyEnriched.toLocaleString()}`)
  console.log(`  Excluded (neighbours):      ${excluded.toLocaleString()}`)
  console.log(`  Matched by CONGOPE:         ${matchedCongope.toLocaleString()}`)
  console.log(`  Matched by class default:   ${matchedClass.toLocaleString()}`)
  const tot = matchedCongope + matchedClass
  console.log(`  Total enriched:             ${tot.toLocaleString()} (${(100 * tot / Math.max(totalRoads, 1)).toFixed(2)}%)`)
  console.log(`  Hexes updated:              ${hexesUpdated}/${hexDirs.length}`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
