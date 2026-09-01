/**
 * Enrich BO roads.arrow with ABC Red Vial Fundamental + WCS + Bolivian CNOSSOS.
 *
 * Sources:
 *   - **ABC Red Vial Fundamental 2024** (community ArcGIS mirror
 *     `velasquezkevincho`):
 *       services2.arcgis.com/1GTOs4RWV6SKu0wr/.../Red_Vial_Fundamental_De_Bolivia
 *     79 polylines with `ruta`, `rodadura` (Pavimentada/Ripio/Urbano/Pav.
 *     en Construccion), `tipo` (CARRETERA/DOBLE VIA), `depto`, `de`, `a`,
 *     `longitud`
 *
 *   - **WCS RED_VIAL** (broader primary network):
 *       services.arcgis.com/x494PplYsmeeZsYB/.../RED_VIAL/FeatureServer/0
 *     556 polylines with `Nombre`, `Categoria_` (all "Primer Orden"),
 *     `Revestimie` (Asfalto 426 / Tierra 61 / blank 69)
 *
 * No per-segment TPDA/IMD published anywhere. ABC publishes only PDF.
 *
 * ## Bolivian AADT defaults (Altiplano/Valles/Llanos regional split)
 *
 *   Three regions with different traffic profiles:
 *     - Altiplano (Andean high plateau, ~70% population): Oruro, Potosí,
 *       La Paz, El Alto — dense population but low GDP per capita
 *     - Valles (Cochabamba valleys): Cochabamba, Sucre, Tarija — commercial
 *     - Llanos (eastern lowlands): Santa Cruz, Beni, Pando — tropical,
 *       agricultural/forestry, moderate density
 *
 *   | OSM class | Altiplano | Valles | Llanos | Tier-1 (×2.0) | Tier-2 (×1.4) |
 *   |---|---:|---:|---:|---:|---:|
 *   | 0 motorway | 25,000 | 20,000 | 25,000 | 50,000 | 35,000 |
 *   | 1 trunk    | 10,000 |  8,000 | 10,000 | 20,000 | 14,000 |
 *   | 2 primary  |  5,000 |  4,000 |  5,000 | 10,000 |  7,000 |
 *   | 3 secondary|  2,500 |  2,000 |  2,500 |  5,000 |  3,500 |
 *   | 4 tertiary |  1,200 |  1,000 |  1,200 |  2,400 |  1,680 |
 *   | 5 residential| 600 |    500 |    600 |  1,200 |    840 |
 *
 * ## ABC/WCS spatial-match AADT
 *
 *   Pavimentada / Asfalto Ruta Fundamental → 15,000
 *   Ripio / mixed surface                   →  5,000
 *   Tierra                                   →  2,000
 *   Urbano (inside city)                     → 12,000
 *
 * ## Bolivian vehicle split
 *
 * High motorcycle share (~20-25% urban — Bolivia has cheap Chinese imports).
 * Very high heavy-vehicle share on mining corridors (Oruro↔Antofagasta Chile,
 * Potosí↔Arica Chile, Santa Cruz↔Cochabamba soy corridor).
 *
 *   Tier-1 (La Paz/El Alto/Santa Cruz/Cochabamba): light 60% / medium 5% / heavy 10% / moto 25%
 *   Tier-2:                                          light 62% / medium 6% / heavy 12% / moto 20%
 *   Altiplano rural:                                 light 50% / medium 8% / heavy 30% / moto 12%
 *   Valles rural:                                    light 58% / medium 8% / heavy 22% / moto 12%
 *   Llanos rural (soy freight):                      light 55% / medium 8% / heavy 25% / moto 12%
 *   **Mining corridors**:                            light 45% / medium 8% / heavy 37% / moto 10%
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-bo.ts
 */

import { readFileSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { SOURCES_BY_KEY } from './lib/sources.js'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_BO_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { flatDist, inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_BO_NATIONAL_ROADS

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/bo`)

const BO_BBOX: [number, number, number, number] = [-22.9, -69.7, -9.5, -57.5]
// Same box drives the hex scan (iterateCountryHexes skips the rest of the planet)
// and the per-row midpoint re-check below. [minLat,minLon,maxLat,maxLon]
const BO_HEX_BBOX: [number, number, number, number] = BO_BBOX

const EXCLUDE_ZONES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Brazil N/NE', bbox: [-16.0, -61.0, -9.5, -57.5] },
  { name: 'Paraguay SE', bbox: [-22.9, -62.7, -19.0, -57.5] },
  { name: 'Argentina S', bbox: [-22.9, -67.5, -21.7, -60.0] },
  { name: 'Chile SW', bbox: [-22.9, -69.7, -17.5, -68.0] },
  { name: 'Peru W', bbox: [-18.4, -69.7, -9.5, -68.5] },
]

// Tier-1 cities (×2.0) — 3 metros
const TIER1_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'La Paz / El Alto', bbox: [-16.58, -68.35, -16.30, -68.00] },
  { name: 'Santa Cruz de la Sierra', bbox: [-17.90, -63.30, -17.70, -63.05] },
  { name: 'Cochabamba', bbox: [-17.45, -66.30, -17.32, -66.10] },
]

// Tier-2 cities (×1.4)
const TIER2_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Sucre',       bbox: [-19.08, -65.30, -19.00, -65.22] },
  { name: 'Oruro',       bbox: [-17.99, -67.17, -17.93, -67.10] },
  { name: 'Tarija',      bbox: [-21.55, -64.75, -21.50, -64.68] },
  { name: 'Potosí',      bbox: [-19.61, -65.78, -19.55, -65.72] },
  { name: 'Trinidad',    bbox: [-14.85, -64.93, -14.80, -64.87] },
  { name: 'Cobija',      bbox: [-11.03, -68.77, -11.00, -68.72] },
  { name: 'Riberalta',   bbox: [-11.02, -66.08, -10.97, -66.02] },
  { name: 'Montero',     bbox: [-17.37, -63.27, -17.32, -63.22] },
  { name: 'Quillacollo', bbox: [-17.42, -66.32, -17.37, -66.27] },
  { name: 'Sacaba',      bbox: [-17.42, -66.05, -17.37, -65.98] },
  { name: 'Warnes',      bbox: [-17.52, -63.19, -17.47, -63.13] },
  { name: 'Yacuiba',     bbox: [-22.03, -63.73, -21.98, -63.67] },
  { name: 'Camiri',      bbox: [-20.06, -63.54, -20.01, -63.48] },
  { name: 'Villazón',    bbox: [-22.10, -65.62, -22.05, -65.57] },
  { name: 'Viacha',      bbox: [-16.67, -68.32, -16.63, -68.27] },
  { name: 'Uyuni',       bbox: [-20.48, -66.85, -20.45, -66.80] },
]

// Mining corridors — extreme HGV share
const MINING_CORRIDORS: Array<[number, number, number, number]> = [
  // Oruro/Uyuni/Villazón (western mining spine to Chilean ports)
  [-22.5, -68.5, -17.0, -66.0],
  // Potosí → Uyuni → Chile border (via Ruta 5)
  [-21.5, -68.0, -19.0, -65.5],
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
function inMiningCorridor(lat: number, lon: number): boolean {
  for (const b of MINING_CORRIDORS) if (inBbox(lat, lon, b)) return true
  return false
}

// Bolivia tri-regional classifier
function boRegion(lat: number, lon: number): 'altiplano' | 'valles' | 'llanos' {
  // Llanos: east of -65, north/east of Cochabamba valleys
  if (lon > -65.0) return 'llanos'
  // Altiplano: west of -67.5 (La Paz/Oruro/Potosí highlands)
  if (lon < -67.0) return 'altiplano'
  // Valles: in between (Cochabamba, Sucre, Tarija)
  return 'valles'
}

interface BoFeat {
  coords: [number, number][]
  rodadura: string  // ABC: Pavimentada/Ripio/Urbano/etc, WCS: Revestimie Asfalto/Tierra
}

function loadRoads(file: string, rodaduraField: string): BoFeat[] {
  const path = resolve(CACHE_DIR, file)
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: BoFeat[] = []
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
      rodadura: (p[rodaduraField] || '').toString().trim(),
    })
  }
  return out
}

function buildGrid(features: BoFeat[]): Map<string, BoFeat[]> {
  const grid = new Map<string, BoFeat[]>()
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

function nearestRoad(midLat: number, midLon: number, grid: Map<string, BoFeat[]>, radiusM: number): BoFeat | null {
  const reach = Math.max(1, Math.ceil(radiusM / 1000))
  const baseLat = Math.floor(midLat * 100)
  const baseLon = Math.floor(midLon * 100)
  let best: BoFeat | null = null
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

const CLASS_AADT_ALT: Record<number, number> = {
  0: 25000, 1: 10000, 2: 5000, 3: 2500, 4: 1200, 5: 600, 6: 250,
}
const CLASS_AADT_VALLES: Record<number, number> = {
  0: 20000, 1: 8000, 2: 4000, 3: 2000, 4: 1000, 5: 500, 6: 200,
}
const CLASS_AADT_LLANOS: Record<number, number> = {
  0: 25000, 1: 10000, 2: 5000, 3: 2500, 4: 1200, 5: 600, 6: 250,
}

function roadAadt(feat: BoFeat): number {
  const r = feat.rodadura.toUpperCase()
  if (/PAVIMENT|ASFAL|HORMIG/.test(r)) return 15000
  if (/URBAN/.test(r)) return 12000
  if (/RIPIO|CONSTRUCCIO/.test(r)) return 5000
  if (/TIERRA/.test(r)) return 2000
  return 6000
}

function tierMultiplier(tier: 0 | 1 | 2): number {
  return tier === 1 ? 2.0 : tier === 2 ? 1.4 : 1.0
}

function splitVehicles(aadt: number, tier: 0 | 1 | 2, region: string, mining: boolean): { light: number; medium: number; heavy: number; moto: number } {
  if (tier === 1) {
    return {
      light: Math.round(aadt * 0.60),
      medium: Math.round(aadt * 0.05),
      heavy: Math.round(aadt * 0.10),
      moto: Math.round(aadt * 0.25),
    }
  }
  if (tier === 2) {
    return {
      light: Math.round(aadt * 0.62),
      medium: Math.round(aadt * 0.06),
      heavy: Math.round(aadt * 0.12),
      moto: Math.round(aadt * 0.20),
    }
  }
  if (mining) {
    return {
      light: Math.round(aadt * 0.45),
      medium: Math.round(aadt * 0.08),
      heavy: Math.round(aadt * 0.37),
      moto: Math.round(aadt * 0.10),
    }
  }
  if (region === 'altiplano') {
    return {
      light: Math.round(aadt * 0.50),
      medium: Math.round(aadt * 0.08),
      heavy: Math.round(aadt * 0.30),
      moto: Math.round(aadt * 0.12),
    }
  }
  if (region === 'llanos') {
    // Santa Cruz soy freight
    return {
      light: Math.round(aadt * 0.55),
      medium: Math.round(aadt * 0.08),
      heavy: Math.round(aadt * 0.25),
      moto: Math.round(aadt * 0.12),
    }
  }
  // Valles
  return {
    light: Math.round(aadt * 0.58),
    medium: Math.round(aadt * 0.08),
    heavy: Math.round(aadt * 0.22),
    moto: Math.round(aadt * 0.12),
  }
}

async function main() {
  console.log(`=== BO Roads Enrichment — ABC RVF + WCS + Bolivian CNOSSOS (${YEAR}) ===\n`)

  const abc = loadRoads('roads-rvf.geojson', 'rodadura')
  console.log(`  ABC Red Vial Fundamental: ${abc.length}`)

  const wcs = loadRoads('roads-wcs.geojson', 'Revestimie')
  console.log(`  WCS RED_VIAL:             ${wcs.length}`)

  const combined = [...abc, ...wcs]
  const grid = buildGrid(combined)
  console.log(`  Grid cells: ${grid.size}\n`)

  const hexDirs = iterateCountryHexes(H3R4_DIR, BO_HEX_BBOX)
  console.log(`  BO-bbox hexes with roads.arrow: ${hexDirs.length}`)

  let totalRoads = 0, excluded = 0, alreadyEnriched = 0
  let matchedSpatial = 0, matchedClass = 0
  let hexesUpdated = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hexId = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hexId, 'roads.arrow'),
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { alreadyEnriched++; return null }

        const midLat = row.midLat
        const midLon = row.midLon

        if (!inBbox(midLat, midLon, BO_BBOX)) return null
        if (inAnyZone(midLat, midLon)) { excluded++; return null }

        const tier = cityTier(midLat, midLon)
        const mult = tierMultiplier(tier)
        const cls = row.roadClass
        const region = boRegion(midLat, midLon)
        const mining = inMiningCorridor(midLat, midLon)

        let aadt = 0
        if (cls <= 2) {
          const near = nearestRoad(midLat, midLon, grid, 500)
          if (near) {
            aadt = roadAadt(near) * mult
            matchedSpatial++
          }
        }
        if (aadt === 0) return null  // A.1: unmatched → source_id=0 → engine country-tier cascade

        const split = splitVehicles(aadt, tier, region, mining && tier === 0)
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
      console.log(`  [${(elapsed / 1000).toFixed(0)}s] ${hi + 1}/${hexDirs.length}, ${matchedSpatial} spatial + ${matchedClass} class`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  Total roads scanned:        ${totalRoads.toLocaleString()}`)
  console.log(`  Already enriched (skip):    ${alreadyEnriched.toLocaleString()}`)
  console.log(`  Excluded (neighbours):      ${excluded.toLocaleString()}`)
  console.log(`  Matched by ABC/WCS:         ${matchedSpatial.toLocaleString()}`)
  console.log(`  Matched by class default:   ${matchedClass.toLocaleString()}`)
  const tot = matchedSpatial + matchedClass
  console.log(`  Total enriched:             ${tot.toLocaleString()} (${(100 * tot / Math.max(totalRoads, 1)).toFixed(2)}%)`)
  console.log(`  Hexes updated:              ${hexesUpdated}/${hexDirs.length}`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
