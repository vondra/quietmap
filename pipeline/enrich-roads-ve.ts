/**
 * Enrich VE roads.arrow with VE360 Vialidad + Venezuelan CNOSSOS.
 *
 * Source:
 *   - **VE360 Vialidad** (proyecto.ve360 SIGOT mirror):
 *       services6.arcgis.com/lpJCO3ug8HhNiEOV/.../Vialidad/FeatureServer/0
 *     15,528 polyline features with Tipo_vía classification:
 *       Autopista: 49
 *       Carretera Pavimentada + 2 Vías: 123
 *       Carretera Pavimentada de 2 vias: 1
 *       Carretera Engranzonada de +2 vías: 554
 *       Carretera Engranzonada de 2 vías: 769
 *       Carretera Engranzonada: 2
 *       Camino Carretero: 3,464
 *       Camino Carretero / Sendero o Pica: 1,941
 *       Carretera de Tierra: 6,666
 *       Sendero o Pica: 760
 *       Ferrocarril: 5
 *       Puente: 1, Tunel: 1, blank: 1,192
 *
 * **No per-segment TPDA/IMD available.** MINTRANSPORTE/INVEA gov portals
 * are all offline. Venezuelan road data is frozen at ~2013-2019 vintage
 * via the SIGOT mirror.
 *
 * ## Venezuelan AADT defaults
 *
 *   | OSM class | Rural | Tier-1 (×2.0) | Tier-2 (×1.4) |
 *   |---|---:|---:|---:|
 *   | 0 motorway (autopista) | 30,000 | 60,000 | 42,000 |
 *   | 1 trunk (Troncal paved)| 10,000 | 20,000 | 14,000 |
 *   | 2 primary               |  5,000 | 10,000 |  7,000 |
 *   | 3 secondary             |  2,500 |  5,000 |  3,500 |
 *   | 4 tertiary              |  1,200 |  2,400 |  1,680 |
 *   | 5 residential           |    600 |  1,200 |    840 |
 *
 * Note: Venezuela's auto fleet is small and declining due to the economic
 * collapse — real traffic is lower than these defaults suggest, especially
 * on the Troncales. Keep defaults conservative.
 *
 * ## Vialidad spatial-match AADT (by Tipo_vía)
 *
 *   Autopista                                    → 22,000
 *   Carretera Pavimentada + 2 Vías                → 12,000
 *   Carretera Pavimentada de 2 vias               →  8,000
 *   Carretera Engranzonada de +2 vías             →  5,000
 *   Carretera Engranzonada de 2 vías              →  3,000
 *   Camino Carretero                              →  1,800
 *   Camino Carretero / Sendero o Pica             →  1,000
 *   Carretera de Tierra                           →  1,500
 *   Sendero o Pica                                 →    500
 *
 * ## Venezuelan vehicle split
 *
 * High motorcycle share (~25% urban — cheap Chinese imports, fuel
 * subsidized). Low heavy share due to economic collapse (less freight).
 *
 *   Tier-1 (Caracas/Maracaibo/Valencia/Barquisimeto): light 60% / medium 5% / heavy 10% / moto 25%
 *   Tier-2:                                            light 62% / medium 6% / heavy 12% / moto 20%
 *   Rural:                                             light 58% / medium 8% / heavy 22% / moto 12%
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-ve.ts
 */

import { readFileSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_VE_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_VE_NATIONAL_ROADS

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/ve`)

const VE_BBOX: [number, number, number, number] = [0.6, -73.4, 12.5, -59.0]

const EXCLUDE_ZONES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Colombia W', bbox: [-4.3, -73.4, 12.5, -67.0] },
  { name: 'Brazil S', bbox: [0.6, -68.0, 5.3, -59.0] },
  { name: 'Guyana E', bbox: [0.6, -61.4, 8.7, -56.5] },
]

// Tier-1 cities (×2.0)
const TIER1_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Caracas', bbox: [10.40, -67.05, 10.55, -66.75] },
  { name: 'Maracaibo', bbox: [10.58, -71.75, 10.75, -71.55] },
  { name: 'Valencia', bbox: [10.10, -68.10, 10.25, -67.95] },
  { name: 'Barquisimeto', bbox: [9.97, -69.40, 10.10, -69.25] },
]

// Tier-2 cities (×1.4)
const TIER2_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Ciudad Guayana', bbox: [8.25, -62.80, 8.40, -62.55] },
  { name: 'Maracay',      bbox: [10.22, -67.65, 10.33, -67.50] },
  { name: 'Maturín',      bbox: [9.72, -63.22, 9.82, -63.12] },
  { name: 'Barcelona',    bbox: [10.10, -64.75, 10.18, -64.65] },
  { name: 'Puerto La Cruz', bbox: [10.18, -64.72, 10.25, -64.60] },
  { name: 'San Cristóbal', bbox: [7.73, -72.28, 7.82, -72.18] },
  { name: 'Cumaná',       bbox: [10.42, -64.22, 10.50, -64.13] },
  { name: 'Mérida',       bbox: [8.57, -71.18, 8.65, -71.10] },
  { name: 'Ciudad Bolívar', bbox: [8.10, -63.60, 8.20, -63.50] },
  { name: 'Cabimas',      bbox: [10.35, -71.48, 10.45, -71.40] },
  { name: 'Coro',         bbox: [11.38, -69.70, 11.45, -69.63] },
  { name: 'Los Teques',   bbox: [10.32, -67.07, 10.38, -66.98] },
  { name: 'Guarenas',     bbox: [10.45, -66.65, 10.50, -66.58] },
  { name: 'Guanare',      bbox: [9.02, -69.77, 9.07, -69.70] },
  { name: 'Valera',       bbox: [9.30, -70.63, 9.35, -70.57] },
  { name: 'Punto Fijo',   bbox: [11.67, -70.22, 11.72, -70.17] },
  { name: 'Acarigua',     bbox: [9.53, -69.22, 9.58, -69.17] },
  { name: 'Barinas',      bbox: [8.60, -70.25, 8.67, -70.18] },
  { name: 'San Felipe',   bbox: [10.32, -68.77, 10.37, -68.72] },
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

interface VeFeat {
  coords: [number, number][]
  tipo: string
}

function loadVialidad(): VeFeat[] {
  const path = resolve(CACHE_DIR, 'roads-vialidad.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: VeFeat[] = []
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
      tipo: (p['Tipo_vía'] || '').toString().trim(),
    })
  }
  return out
}

function buildGrid(features: VeFeat[]): Map<string, VeFeat[]> {
  const grid = new Map<string, VeFeat[]>()
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

function nearestRoad(midLat: number, midLon: number, grid: Map<string, VeFeat[]>, radiusM: number): VeFeat | null {
  const reach = Math.max(1, Math.ceil(radiusM / 1000))
  const baseLat = Math.floor(midLat * 100)
  const baseLon = Math.floor(midLon * 100)
  let best: VeFeat | null = null
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

function vialidadAadt(feat: VeFeat): number {
  const t = feat.tipo.toLowerCase()
  if (/autopista/.test(t)) return 22000
  if (/pavimentada.*\+\s*2/.test(t)) return 12000
  if (/pavimentada/.test(t)) return 8000
  if (/engranzonada.*\+\s*2/.test(t) || /engranzonada.*\+2/.test(t)) return 5000
  if (/engranzonada/.test(t)) return 3000
  if (/camino.*sendero/.test(t)) return 1000
  if (/camino/.test(t)) return 1800
  if (/tierra/.test(t)) return 1500
  if (/sendero/.test(t)) return 500
  return 2000
}

function tierMultiplier(tier: 0 | 1 | 2): number {
  return tier === 1 ? 2.0 : tier === 2 ? 1.4 : 1.0
}

function splitVehicles(aadt: number, tier: 0 | 1 | 2): { light: number; medium: number; heavy: number; moto: number } {
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
  // Rural — Venezuelan economy has collapsed, fewer trucks
  return {
    light: Math.round(aadt * 0.58),
    medium: Math.round(aadt * 0.08),
    heavy: Math.round(aadt * 0.22),
    moto: Math.round(aadt * 0.12),
  }
}

// Venezuela bbox; [minLat,minLon,maxLat,maxLon]. iterateCountryHexes skips the rest
// of the planet, and the same box re-gates each segment midpoint inside the closure.
const VE_HEX_BBOX: [number, number, number, number] = VE_BBOX

async function main() {
  console.log(`=== VE Roads Enrichment — VE360 Vialidad + Venezuelan CNOSSOS (${YEAR}) ===\n`)

  const vialidad = loadVialidad()
  console.log(`  VE360 Vialidad loaded: ${vialidad.length}`)
  const grid = buildGrid(vialidad)
  console.log(`  Grid cells: ${grid.size}\n`)

  const hexDirs = iterateCountryHexes(H3R4_DIR, VE_HEX_BBOX)
  console.log(`  VE-bbox hexes with roads.arrow: ${hexDirs.length}`)

  let totalRoads = 0, excluded = 0, alreadyEnriched = 0
  let matchedSpatial = 0, matchedClass = 0
  let hexesUpdated = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hexId = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hexId, 'roads.arrow'),
      (row) => {
        // Fast-exit before the expensive grid match when a higher-priority dataset
        // already owns the row (writeRoadAadt re-checks the gate — this only saves work).
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { alreadyEnriched++; return null }

        const midLat = row.midLat
        const midLon = row.midLon

        if (!inBbox(midLat, midLon, VE_BBOX)) return null
        if (inAnyZone(midLat, midLon)) { excluded++; return null }

        const tier = cityTier(midLat, midLon)
        const mult = tierMultiplier(tier)
        const cls = row.roadClass

        let aadt = 0
        if (cls <= 2) {
          const near = nearestRoad(midLat, midLon, grid, 400)
          if (near) {
            aadt = vialidadAadt(near) * mult
            matchedSpatial++
          }
        }
        if (aadt === 0) return null  // A.1: unmatched → source_id=0 → engine country-tier cascade

        const split = splitVehicles(aadt, tier)
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
      console.log(`  [${(elapsed / 1000).toFixed(0)}s] ${hi + 1}/${hexDirs.length}, ${matchedSpatial} Vialidad + ${matchedClass} class`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  Total roads scanned:        ${totalRoads.toLocaleString()}`)
  console.log(`  Already enriched (skip):    ${alreadyEnriched.toLocaleString()}`)
  console.log(`  Excluded (neighbours):      ${excluded.toLocaleString()}`)
  console.log(`  Matched by Vialidad:        ${matchedSpatial.toLocaleString()}`)
  console.log(`  Matched by class default:   ${matchedClass.toLocaleString()}`)
  const tot = matchedSpatial + matchedClass
  console.log(`  Total enriched:             ${tot.toLocaleString()} (${(100 * tot / Math.max(totalRoads, 1)).toFixed(2)}%)`)
  console.log(`  Hexes updated:              ${hexesUpdated}/${hexDirs.length}`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
