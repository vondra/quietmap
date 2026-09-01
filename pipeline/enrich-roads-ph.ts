/**
 * Enrich PH roads.arrow with DPWH Road Classification + Philippine-tuned
 * CNOSSOS class defaults.
 *
 * No per-segment AADT is published openly for the Philippines. DPWH RTI
 * portal at dpwh.gov.ph/dpwh/gis/rti is Incapsula-blocked; data.gov.ph is
 * a SPA without a public API. The only open machine-readable road data is
 * the DPWH Road_Classification ArcGIS FeatureServer.
 *
 * Source: DPWH Road_Classification (ArcGIS FeatureServer)
 *   URL: services1.arcgis.com/IwZZTMxZCmAmFYvF/arcgis/rest/services/
 *        Road_Classification/FeatureServer/1
 *   Records: 4,476 polyline segments of the PH national road network
 *   Classes: Primary 689, Secondary 1,570, Tertiary 2,217
 *   Fields: ROAD_NAME, ROAD_SEC_CLASS, ROUTE_NO
 *
 * ## AADT strategy
 *
 * 1. Tier A — DPWH spatial class match (within 300m):
 *      Primary (Expressway/major arterial) → 50,000 AADT × tier mult
 *      Secondary                            → 20,000 × tier mult
 *      Tertiary                             → 8,000 × tier mult
 *
 * 2. Tier B — Philippine-tuned CNOSSOS class defaults with Metro Manila ×2.0
 *    and regional city ×1.4 multipliers.
 *
 * ## Philippine vehicle split
 *
 * Philippines has a mix of jeepneys, tricycles, motorcycles, cars — similar
 * urban motorcycle share to Indonesia (~55% Metro Manila) but includes many
 * heavy jeepneys and tricycles.
 *
 *   Metro Manila (Tier-1):    light 35% / medium 8% / heavy 7% / moto 50%
 *   Regional (Tier-2):        light 45% / medium 8% / heavy 7% / moto 40%
 *   Rural:                    light 55% / medium 10% / heavy 10% / moto 25%
 *
 * Exclusion zones: Malaysia Sabah, Taiwan, Indonesia (small overlap).
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-ph.ts
 */

import { readFileSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_PH_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_PH_NATIONAL_ROADS

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/ph`)

// PH archipelago bbox
const PH_BBOX: [number, number, number, number] = [4.0, 115.8, 22.0, 127.5]

// Exclusion zones for neighbour countries inside PH bbox
const EXCLUDE_ZONES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Taiwan', bbox: [21.8, 119.5, 25.5, 122.1] },
  { name: 'Malaysia-Sabah', bbox: [4.0, 115.8, 7.4, 119.5] },
  { name: 'Indonesia-NE', bbox: [4.0, 124.0, 5.0, 127.5] },
]

// Metro Manila (NCR + adjacent) — the only Tier-1 metro
const METRO_MANILA: [number, number, number, number] = [14.3, 120.8, 14.85, 121.2]

// Regional Tier-2 cities
const TIER2_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Cebu City', bbox: [10.25, 123.8, 10.45, 124.0] },
  { name: 'Davao City', bbox: [7.0, 125.5, 7.15, 125.65] },
  { name: 'Quezon City', bbox: [14.6, 121.0, 14.75, 121.15] },  // Part of NCR but explicit
  { name: 'Iloilo City', bbox: [10.6, 122.5, 10.8, 122.7] },
  { name: 'Bacolod', bbox: [10.6, 122.9, 10.75, 123.05] },
  { name: 'Zamboanga City', bbox: [6.85, 122.0, 6.98, 122.15] },
  { name: 'Cagayan de Oro', bbox: [8.4, 124.58, 8.55, 124.73] },
  { name: 'Baguio', bbox: [16.38, 120.55, 16.45, 120.65] },
  { name: 'General Santos', bbox: [6.05, 125.1, 6.2, 125.22] },
  { name: 'Angeles', bbox: [15.12, 120.55, 15.2, 120.65] },
  { name: 'Bataan (Mariveles)', bbox: [14.35, 120.4, 14.5, 120.55] },
  { name: 'Cavite City', bbox: [14.45, 120.85, 14.55, 120.97] },
  { name: 'Calamba (Laguna)', bbox: [14.15, 121.1, 14.25, 121.2] },
  { name: 'Tacloban', bbox: [11.2, 124.95, 11.3, 125.05] },
  { name: 'Dagupan', bbox: [16.0, 120.32, 16.08, 120.4] },
  { name: 'Olongapo', bbox: [14.82, 120.25, 14.88, 120.32] },
  { name: 'Naga (Camarines Sur)', bbox: [13.6, 123.15, 13.7, 123.25] },
  { name: 'Butuan', bbox: [8.92, 125.52, 9.0, 125.62] },
  { name: 'Iligan', bbox: [8.2, 124.2, 8.28, 124.3] },
]

function inAnyZone(lat: number, lon: number): boolean {
  for (const z of EXCLUDE_ZONES) if (inBbox(lat, lon, z.bbox)) return true
  return false
}
function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  if (inBbox(lat, lon, METRO_MANILA)) return 1
  for (const c of TIER2_CITIES) if (inBbox(lat, lon, c.bbox)) return 2
  return 0
}

interface DpwhFeat {
  coords: [number, number][]
  cls: string
}

function loadDpwh(): DpwhFeat[] {
  const path = resolve(CACHE_DIR, 'roads-classification.geojson')
  if (!existsSync(path)) throw new Error(`Missing ${path}`)
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: DpwhFeat[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g) continue
    let coords: [number, number][] = []
    if (g.type === 'LineString') coords = g.coordinates
    else if (g.type === 'MultiLineString') {
      for (const part of g.coordinates) coords.push(...part)
    }
    if (coords.length < 2) continue
    out.push({ coords, cls: (f.properties?.ROAD_SEC_CLASS || '').trim() })
  }
  return out
}

function buildGrid(features: DpwhFeat[]): Map<string, DpwhFeat[]> {
  const grid = new Map<string, DpwhFeat[]>()
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

function nearestDpwh(midLat: number, midLon: number, grid: Map<string, DpwhFeat[]>, radiusM: number): DpwhFeat | null {
  const reach = Math.max(1, Math.ceil(radiusM / 1000))
  const baseLat = Math.floor(midLat * 100)
  const baseLon = Math.floor(midLon * 100)
  let best: DpwhFeat | null = null
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

const DPWH_AADT: Record<string, number> = {
  'Primary': 50000,
  'Secondary': 20000,
  'Tertiary': 8000,
}

function tierMultiplier(tier: 0 | 1 | 2): number {
  return tier === 1 ? 2.0 : tier === 2 ? 1.4 : 1.0
}

function splitVehicles(aadt: number, tier: 0 | 1 | 2): { light: number; medium: number; heavy: number; moto: number } {
  if (tier === 1) {
    return {
      light: Math.round(aadt * 0.35),
      medium: Math.round(aadt * 0.08),
      heavy: Math.round(aadt * 0.07),
      moto: Math.round(aadt * 0.50),
    }
  }
  if (tier === 2) {
    return {
      light: Math.round(aadt * 0.45),
      medium: Math.round(aadt * 0.08),
      heavy: Math.round(aadt * 0.07),
      moto: Math.round(aadt * 0.40),
    }
  }
  return {
    light: Math.round(aadt * 0.55),
    medium: Math.round(aadt * 0.10),
    heavy: Math.round(aadt * 0.10),
    moto: Math.round(aadt * 0.25),
  }
}

// PH archipelago bbox (mirrors PH_BBOX); iterateCountryHexes skips the rest of
// the planet so the loader doesn't read every roads.arrow on Earth.
const PH_HEX_BBOX: [number, number, number, number] = PH_BBOX

async function main() {
  console.log(`=== PH Roads Enrichment — DPWH + Philippine-tuned CNOSSOS (${YEAR}) ===\n`)
  const dpwh = loadDpwh()
  console.log(`  DPWH loaded: ${dpwh.length}`)
  const counts: Record<string, number> = {}
  for (const f of dpwh) counts[f.cls] = (counts[f.cls] || 0) + 1
  for (const [k, v] of Object.entries(counts).sort((a, b) => b[1] - a[1])) console.log(`    ${k}: ${v}`)
  const grid = buildGrid(dpwh)
  console.log(`  Grid cells: ${grid.size}\n`)

  const hexDirs = iterateCountryHexes(H3R4_DIR, PH_HEX_BBOX)
  console.log(`  PH-bbox hexes with roads.arrow: ${hexDirs.length}`)

  let totalRoads = 0, excluded = 0, alreadyEnriched = 0
  let matchedDpwh = 0
  const matchedClass = 0
  let hexesUpdated = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        // Fast-exit before the expensive DPWH grid match when a higher-priority
        // dataset already owns the row (writeRoadAadt re-checks the gate).
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { alreadyEnriched++; return null }

        const midLat = row.midLat
        const midLon = row.midLon

        if (!inBbox(midLat, midLon, PH_BBOX)) return null
        if (inAnyZone(midLat, midLon)) { excluded++; return null }

        const tier = cityTier(midLat, midLon)
        const mult = tierMultiplier(tier)
        const cls = row.roadClass

        let aadt = 0
        // DPWH spatial match only for OSM class ≤ 2
        if (cls <= 2) {
          const near = nearestDpwh(midLat, midLon, grid, 300)
          if (near && DPWH_AADT[near.cls]) {
            aadt = DPWH_AADT[near.cls] * mult
            matchedDpwh++
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
    if (elapsed > 10_000 && hi % 30 === 0) {
      console.log(`  [${(elapsed / 1000).toFixed(0)}s] ${hi + 1}/${hexDirs.length}, ${matchedDpwh} DPWH + ${matchedClass} class`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  Total roads scanned:        ${totalRoads.toLocaleString()}`)
  console.log(`  Already enriched (skip):    ${alreadyEnriched.toLocaleString()}`)
  console.log(`  Excluded (neighbours):      ${excluded.toLocaleString()}`)
  console.log(`  Matched by DPWH:            ${matchedDpwh.toLocaleString()}`)
  console.log(`  Matched by class default:   ${matchedClass.toLocaleString()}`)
  const tot = matchedDpwh + matchedClass
  console.log(`  Total enriched:             ${tot.toLocaleString()} (${(100 * tot / Math.max(totalRoads, 1)).toFixed(2)}%)`)
  console.log(`  Hexes updated:              ${hexesUpdated}/${hexDirs.length}`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
