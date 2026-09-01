/**
 * Enrich CO roads.arrow with INVIAS Red Vial Nacional + TPDS_NUBE TPDA.
 *
 * Sources:
 *   - **Red Vial Nacional** via INVIAS Esri mirror (`SIG_INVIAS` orgId
 *     `kyerLIHvrND0OSya`):
 *       services6.arcgis.com/kyerLIHvrND0OSya/.../RedVialNacional_OpenData/FeatureServer/0
 *     625 polylines of the national road network
 *     Fields: codigo_via, superficie ('1'=paved/'2'=unpaved), administrador
 *     ('1'=INVIAS / '2'=ANI / '3'=Concesión Departamental), calzada
 *     ('1'=single/'2'=dual), concesion, ruta, sector, territorial
 *
 *   - **TPDS_NUBE (TPDA traffic counts)** via same mirror:
 *       services6.arcgis.com/kyerLIHvrND0OSya/.../TPDS_NUBE/FeatureServer/0
 *     **1,271 polyline segments** with REAL traffic data, of which **822 have
 *     non-zero `conteo`** (AADT). This is the **ONLY country in the entire
 *     pipeline with native CNOSSOS-format vehicle class breakdown** —
 *     Colombia's INVIAS publishes per-segment splits as percentages and
 *     axle-class counts.
 *     Fields:
 *       conteo  — total AADT (4–86,400 range, max ~Bogotá area)
 *       au_p    — automoviles (cars) percentage
 *       bu_p    — buses percentage
 *       ca_p    — camiones (trucks) percentage
 *       tcam    — total truck count (raw)
 *       c3..c7  — axle-class breakdown (3, 4, 5, 6, 7+ axles)
 *       estacion, codigo_via, sector, territ, km
 *
 * **Key insight**: We use the actual vehicle splits from TPDA when matched,
 * instead of applying default tier-based splits. For non-matched segments,
 * fall back to Colombian-tuned class defaults.
 *
 * ## Colombian AADT defaults (fallback when no TPDA match)
 *
 *   | OSM class | Rural | Tier-1 (×2.0) | Tier-2 (×1.4) |
 *   |---|---:|---:|---:|
 *   | 0 motorway (autopista, Ruta del Sol)  | 35,000 | 70,000 | 49,000 |
 *   | 1 trunk (INVIAS Red Vial paved)        | 12,000 | 24,000 | 16,800 |
 *   | 2 primary                               |  6,000 | 12,000 |  8,400 |
 *   | 3 secondary                             |  3,000 |  6,000 |  4,200 |
 *   | 4 tertiary                              |  1,500 |  3,000 |  2,100 |
 *   | 5 residential                           |    700 |  1,400 |    980 |
 *
 * ## Red Vial spatial-match AADT (fallback by surface + admin)
 *
 *   ANI/INVIAS + paved (Concesionado) → 25,000
 *   INVIAS + paved single calzada       → 12,000
 *   Departamental + paved               → 6,000
 *   Unpaved                              → 1,500
 *
 * ## Colombian vehicle split (fallback when no TPDA per-segment match)
 *
 * Colombia has a **HIGH motorcycle share** (~25-30% urban) — second only to
 * SE Asia in this pipeline. Heavy share is moderate but elevated on coal
 * corridors (La Guajira/Cesar) and oil routes (Llanos/Putumayo).
 *
 *   Tier-1 (Bogotá/Medellín):  light 55% / medium 5% / heavy 10% / moto 30%
 *   Tier-2:                    light 60% / medium 5% / heavy 10% / moto 25%
 *   Rural:                     light 55% / medium 8% / heavy 22% / moto 15%
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-co.ts
 */

import { readFileSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_CO_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_CO_NATIONAL_ROADS

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/co`)

// Colombia bbox — covers all 5 main regions (Andes/Caribe/Amazon/Llanos/Pacífico)
const CO_BBOX: [number, number, number, number] = [-4.3, -82.0, 13.5, -66.8]

// Border with neighbours: Venezuela (E), Brazil (SE), Peru (S), Ecuador (SW), Panamá (NW)
const EXCLUDE_ZONES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  // Venezuela (NE) — east of approx -72 in northern half
  { name: 'Venezuela', bbox: [0.6, -72.5, 12.5, -59.0] },
  // Brazil (SE Amazon corner)
  { name: 'Brazil (Amazon)', bbox: [-4.3, -70.0, -1.0, -66.5] },
  // Peru (south, west of Brazil)
  { name: 'Peru', bbox: [-18.4, -82.0, -1.5, -69.0] },
  // Ecuador (SW)
  { name: 'Ecuador', bbox: [-5.0, -82.0, 1.5, -75.0] },
  // Panama (NW Darien)
  { name: 'Panama', bbox: [7.2, -83.0, 9.7, -77.2] },
]

// Tier-1 cities (×2.0)
const TIER1_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Bogotá D.C.', bbox: [4.50, -74.20, 4.85, -73.95] },
  { name: 'Medellín (Valle de Aburrá)', bbox: [6.10, -75.65, 6.35, -75.50] },
]

// Tier-2 cities (×1.4)
const TIER2_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Cali',          bbox: [3.30, -76.60, 3.55, -76.45] },
  { name: 'Barranquilla',  bbox: [10.90, -74.85, 11.05, -74.75] },
  { name: 'Cartagena',     bbox: [10.35, -75.55, 10.50, -75.45] },
  { name: 'Cúcuta',        bbox: [7.85, -72.55, 8.00, -72.45] },
  { name: 'Bucaramanga',   bbox: [7.05, -73.15, 7.20, -73.05] },
  { name: 'Pereira',       bbox: [4.78, -75.75, 4.88, -75.65] },
  { name: 'Santa Marta',   bbox: [11.20, -74.25, 11.30, -74.15] },
  { name: 'Ibagué',        bbox: [4.40, -75.25, 4.50, -75.15] },
  { name: 'Manizales',     bbox: [5.03, -75.55, 5.12, -75.45] },
  { name: 'Pasto',         bbox: [1.18, -77.30, 1.25, -77.25] },
  { name: 'Villavicencio', bbox: [4.10, -73.65, 4.20, -73.60] },
  { name: 'Neiva',         bbox: [2.90, -75.30, 2.97, -75.25] },
  { name: 'Armenia',       bbox: [4.50, -75.70, 4.55, -75.65] },
  { name: 'Soledad',       bbox: [10.88, -74.80, 10.95, -74.74] },
  { name: 'Soacha',        bbox: [4.55, -74.25, 4.65, -74.15] },
  { name: 'Valledupar',    bbox: [10.45, -73.27, 10.50, -73.22] },
  { name: 'Montería',      bbox: [8.73, -75.92, 8.80, -75.85] },
  { name: 'Sincelejo',     bbox: [9.27, -75.42, 9.32, -75.36] },
  { name: 'Buenaventura',  bbox: [3.85, -77.05, 3.90, -76.97] },
  { name: 'Tunja',         bbox: [5.52, -73.40, 5.58, -73.35] },
  { name: 'Riohacha',      bbox: [11.50, -72.95, 11.57, -72.88] },
  { name: 'Quibdó',        bbox: [5.67, -76.68, 5.72, -76.62] },
  { name: 'Florencia',     bbox: [1.59, -75.62, 1.64, -75.58] },
  { name: 'Popayán',       bbox: [2.42, -76.62, 2.47, -76.57] },
]

// Coal mining regions (Cerrejón La Guajira, Drummond Cesar) — extreme HGV share
const COAL_REGION_BBOXES: Array<[number, number, number, number]> = [
  [10.50, -73.00, 11.80, -72.00],  // La Guajira (Cerrejón)
  [9.50, -73.85, 10.50, -73.20],   // Cesar (Drummond/Calenturitas/La Loma)
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
function inCoalRegion(lat: number, lon: number): boolean {
  for (const b of COAL_REGION_BBOXES) if (inBbox(lat, lon, b)) return true
  return false
}

interface RvnFeat {
  coords: [number, number][]
  superficie: string
  administrador: string
  calzada: string
  concesion: string
}

interface TpdaFeat {
  coords: [number, number][]
  conteo: number
  auP: number       // cars %
  buP: number       // buses %
  caP: number       // trucks %
  estacion: string
  sector: string
}

function loadRvn(): RvnFeat[] {
  const path = resolve(CACHE_DIR, 'roads-network.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: RvnFeat[] = []
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
      superficie: (p.superficie || '').toString().trim(),
      administrador: (p.administrador || '').toString().trim(),
      calzada: (p.calzada || '').toString().trim(),
      concesion: (p.concesion || '').toString().trim(),
    })
  }
  return out
}

function loadTpda(): TpdaFeat[] {
  const path = resolve(CACHE_DIR, 'tpda-2024.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: TpdaFeat[] = []
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
    const conteo = typeof p.conteo === 'number' ? p.conteo : parseInt(String(p.conteo || 0), 10) || 0
    if (conteo < 50) continue
    out.push({
      coords,
      conteo,
      auP: typeof p.au_p === 'number' ? p.au_p : 65,
      buP: typeof p.bu_p === 'number' ? p.bu_p : 5,
      caP: typeof p.ca_p === 'number' ? p.ca_p : 30,
      estacion: (p.estacion || '').toString(),
      sector: (p.sector || '').toString(),
    })
  }
  return out
}

function buildLineGrid<T extends { coords: [number, number][] }>(features: T[]): Map<string, T[]> {
  const grid = new Map<string, T[]>()
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

function nearestLine<T extends { coords: [number, number][] }>(
  midLat: number, midLon: number, grid: Map<string, T[]>, radiusM: number,
): { feat: T; dist: number } | null {
  const reach = Math.max(1, Math.ceil(radiusM / 1000))
  const baseLat = Math.floor(midLat * 100)
  const baseLon = Math.floor(midLon * 100)
  let best: T | null = null
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
  return best ? { feat: best, dist: bestDist } : null
}

function rvnAadt(feat: RvnFeat): number {
  const paved = feat.superficie === '1'
  if (!paved) return 1500
  // Concesionada (toll road) — usually ANI 4G
  if (feat.administrador === '2' && feat.calzada === '2') return 25000
  if (feat.administrador === '2') return 18000  // ANI single calzada
  if (feat.administrador === '1') return 12000  // INVIAS national
  return 6000  // departmental
}

function tierMultiplier(tier: 0 | 1 | 2): number {
  return tier === 1 ? 2.0 : tier === 2 ? 1.4 : 1.0
}

function splitVehiclesFromTpda(aadt: number, auP: number, buP: number, caP: number): { light: number; medium: number; heavy: number; moto: number } {
  // Use actual vehicle percentages from TPDA. CNOSSOS doesn't have a "buses"
  // category — buses go into medium. Motorcycles aren't in TPDA so assume
  // 5% on TPDA-counted segments (mostly major intercity routes where moto
  // share is lower than urban averages).
  const motoPct = 5
  const total = auP + buP + caP
  // Normalize to 95% (leaving 5% for motos)
  const scale = total > 0 ? 95 / total : 1
  const light = Math.round(aadt * (auP * scale) / 100)
  const medium = Math.round(aadt * (buP * scale) / 100)
  const heavy = Math.round(aadt * (caP * scale) / 100)
  const moto = Math.round(aadt * motoPct / 100)
  return { light, medium, heavy, moto }
}

function splitVehiclesDefault(aadt: number, tier: 0 | 1 | 2, coal: boolean): { light: number; medium: number; heavy: number; moto: number } {
  if (tier === 1) {
    return {
      light: Math.round(aadt * 0.55),
      medium: Math.round(aadt * 0.05),
      heavy: Math.round(aadt * 0.10),
      moto: Math.round(aadt * 0.30),
    }
  }
  if (tier === 2) {
    return {
      light: Math.round(aadt * 0.60),
      medium: Math.round(aadt * 0.05),
      heavy: Math.round(aadt * 0.10),
      moto: Math.round(aadt * 0.25),
    }
  }
  if (coal) {
    // Coal corridor — extreme heavy share for trucks/conveyors
    return {
      light: Math.round(aadt * 0.40),
      medium: Math.round(aadt * 0.05),
      heavy: Math.round(aadt * 0.45),
      moto: Math.round(aadt * 0.10),
    }
  }
  // Rural — high motorcycle + heavy share
  return {
    light: Math.round(aadt * 0.55),
    medium: Math.round(aadt * 0.08),
    heavy: Math.round(aadt * 0.22),
    moto: Math.round(aadt * 0.15),
  }
}

async function main() {
  console.log(`=== CO Roads Enrichment — INVIAS Red Vial + TPDS_NUBE TPDA + Colombian CNOSSOS (${YEAR}) ===\n`)

  const rvn = loadRvn()
  console.log(`  Red Vial Nacional loaded: ${rvn.length}`)

  const tpda = loadTpda()
  console.log(`  TPDS_NUBE loaded:        ${tpda.length} segments with REAL AADT + vehicle class`)

  const rvnGrid = buildLineGrid(rvn)
  const tpdaGrid = buildLineGrid(tpda)
  console.log(`  RVN grid cells:  ${rvnGrid.size}`)
  console.log(`  TPDA grid cells: ${tpdaGrid.size}\n`)

  // [minLat,minLon,maxLat,maxLon] — iterateCountryHexes skips the rest of the
  // planet so the loader doesn't read every roads.arrow on Earth.
  const hexDirs = iterateCountryHexes(H3R4_DIR, CO_BBOX)
  console.log(`  CO-bbox hexes with roads.arrow: ${hexDirs.length}`)

  let totalRoads = 0, excluded = 0, alreadyEnriched = 0
  let matchedTpda = 0, matchedRvn = 0
  const matchedClass = 0
  let hexesUpdated = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        // Fast-exit before the expensive two-source spatial match when a
        // higher-priority dataset already owns the row (writeRoadAadt re-checks
        // the gate — this only saves work, and preserves the original
        // alreadyEnriched diagnostic counter).
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { alreadyEnriched++; return null }

        const midLat = row.midLat
        const midLon = row.midLon

        if (!inBbox(midLat, midLon, CO_BBOX)) return null
        if (inAnyZone(midLat, midLon)) { excluded++; return null }

        const tier = cityTier(midLat, midLon)
        const mult = tierMultiplier(tier)
        const cls = row.roadClass
        const coal = inCoalRegion(midLat, midLon)

        let split: { light: number; medium: number; heavy: number; moto: number } | null = null
        // 1. TPDA real AADT + real vehicle class breakdown (best, only for class ≤ 2)
        if (cls <= 2) {
          const t = nearestLine(midLat, midLon, tpdaGrid, 500)
          if (t) {
            const aadt = t.feat.conteo * mult
            split = splitVehiclesFromTpda(aadt, t.feat.auP, t.feat.buP, t.feat.caP)
            matchedTpda++
          }
        }
        // 2. Red Vial Nacional spatial join (paved/concesionado/single-vs-dual calzada)
        if (!split && cls <= 2) {
          const d = nearestLine(midLat, midLon, rvnGrid, 400)
          if (d) {
            const aadt = rvnAadt(d.feat) * mult
            split = splitVehiclesDefault(aadt, tier, coal)
            matchedRvn++
          }
        }
        // 3. Colombian class default
        if (!split) return null  // A.1: unmatched → source_id=0 → engine country-tier cascade

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
      console.log(`  [${(elapsed / 1000).toFixed(0)}s] ${hi + 1}/${hexDirs.length}, ${matchedTpda} TPDA + ${matchedRvn} RVN + ${matchedClass} class`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  Total roads scanned:        ${totalRoads.toLocaleString()}`)
  console.log(`  Already enriched (skip):    ${alreadyEnriched.toLocaleString()}`)
  console.log(`  Excluded (neighbours):      ${excluded.toLocaleString()}`)
  console.log(`  Matched by TPDA (real):     ${matchedTpda.toLocaleString()}`)
  console.log(`  Matched by RVN class:       ${matchedRvn.toLocaleString()}`)
  console.log(`  Matched by class default:   ${matchedClass.toLocaleString()}`)
  const tot = matchedTpda + matchedRvn + matchedClass
  console.log(`  Total enriched:             ${tot.toLocaleString()} (${(100 * tot / Math.max(totalRoads, 1)).toFixed(2)}%)`)
  console.log(`  Hexes updated:              ${hexesUpdated}/${hexDirs.length}`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
