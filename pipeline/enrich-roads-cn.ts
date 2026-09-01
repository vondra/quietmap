/**
 * Enrich CN roads.arrow with China Highway Network + Chinese-tuned CNOSSOS defaults.
 *
 * No per-segment AADT is published openly for China. Ministry of Transport
 * (mot.gov.cn) yearbook data is PDF-only; data.gov.cn is TCP-blocked from
 * non-CN IPs. The only open machine-readable road-network dataset is an
 * ArcGIS Online public layer with per-feature classification (Highway /
 * Major / Local).
 *
 * **The workaround**: `services1.arcgis.com/ERdCHt0sNM6dENSD/.../
 * China_Province_Cities_Highways_Network_WFL1` — 8,690 CN road polylines
 * (of 8,724 total with minor neighbor spillover) classified by TYPE and
 * RANK. No traffic counts, but class is a useful proxy for CNOSSOS defaults.
 *
 * ## Sources
 *
 * 1. **China Province Cities Highways Network** (ArcGIS Online)
 *    URL: `services1.arcgis.com/ERdCHt0sNM6dENSD/arcgis/rest/services/
 *          China_Province_Cities_Highways_Network_WFL1/FeatureServer/0`
 *    - 8,690 CN polylines
 *    - TYPE: Highway (3,428), Major road (5,099), Local road (185)
 *    - Fields: TYPE, RANK, Name1/Shield (route number, e.g. G1, G5, etc.),
 *              ISO_CC (filter to CN)
 *    - Used as spatial match for OSM road_class ≤ 2 (performance).
 *
 * 2. **Chinese-tuned CNOSSOS class defaults** (fallback)
 *    Chinese roads carry high traffic: a typical G-series expressway (G1 Beijing-
 *    Harbin, G4 Beijing-Hong Kong-Macau) carries 50-100k AADT. Urban arterials
 *    in Tier-1 metros (Beijing 3rd Ring, Shanghai Inner Ring) carry 150k+ AADT.
 *
 *    | Class | Rural AADT | Tier-1 ×2.0 | Tier-2 ×1.4 |
 *    |---|---:|---:|---:|
 *    | 0 motorway (G-roads) | 60,000 | 120,000 | 84,000 |
 *    | 1 trunk (provincial S-roads) | 25,000 | 50,000 | 35,000 |
 *    | 2 primary (prefectural) | 12,000 | 24,000 | 16,800 |
 *    | 3 secondary (county) | 5,000 | 10,000 | 7,000 |
 *    | 4 tertiary | 2,000 | 4,000 | 2,800 |
 *    | 5 residential | 1,000 | 2,000 | 1,400 |
 *
 * 3. **Chinese vehicle split** — different from India/Thailand. Gas motorcycles
 *    are banned in most major cities; electric scooters dominate but are quiet
 *    (don't contribute to noise). Urban trucks restricted during daytime.
 *
 *    Urban (Tier-1/2):  light 75% / medium 10% / heavy 10% / moto 5%
 *    Rural:             light 65% / medium 12% / heavy 18% / moto 5%
 *
 * Tier-1 cities (×2.0): Beijing, Shanghai, Guangzhou, Shenzhen, Chengdu,
 *   Chongqing, Wuhan, Xi'an, Hangzhou, Nanjing, Suzhou, Tianjin
 *
 * Tier-2 cities (×1.4): Changsha, Qingdao, Dalian, Hefei, Zhengzhou, Jinan,
 *   Kunming, Fuzhou, Xiamen, Ningbo, Wuxi, Harbin, Shenyang, Nanchang,
 *   Hohhot, Urumqi, Lanzhou, Xining, Guiyang, Haikou, Nanning, Lhasa,
 *   Yinchuan, Taiyuan, Shijiazhuang, Dongguan, Foshan, Jinhua, Wenzhou,
 *   Shantou, Zhuhai, Zhongshan, Huizhou
 *
 * Exclusion zones: Mongolia, Russia (far east), Kazakhstan, Kyrgyzstan,
 *   Tajikistan, Afghanistan, Pakistan (far west), India (west + south edge),
 *   Nepal, Bhutan, Myanmar, Laos, Vietnam, North Korea, South Korea (edge),
 *   Japan (far east edge).
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-cn.ts
 */

import { readFileSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { SOURCES_BY_KEY } from './lib/sources.js'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_CN_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { flatDist, inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_CN_NATIONAL_ROADS

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/cn`)

// CN coarse bbox. [minLat,minLon,maxLat,maxLon] — fed directly to iterateCountryHexes
// (skips the rest of the planet's roads.arrow) and re-checked per segment midpoint.
const CN_HEX_BBOX: [number, number, number, number] = [18.0, 73.0, 54.0, 135.5]

// Exclusion zones for neighbour countries inside CN bbox
const EXCLUDE_ZONES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  // India / Nepal / Bhutan (far south-west Himalayan corner)
  { name: 'India-south', bbox: [18.0, 73.0, 29.0, 85.0] },
  // Pakistan / Afghanistan (far west, lat 32-38, lon 73-75)
  { name: 'Pakistan-West', bbox: [24.0, 73.0, 37.5, 74.5] },
  // Nepal
  { name: 'Nepal', bbox: [26.5, 80.0, 30.5, 88.2] },
  // Bhutan
  { name: 'Bhutan', bbox: [26.7, 88.8, 28.3, 92.1] },
  // Myanmar (far south border, north to 28°N)
  { name: 'Myanmar', bbox: [18.0, 92.0, 28.5, 100.5] },
  // Laos (south of 22°N, west of 104°E)
  { name: 'Laos', bbox: [13.0, 100.0, 22.5, 107.8] },
  // Vietnam (east, lon 102-110)
  { name: 'Vietnam', bbox: [8.0, 102.0, 23.5, 110.0] },
  // Thailand (south of 21°N, west of 106°E) — defensive
  { name: 'Thailand', bbox: [5.0, 97.0, 21.0, 106.0] },
  // Kazakhstan / Kyrgyzstan / Tajikistan (far west Xinjiang border)
  { name: 'Central Asia', bbox: [36.0, 73.0, 50.0, 81.0] },
  // Mongolia (north of 42°N, lon 88-120°E roughly)
  { name: 'Mongolia', bbox: [42.0, 88.0, 54.0, 120.0] },
  // North Korea
  { name: 'North Korea', bbox: [37.5, 124.0, 43.0, 131.0] },
  // South Korea (far east edge, lon 126-130)
  { name: 'South Korea', bbox: [33.0, 126.0, 38.5, 130.0] },
  // Russia Far East (north of 50°N east of 115°E)
  { name: 'Russia Far East', bbox: [50.0, 115.0, 54.0, 135.5] },
  // Japan (far east edge, lon 128+)
  { name: 'Japan', bbox: [24.0, 129.0, 54.0, 135.5] },
  // Taiwan — skip CN politics, just exclude
  { name: 'Taiwan', bbox: [21.8, 119.5, 25.5, 122.1] },
]

// Tier-1 Chinese cities (×2.0 multiplier)
const TIER1_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Beijing', bbox: [39.7, 116.1, 40.2, 116.7] },
  { name: 'Shanghai', bbox: [31.0, 121.3, 31.5, 121.8] },
  { name: 'Guangzhou', bbox: [23.0, 113.1, 23.3, 113.6] },
  { name: 'Shenzhen', bbox: [22.4, 113.8, 22.8, 114.4] },
  { name: 'Chengdu', bbox: [30.5, 103.9, 30.8, 104.3] },
  { name: 'Chongqing', bbox: [29.4, 106.3, 29.7, 106.8] },
  { name: 'Wuhan', bbox: [30.4, 114.1, 30.8, 114.5] },
  { name: 'Xian', bbox: [34.1, 108.8, 34.4, 109.1] },
  { name: 'Hangzhou', bbox: [30.1, 120.0, 30.4, 120.4] },
  { name: 'Nanjing', bbox: [32.0, 118.6, 32.2, 119.0] },
  { name: 'Suzhou', bbox: [31.2, 120.5, 31.4, 120.8] },
  { name: 'Tianjin', bbox: [39.0, 117.1, 39.3, 117.4] },
]

// Tier-2 Chinese cities (×1.4 multiplier)
const TIER2_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Changsha', bbox: [28.1, 112.9, 28.3, 113.1] },
  { name: 'Qingdao', bbox: [36.0, 120.2, 36.2, 120.5] },
  { name: 'Dalian', bbox: [38.8, 121.5, 39.0, 121.8] },
  { name: 'Hefei', bbox: [31.7, 117.1, 31.9, 117.4] },
  { name: 'Zhengzhou', bbox: [34.6, 113.5, 34.9, 113.8] },
  { name: 'Jinan', bbox: [36.6, 116.9, 36.8, 117.1] },
  { name: 'Kunming', bbox: [24.9, 102.6, 25.1, 102.9] },
  { name: 'Fuzhou', bbox: [26.0, 119.2, 26.2, 119.4] },
  { name: 'Xiamen', bbox: [24.4, 118.0, 24.6, 118.2] },
  { name: 'Ningbo', bbox: [29.8, 121.4, 30.0, 121.7] },
  { name: 'Wuxi', bbox: [31.5, 120.2, 31.7, 120.4] },
  { name: 'Harbin', bbox: [45.6, 126.5, 45.9, 126.8] },
  { name: 'Shenyang', bbox: [41.7, 123.3, 42.0, 123.6] },
  { name: 'Nanchang', bbox: [28.6, 115.8, 28.8, 116.0] },
  { name: 'Hohhot', bbox: [40.7, 111.6, 40.9, 111.8] },
  { name: 'Urumqi', bbox: [43.7, 87.5, 43.9, 87.8] },
  { name: 'Lanzhou', bbox: [36.0, 103.7, 36.2, 103.9] },
  { name: 'Xining', bbox: [36.6, 101.7, 36.8, 101.9] },
  { name: 'Guiyang', bbox: [26.5, 106.6, 26.7, 106.8] },
  { name: 'Haikou', bbox: [20.0, 110.2, 20.1, 110.4] },
  { name: 'Nanning', bbox: [22.7, 108.2, 22.9, 108.5] },
  { name: 'Lhasa', bbox: [29.6, 91.0, 29.7, 91.2] },
  { name: 'Yinchuan', bbox: [38.4, 106.1, 38.6, 106.3] },
  { name: 'Taiyuan', bbox: [37.7, 112.4, 37.9, 112.6] },
  { name: 'Shijiazhuang', bbox: [38.0, 114.4, 38.2, 114.6] },
  { name: 'Dongguan', bbox: [22.9, 113.6, 23.1, 113.9] },
  { name: 'Foshan', bbox: [22.9, 113.0, 23.1, 113.2] },
  { name: 'Wenzhou', bbox: [27.9, 120.5, 28.1, 120.8] },
  { name: 'Shantou', bbox: [23.3, 116.6, 23.5, 116.8] },
  { name: 'Zhuhai', bbox: [22.2, 113.5, 22.4, 113.7] },
  { name: 'Zhongshan', bbox: [22.4, 113.3, 22.6, 113.5] },
  { name: 'Huizhou', bbox: [23.0, 114.3, 23.2, 114.5] },
  { name: 'Jinhua', bbox: [29.0, 119.5, 29.2, 119.8] },
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

// ── Load highway network ──

interface HwyFeat {
  coords: [number, number][]
  type: string
}

function loadHighways(): HwyFeat[] {
  const path = resolve(CACHE_DIR, 'highway-network.geojson')
  if (!existsSync(path)) throw new Error(`Missing ${path}`)
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: HwyFeat[] = []
  for (const f of fc.features || []) {
    const iso = (f.properties?.ISO_CC || '').trim()
    if (iso !== 'CN') continue  // filter out neighbour spillover
    const g = f.geometry
    if (!g) continue
    let coords: [number, number][] = []
    if (g.type === 'LineString') coords = g.coordinates
    else if (g.type === 'MultiLineString') coords = g.coordinates[0] || []
    if (coords.length < 2) continue
    out.push({ coords, type: (f.properties?.TYPE || '').trim() })
  }
  return out
}

function buildGrid(features: HwyFeat[]): Map<string, HwyFeat[]> {
  const grid = new Map<string, HwyFeat[]>()
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

function nearestHwy(midLat: number, midLon: number, grid: Map<string, HwyFeat[]>, radiusM: number): HwyFeat | null {
  const reach = Math.max(1, Math.ceil(radiusM / 1000))
  const baseLat = Math.floor(midLat * 100)
  const baseLon = Math.floor(midLon * 100)
  let best: HwyFeat | null = null
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

// ── AADT defaults ──

const HWY_AADT: Record<string, number> = {
  'Highway': 60000,
  'Major road': 25000,
  'Local road': 8000,
  'Ferry': 0,
}

function tierMultiplier(tier: 0 | 1 | 2): number {
  return tier === 1 ? 2.0 : tier === 2 ? 1.4 : 1.0
}

function splitVehicles(aadt: number, tier: 0 | 1 | 2): { light: number; medium: number; heavy: number; moto: number } {
  if (tier === 1 || tier === 2) {
    // Urban Chinese: cars dominant, trucks restricted daytime, few gas motos
    return {
      light: Math.round(aadt * 0.75),
      medium: Math.round(aadt * 0.10),
      heavy: Math.round(aadt * 0.10),
      moto: Math.round(aadt * 0.05),
    }
  }
  // Rural: more trucks (freight corridors)
  return {
    light: Math.round(aadt * 0.65),
    medium: Math.round(aadt * 0.12),
    heavy: Math.round(aadt * 0.18),
    moto: Math.round(aadt * 0.05),
  }
}

async function main() {
  console.log(`=== CN Roads Enrichment — Highway Network + Chinese-tuned CNOSSOS (${YEAR}) ===\n`)
  const hwys = loadHighways()
  console.log(`  Loaded CN highway network: ${hwys.length}`)
  const counts: Record<string, number> = {}
  for (const h of hwys) counts[h.type] = (counts[h.type] || 0) + 1
  for (const [k, v] of Object.entries(counts).sort((a, b) => b[1] - a[1])) console.log(`    ${k}: ${v}`)

  const grid = buildGrid(hwys)
  console.log(`  Grid cells: ${grid.size}\n`)

  const hexDirs = iterateCountryHexes(H3R4_DIR, CN_HEX_BBOX)
  console.log(`  CN-bbox hexes with roads.arrow: ${hexDirs.length}`)

  let totalRoads = 0, excluded = 0, alreadyEnriched = 0
  let matchedHwy = 0, matchedClass = 0
  let hexesUpdated = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        // Fast-exit if a higher-priority dataset owns the row (writeRoadAadt
        // re-checks the gate — this only saves the spatial match work).
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { alreadyEnriched++; return null }

        const midLat = row.midLat
        const midLon = row.midLon

        if (!inBbox(midLat, midLon, CN_HEX_BBOX)) return null
        if (inAnyZone(midLat, midLon)) { excluded++; return null }

        const tier = cityTier(midLat, midLon)
        const mult = tierMultiplier(tier)
        const cls = row.roadClass

        let aadt = 0
        // Tier 1: spatial match to Highway Network (class ≤ 2 only for speed)
        if (cls <= 2) {
          const near = nearestHwy(midLat, midLon, grid, 400)
          if (near && HWY_AADT[near.type]) {
            aadt = HWY_AADT[near.type] * mult
            matchedHwy++
          }
        }
        // Tier 2: class defaults
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
      console.log(`  [${(elapsed / 1000).toFixed(0)}s] ${hi + 1}/${hexDirs.length}, ${matchedHwy} hwy + ${matchedClass} class`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  Total roads scanned:        ${totalRoads.toLocaleString()}`)
  console.log(`  Already enriched (skip):    ${alreadyEnriched.toLocaleString()}`)
  console.log(`  Excluded (neighbours):      ${excluded.toLocaleString()}`)
  console.log(`  Matched by Highway Network: ${matchedHwy.toLocaleString()}`)
  console.log(`  Matched by class default:   ${matchedClass.toLocaleString()}`)
  const tot = matchedHwy + matchedClass
  console.log(`  Total enriched:             ${tot.toLocaleString()} (${(100 * tot / Math.max(totalRoads, 1)).toFixed(2)}%)`)
  console.log(`  Hexes updated:              ${hexesUpdated}/${hexDirs.length}`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
