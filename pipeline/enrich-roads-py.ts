/**
 * Enrich PY roads.arrow with MOPC Rutas Nacionales + Paraguayan defaults.
 *
 * Sources:
 *   - **MOPC Rutas Nacionales 2023** (downloaded as KMZ, converted to GeoJSON):
 *       22 national routes (PY01–PY22) combined into 22 MultiLineString features
 *       Fields: CODIGO, DEPARTAMEN, TIPO_SUP (PCA=paved / T=tierra-unpaved / mixed),
 *               TIPO_RED (NACIONAL), DESCP_DIST, DESCP_TRAM, INICIO, FIN,
 *               REGION (ORIENTAL/OCCIDENTAL), LONG_TOT_R
 *     Routes: PY01 Asunción↔Encarnación 378 km, PY02 Asunción↔Ciudad del Este
 *     343 km, PY03 Asunción↔Pedro Juan Caballero 413 km, PY07 Ciudad del Este↔
 *     Villarrica 417 km, PY09 Transchaco (Asunción↔Bolivia) 780 km, PY12 744 km
 *
 * **No TPDA/traffic volumes published by MOPC.** Use class-based defaults.
 *
 * ## Paraguayan AADT defaults
 *
 *   | OSM class | Rural | Tier-1 (×2.0) | Tier-2 (×1.4) |
 *   |---|---:|---:|---:|
 *   | 0 motorway (autopista — none in PY) | 30,000 | 60,000 | 42,000 |
 *   | 1 trunk (Ruta Nacional paved)       | 10,000 | 20,000 | 14,000 |
 *   | 2 primary                             |  5,000 | 10,000 |  7,000 |
 *   | 3 secondary                           |  2,500 |  5,000 |  3,500 |
 *   | 4 tertiary                            |  1,200 |  2,400 |  1,680 |
 *   | 5 residential                         |    600 |  1,200 |    840 |
 *
 * ## MOPC spatial-match AADT (by TIPO_SUP)
 *
 *   PCA (paved) Ruta Nacional → 12,000
 *   PCA, T (mixed) Ruta Nacional → 8,000
 *   T (unpaved) Ruta Nacional → 3,000
 *
 * ## Paraguayan vehicle split
 *
 * Paraguay has moderate motorcycle share (~20% urban — Paraguay has
 * cheap Chinese motorbike imports). Very high heavy share on Ruta 2
 * and Ruta 7 (soy freight from Alto Paraná to Paranaguá/Buenos Aires
 * via trucks and Río Paraná barges).
 *
 *   Tier-1 (Gran Asunción): light 65% / medium 5% / heavy 10% / moto 20%
 *   Tier-2:                 light 62% / medium 6% / heavy 12% / moto 20%
 *   Rural (soy corridors):  light 55% / medium 8% / heavy 25% / moto 12%
 *   Chaco (low-density):    light 60% / medium 10% / heavy 20% / moto 10%
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-py.ts
 */

import { readFileSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_PY_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_PY_NATIONAL_ROADS

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/py`)

const PY_BBOX: [number, number, number, number] = [-27.7, -62.7, -19.3, -54.2]

const EXCLUDE_ZONES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Brazil NE', bbox: [-22.0, -54.5, -19.3, -53.0] },
  { name: 'Argentina SW', bbox: [-28.0, -62.7, -27.5, -57.0] },
  { name: 'Bolivia NW', bbox: [-21.0, -62.7, -19.3, -57.5] },
]

// Tier-1 (×2.0) — Gran Asunción
const TIER1_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Gran Asunción', bbox: [-25.45, -57.72, -25.20, -57.35] },
]

// Tier-2 cities (×1.4)
const TIER2_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Ciudad del Este', bbox: [-25.55, -54.72, -25.45, -54.58] },
  { name: 'Encarnación',    bbox: [-27.35, -55.90, -27.28, -55.78] },
  { name: 'Luque',          bbox: [-25.30, -57.55, -25.22, -57.45] },
  { name: 'San Lorenzo',    bbox: [-25.37, -57.55, -25.30, -57.48] },
  { name: 'Capiatá',        bbox: [-25.40, -57.48, -25.33, -57.40] },
  { name: 'Lambaré',        bbox: [-25.36, -57.65, -25.30, -57.58] },
  { name: 'Fernando de la Mora', bbox: [-25.35, -57.57, -25.30, -57.50] },
  { name: 'Limpio',         bbox: [-25.17, -57.50, -25.12, -57.43] },
  { name: 'Ñemby',          bbox: [-25.42, -57.55, -25.36, -57.50] },
  { name: 'Pedro Juan Caballero', bbox: [-22.58, -55.77, -22.52, -55.70] },
  { name: 'Concepción',     bbox: [-23.43, -57.47, -23.38, -57.39] },
  { name: 'Coronel Oviedo', bbox: [-25.45, -56.48, -25.40, -56.42] },
  { name: 'Villarrica',     bbox: [-25.80, -56.48, -25.73, -56.40] },
  { name: 'Pilar',          bbox: [-26.88, -58.32, -26.82, -58.25] },
  { name: 'Caaguazú',       bbox: [-25.50, -56.05, -25.42, -55.97] },
  { name: 'San Lorenzo de Chaco (Filadelfia)', bbox: [-22.36, -60.06, -22.32, -60.00] },
  { name: 'Salto del Guairá', bbox: [-24.09, -54.35, -24.00, -54.28] },
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
// Chaco region (Occidental) — sparse, different vehicle split
function inChaco(lat: number, lon: number): boolean {
  // West of Río Paraguay (~-57.3) and north of Asunción
  return lon < -57.3 && lat > -26.0
}

interface MopcFeat {
  coords: [number, number][]
  codigo: string
  tipoSup: string
}

function loadMopcRoads(): MopcFeat[] {
  const path = resolve(CACHE_DIR, 'roads-mopc.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: MopcFeat[] = []
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
      codigo: (p.CODIGO || '').toString().trim(),
      tipoSup: (p.TIPO_SUP || '').toString().trim(),
    })
  }
  return out
}

function buildGrid(features: MopcFeat[]): Map<string, MopcFeat[]> {
  const grid = new Map<string, MopcFeat[]>()
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

function nearestRoad(midLat: number, midLon: number, grid: Map<string, MopcFeat[]>, radiusM: number): MopcFeat | null {
  const reach = Math.max(1, Math.ceil(radiusM / 1000))
  const baseLat = Math.floor(midLat * 100)
  const baseLon = Math.floor(midLon * 100)
  let best: MopcFeat | null = null
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

function mopcAadt(feat: MopcFeat): number {
  const sup = feat.tipoSup.toUpperCase()
  if (/^PCA$/.test(sup)) return 12000  // fully paved
  if (/PCA.*T|T.*PCA/.test(sup)) return 8000  // mixed
  if (/^T$/.test(sup)) return 3000  // unpaved
  return 6000
}

function tierMultiplier(tier: 0 | 1 | 2): number {
  return tier === 1 ? 2.0 : tier === 2 ? 1.4 : 1.0
}

function splitVehicles(aadt: number, tier: 0 | 1 | 2, chaco: boolean): { light: number; medium: number; heavy: number; moto: number } {
  if (tier === 1) {
    return {
      light: Math.round(aadt * 0.65),
      medium: Math.round(aadt * 0.05),
      heavy: Math.round(aadt * 0.10),
      moto: Math.round(aadt * 0.20),
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
  if (chaco) {
    return {
      light: Math.round(aadt * 0.60),
      medium: Math.round(aadt * 0.10),
      heavy: Math.round(aadt * 0.20),
      moto: Math.round(aadt * 0.10),
    }
  }
  // Rural Oriental (soy freight corridors)
  return {
    light: Math.round(aadt * 0.55),
    medium: Math.round(aadt * 0.08),
    heavy: Math.round(aadt * 0.25),
    moto: Math.round(aadt * 0.12),
  }
}

// Paraguay bbox (matches PY_BBOX); [minLat,minLon,maxLat,maxLon]. iterateCountryHexes
// skips the rest of the planet so the loader doesn't read every roads.arrow on Earth.
const PY_HEX_BBOX: [number, number, number, number] = PY_BBOX

async function main() {
  console.log(`=== PY Roads Enrichment — MOPC Rutas Nacionales + Paraguayan CNOSSOS (${YEAR}) ===\n`)

  if (!existsSync(H3R4_DIR)) {
    console.error(`ERROR: H3R4 directory not found: ${H3R4_DIR}`)
    process.exit(1)
  }

  const mopc = loadMopcRoads()
  console.log(`  MOPC Rutas Nacionales loaded: ${mopc.length}`)
  const grid = buildGrid(mopc)
  console.log(`  Grid cells: ${grid.size}\n`)

  const hexDirs = iterateCountryHexes(H3R4_DIR, PY_HEX_BBOX)
  console.log(`  PY-bbox hexes with roads.arrow: ${hexDirs.length}`)

  let totalRoads = 0, excluded = 0, alreadyEnriched = 0
  let matchedMopc = 0
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

        if (!inBbox(midLat, midLon, PY_BBOX)) return null
        if (inAnyZone(midLat, midLon)) { excluded++; return null }

        const tier = cityTier(midLat, midLon)
        const mult = tierMultiplier(tier)
        const cls = row.roadClass
        const chaco = inChaco(midLat, midLon)

        let aadt = 0
        // 1. MOPC spatial match (class ≤ 2)
        if (cls <= 2) {
          const near = nearestRoad(midLat, midLon, grid, 500)
          if (near) {
            aadt = mopcAadt(near) * mult
            matchedMopc++
          }
        }
        // 2. Paraguayan class default
        if (aadt === 0) return null  // A.1: unmatched → source_id=0 → engine country-tier cascade

        const split = splitVehicles(aadt, tier, chaco)
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
      console.log(`  [${(elapsed / 1000).toFixed(0)}s] ${hi + 1}/${hexDirs.length}, ${matchedMopc} MOPC + ${matchedClass} class`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  Total roads scanned:        ${totalRoads.toLocaleString()}`)
  console.log(`  Already enriched (skip):    ${alreadyEnriched.toLocaleString()}`)
  console.log(`  Excluded (neighbours):      ${excluded.toLocaleString()}`)
  console.log(`  Matched by MOPC class:      ${matchedMopc.toLocaleString()}`)
  console.log(`  Matched by class default:   ${matchedClass.toLocaleString()}`)
  const tot = matchedMopc + matchedClass
  console.log(`  Total enriched:             ${tot.toLocaleString()} (${(100 * tot / Math.max(totalRoads, 1)).toFixed(2)}%)`)
  console.log(`  Hexes updated:              ${hexesUpdated}/${hexDirs.length}`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
