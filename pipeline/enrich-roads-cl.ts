/**
 * Enrich CL roads.arrow with MOP Vialidad Red Vial + TMDA 2024 + Chilean CNOSSOS.
 *
 * Sources:
 *   - **Red_Vial Nacional** via Esri Chile mirror
 *     `services.arcgis.com/r7t1P5pnkoOLRdhr/.../Red_Vial/FeatureServer/0`
 *     13,962 polylines from MOP Dirección de Vialidad
 *     Fields: ROL, CLASIFICACION, CARPETA, CONCESIONADO, KM_I, KM_F, NOMBRE_CAMINO
 *     Classes:
 *       Camino Nacional Longitudinal (Ruta 5 Pan-American backbone)
 *       Camino Nacional / Camino Nacional con Carácter de Internacional
 *       Camino Regional Principal/Provincial/Comunal/Acceso
 *       Camino Privado / Camino Indígena
 *
 *   - **Plan Nacional de Censos Viales (TMDA)** via MOP MapServer
 *     `rest-sit.mop.gob.cl/.../Plan_Nacional_de_Censos/MapServer/0`
 *     863 census points with REAL AADT values, 2024 data (FRESHEST in pipeline)
 *     Fields: ESTACION_DE_CONTROL, TMDA_RAMA_1..4, ROL_RAMA_1..4, REGION, ANO, LINK
 *     Top values verified: Talca 93,752, Ruta 5 Peñuelas 32,647, Rancagua 35,384
 *
 * ## Chilean AADT defaults (fallback when no TMDA match)
 *
 *   | OSM class | Rural | Tier-1 (×2.0) | Tier-2 (×1.4) |
 *   |---|---:|---:|---:|
 *   | 0 motorway (autopista, Ruta 5)  | 50,000 | 100,000 | 70,000 |
 *   | 1 trunk (Camino Nacional)       | 18,000 |  36,000 | 25,200 |
 *   | 2 primary                       |  9,000 |  18,000 | 12,600 |
 *   | 3 secondary                     |  4,000 |   8,000 |  5,600 |
 *   | 4 tertiary                      |  1,800 |   3,600 |  2,520 |
 *   | 5 residential                   |    800 |   1,600 |  1,120 |
 *
 * ## Red Vial spatial-match AADT (by classification + carpeta + concesion)
 *
 *   Concesionado + paved (Autopista Central/Costanera/Ruta 5 toll) → 35,000
 *   Camino Nacional Longitudinal (Ruta 5 backbone) + paved        → 22,000
 *   Camino Nacional + paved                                       → 14,000
 *   Camino Regional Principal + paved                             → 7,000
 *   Camino Regional Provincial + paved                            → 3,500
 *   Unpaved (Ripio/Tierra)                                        → 1,500
 *
 * ## Chilean vehicle split
 *
 * Chile is dominated by Pampean truck freight (similar to AR) but with
 * additional mining truck traffic in the north (Antofagasta/Tarapacá)
 * and forestry truck traffic in the south. Motorcycle share is moderate.
 *
 *   Tier-1 (Santiago): light 75% / medium 10% / heavy 10% / moto 5%
 *   Tier-2:            light 73% / medium 10% / heavy 12% / moto 5%
 *   Rural:             light 60% / medium 10% / heavy 27% / moto 3%
 *   Mining-region:     light 50% / medium 10% / heavy 38% / moto 2%
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-cl.ts
 */

import { readFileSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_CL_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { flatDist, inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_CL_NATIONAL_ROADS

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/cl`)

// Chile bbox — long thin strip. [minLat,minLon,maxLat,maxLon] for iterateCountryHexes.
const CL_HEX_BBOX: [number, number, number, number] = [-56.0, -76.0, -17.5, -66.4]
// Per-row midpoint membership test reuses the same box.
const CL_BBOX = CL_HEX_BBOX

// Chile is very narrow (~180 km wide max). Border with Argentina is along
// the Andes crest, roughly at lon -67.5 in northern Chile and bending west
// toward -71 in southern Patagonia.
const EXCLUDE_ZONES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Argentina (north)', bbox: [-42.0, -67.5, -17.5, -53.0] },
  { name: 'Argentina (Patagonia)', bbox: [-56.0, -71.0, -42.0, -53.0] },
  { name: 'Bolivia', bbox: [-22.9, -68.5, -9.5, -57.5] },
  { name: 'Peru', bbox: [-17.65, -82.0, -3.0, -68.0] },
]

// Tier-1 cities (×2.0) — only Santiago (Greater)
const TIER1_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Santiago (Gran Santiago)', bbox: [-33.65, -70.85, -33.30, -70.45] },
]

// Tier-2 cities (×1.4)
const TIER2_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Valparaíso',     bbox: [-33.10, -71.65, -32.95, -71.45] },
  { name: 'Viña del Mar',   bbox: [-33.05, -71.60, -32.95, -71.45] },
  { name: 'Concepción',     bbox: [-37.00, -73.10, -36.65, -72.85] },
  { name: 'Talcahuano',     bbox: [-36.78, -73.15, -36.65, -73.00] },
  { name: 'La Serena',      bbox: [-30.05, -71.30, -29.85, -71.20] },
  { name: 'Coquimbo',       bbox: [-29.98, -71.40, -29.88, -71.30] },
  { name: 'Antofagasta',    bbox: [-23.75, -70.50, -23.55, -70.35] },
  { name: 'Iquique',        bbox: [-20.30, -70.20, -20.15, -70.10] },
  { name: 'Arica',          bbox: [-18.55, -70.35, -18.40, -70.20] },
  { name: 'Temuco',         bbox: [-38.78, -72.65, -38.65, -72.55] },
  { name: 'Rancagua',       bbox: [-34.25, -70.80, -34.10, -70.65] },
  { name: 'Talca',          bbox: [-35.50, -71.70, -35.35, -71.60] },
  { name: 'Chillán',        bbox: [-36.65, -72.15, -36.55, -72.05] },
  { name: 'Puerto Montt',   bbox: [-41.55, -73.05, -41.40, -72.90] },
  { name: 'Osorno',         bbox: [-40.65, -73.20, -40.55, -73.10] },
  { name: 'Valdivia',       bbox: [-39.90, -73.30, -39.75, -73.20] },
  { name: 'Calama',         bbox: [-22.55, -69.00, -22.40, -68.85] },
  { name: 'Copiapó',        bbox: [-27.45, -70.40, -27.30, -70.25] },
  { name: 'Punta Arenas',   bbox: [-53.20, -70.95, -53.05, -70.85] },
  { name: 'Curicó',         bbox: [-35.00, -71.30, -34.93, -71.20] },
  { name: 'Los Ángeles',    bbox: [-37.50, -72.40, -37.40, -72.30] },
  { name: 'San Antonio',    bbox: [-33.65, -71.65, -33.55, -71.55] },
  { name: 'Quillota',       bbox: [-32.93, -71.30, -32.85, -71.20] },
  { name: 'Tomé',           bbox: [-36.65, -72.97, -36.58, -72.90] },
]

// Mining region (Antofagasta/Tarapacá/Atacama Norte) — very high HGV share
const MINING_REGION_BBOX: [number, number, number, number] = [-30.0, -71.5, -17.5, -66.4]

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
  return inBbox(lat, lon, MINING_REGION_BBOX)
}

interface RedVialFeat {
  coords: [number, number][]
  rol: string
  clase: string
  carpeta: string
  concesionado: boolean
}

interface TmdaFeat {
  lat: number
  lon: number
  aadt: number
  rol: string
  region: string
}

function loadRedVial(): RedVialFeat[] {
  const path = resolve(CACHE_DIR, 'roads-network.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: RedVialFeat[] = []
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
      rol: (p.ROL || '').toString().trim(),
      clase: (p.CLASIFICACION || '').toString().trim(),
      carpeta: (p.CARPETA || '').toString().trim(),
      concesionado: /si|true|1/i.test((p.CONCESIONADO || '').toString()),
    })
  }
  return out
}

function loadTmda(): TmdaFeat[] {
  const path = resolve(CACHE_DIR, 'tmda-2024.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: TmdaFeat[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g) continue
    // Census points may be Point or MultiPoint or LineString
    let lat: number, lon: number
    if (g.type === 'Point') {
      ;[lon, lat] = g.coordinates
    } else if (g.type === 'MultiPoint' || g.type === 'LineString') {
      ;[lon, lat] = g.coordinates[0]
    } else if (g.type === 'MultiLineString') {
      ;[lon, lat] = g.coordinates[0][0]
    } else continue
    if (lat == null || lon == null) continue
    const p = f.properties || {}
    // Take max of TMDA_RAMA_1..4 (different ramps at the station)
    const tmdas = [p.TMDA_RAMA_1, p.TMDA_RAMA_2, p.TMDA_RAMA_3, p.TMDA_RAMA_4]
      .map(v => (typeof v === 'number' ? v : parseInt(String(v || 0), 10) || 0))
    const aadt = Math.max(...tmdas)
    if (aadt < 50) continue
    const rol = [p.ROL_RAMA_1, p.ROL_RAMA_2].filter(Boolean).join('/')
    out.push({
      lat, lon, aadt,
      rol: rol.toString(),
      region: (p.REGION || '').toString(),
    })
  }
  return out
}

function buildLineGrid(features: RedVialFeat[]): Map<string, RedVialFeat[]> {
  const grid = new Map<string, RedVialFeat[]>()
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

function buildPointGrid(features: TmdaFeat[]): Map<string, TmdaFeat[]> {
  const grid = new Map<string, TmdaFeat[]>()
  for (const feat of features) {
    const key = `${Math.floor(feat.lat * 100)}_${Math.floor(feat.lon * 100)}`
    if (!grid.has(key)) grid.set(key, [])
    grid.get(key)!.push(feat)
  }
  return grid
}

function nearestRedVial(midLat: number, midLon: number, grid: Map<string, RedVialFeat[]>, radiusM: number): RedVialFeat | null {
  const reach = Math.max(1, Math.ceil(radiusM / 1000))
  const baseLat = Math.floor(midLat * 100)
  const baseLon = Math.floor(midLon * 100)
  let best: RedVialFeat | null = null
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

function nearestTmda(midLat: number, midLon: number, grid: Map<string, TmdaFeat[]>, radiusM: number): TmdaFeat | null {
  const reach = Math.max(1, Math.ceil(radiusM / 1000))
  const baseLat = Math.floor(midLat * 100)
  const baseLon = Math.floor(midLon * 100)
  let best: TmdaFeat | null = null
  let bestDist = radiusM
  for (let dy = -reach; dy <= reach; dy++) {
    for (let dx = -reach; dx <= reach; dx++) {
      const cell = grid.get(`${baseLat + dy}_${baseLon + dx}`)
      if (!cell) continue
      for (const feat of cell) {
        const d = flatDist(midLat, midLon, feat.lat, feat.lon)
        if (d < bestDist) { bestDist = d; best = feat }
      }
    }
  }
  return best
}

function redVialAadt(feat: RedVialFeat): number {
  const paved = /asfalto|hormigon|pavi|pav/i.test(feat.carpeta)
  if (!paved) return 1500
  if (feat.concesionado) return 35000
  if (/longitudinal/i.test(feat.clase)) return 22000  // Ruta 5 Pan-American
  if (/nacional/i.test(feat.clase)) return 14000
  if (/principal/i.test(feat.clase)) return 7000
  if (/provincial/i.test(feat.clase)) return 3500
  return 2000  // comunal/acceso/privado
}

function tierMultiplier(tier: 0 | 1 | 2): number {
  return tier === 1 ? 2.0 : tier === 2 ? 1.4 : 1.0
}

function splitVehicles(aadt: number, tier: 0 | 1 | 2, mining: boolean): { light: number; medium: number; heavy: number; moto: number } {
  if (tier === 1) {
    return {
      light: Math.round(aadt * 0.75),
      medium: Math.round(aadt * 0.10),
      heavy: Math.round(aadt * 0.10),
      moto: Math.round(aadt * 0.05),
    }
  }
  if (tier === 2) {
    return {
      light: Math.round(aadt * 0.73),
      medium: Math.round(aadt * 0.10),
      heavy: Math.round(aadt * 0.12),
      moto: Math.round(aadt * 0.05),
    }
  }
  if (mining) {
    // Mining region (Antofagasta/Tarapacá/Atacama) — extreme HGV share
    return {
      light: Math.round(aadt * 0.50),
      medium: Math.round(aadt * 0.10),
      heavy: Math.round(aadt * 0.38),
      moto: Math.round(aadt * 0.02),
    }
  }
  // Rural — high heavy share for grain/forestry/freight
  return {
    light: Math.round(aadt * 0.60),
    medium: Math.round(aadt * 0.10),
    heavy: Math.round(aadt * 0.27),
    moto: Math.round(aadt * 0.03),
  }
}

async function main() {
  console.log(`=== CL Roads Enrichment — MOP Red Vial + TMDA 2024 + Chilean CNOSSOS (${YEAR}) ===\n`)

  const redVial = loadRedVial()
  console.log(`  Red Vial loaded: ${redVial.length}`)

  const tmda = loadTmda()
  console.log(`  TMDA 2024 loaded: ${tmda.length} census points with real AADT`)

  const redVialGrid = buildLineGrid(redVial)
  const tmdaGrid = buildPointGrid(tmda)
  console.log(`  Red Vial grid cells: ${redVialGrid.size}`)
  console.log(`  TMDA grid cells: ${tmdaGrid.size}\n`)

  const hexDirs = iterateCountryHexes(H3R4_DIR, CL_HEX_BBOX)
  console.log(`  CL-bbox hexes with roads.arrow: ${hexDirs.length}`)

  let totalRoads = 0, excluded = 0, alreadyEnriched = 0
  let matchedTmda = 0, matchedRedVial = 0, matchedClass = 0
  let hexesUpdated = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        // Both census datasets stamp the single CL national-roads provenance id, so
        // gating + returned sourceId use the same MY_SOURCE_ID.
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { alreadyEnriched++; return null }

        const midLat = row.midLat
        const midLon = row.midLon

        if (!inBbox(midLat, midLon, CL_BBOX)) return null
        if (inAnyZone(midLat, midLon)) { excluded++; return null }

        const tier = cityTier(midLat, midLon)
        const mult = tierMultiplier(tier)
        const cls = row.roadClass
        const mining = inMiningRegion(midLat, midLon)

        let aadt = 0
        // 1. TMDA real AADT (best — only for class ≤ 2; point station, 600 m cap)
        if (cls <= 2) {
          const t = nearestTmda(midLat, midLon, tmdaGrid, 600)
          if (t) {
            aadt = t.aadt * mult
            matchedTmda++
          }
        }
        // 2. Red Vial classification (paved/concesionado/longitudinal; polyline, 400 m cap)
        if (aadt === 0 && cls <= 2) {
          const d = nearestRedVial(midLat, midLon, redVialGrid, 400)
          if (d) {
            aadt = redVialAadt(d) * mult
            matchedRedVial++
          }
        }
        // 3. Chilean class default
        if (aadt === 0) return null  // A.1: unmatched → source_id=0 → engine country-tier cascade

        const split = splitVehicles(aadt, tier, mining && tier === 0)
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
      console.log(`  [${(elapsed / 1000).toFixed(0)}s] ${hi + 1}/${hexDirs.length}, ${matchedTmda} TMDA + ${matchedRedVial} RedVial + ${matchedClass} class`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  Total roads scanned:        ${totalRoads.toLocaleString()}`)
  console.log(`  Already enriched (skip):    ${alreadyEnriched.toLocaleString()}`)
  console.log(`  Excluded (neighbours):      ${excluded.toLocaleString()}`)
  console.log(`  Matched by TMDA (real):     ${matchedTmda.toLocaleString()}`)
  console.log(`  Matched by Red Vial class:  ${matchedRedVial.toLocaleString()}`)
  console.log(`  Matched by class default:   ${matchedClass.toLocaleString()}`)
  const tot = matchedTmda + matchedRedVial + matchedClass
  console.log(`  Total enriched:             ${tot.toLocaleString()} (${(100 * tot / Math.max(totalRoads, 1)).toFixed(2)}%)`)
  console.log(`  Hexes updated:              ${hexesUpdated}/${hexDirs.length}`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
