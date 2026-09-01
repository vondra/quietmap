/**
 * Enrich IN roads.arrow with Bharatmala road classification (Esri Living Atlas India).
 *
 * No per-segment AADT is published openly for India. NHAI and MoRTH traffic
 * census data is PDF-only; the official NHAI / MoRTH / data.gov.in portals
 * are WAF-blocked for programmatic access.
 *
 * **The workaround**: Esri Living Atlas India (`livingatlas.esri.in`) is a
 * public ArcGIS Server mirroring official Indian government datasets —
 * bypassing the NHAI/MoRTH WAF. 309+ public feature services under
 * `esri_IN_content` ownership, all anonymously accessible.
 *
 * Sources used:
 *
 * 1. **Bharatmala Road Network** — 28,372 polyline segments with Indian road
 *    classification. Fields: `rdtype` (Expressway / National Highway / State
 *    Highway / Ring Road), `rdname1`, `rdcodeprm`, `state`, `roadlength`.
 *    URL: `livingatlas.esri.in/server/rest/services/Road_Network/Road_Centerline_Bharatmala/MapServer/0`
 *    Breakdown: Expressway=486, National Highway=16,525, State Highway=11,356,
 *    Ring Road=5
 *
 * 2. **India Toll Plazas** — 1,766 point locations with `nh_no`, `plaza_name`,
 *    state, and per-axle toll rates. Used for NH ref-based matching only.
 *
 * Strategy: for each OSM road segment, find the nearest Bharatmala polyline
 * within 500m. Apply an AADT default from Indian road class multiplied by an
 * urban/rural multiplier based on segment location:
 *
 *   | Bharatmala rdtype | Rural AADT | Tier-1 city ×2.0 | Tier-2 city ×1.3 |
 *   |---|---:|---:|---:|
 *   | Expressway        | 80,000  | 160,000 | 104,000 |
 *   | National Highway  | 35,000  | 70,000  | 45,500  |
 *   | State Highway     | 15,000  | 30,000  | 19,500  |
 *   | Ring Road         | 50,000  | 100,000 | 65,000  |
 *
 * Fallback: CNOSSOS class defaults tuned for India (2-3× EU; motorcycle-heavy
 * fleet mix reflecting Indian urban traffic).
 *
 *   | road_class | Rural AADT | Tier-1 city ×2.0 | Tier-2 city ×1.3 |
 *   |---|---:|---:|---:|
 *   | 0 motorway  | 70,000 | 140,000 | 91,000 |
 *   | 1 trunk     | 30,000 | 60,000  | 39,000 |
 *   | 2 primary   | 12,000 | 24,000  | 15,600 |
 *   | 3 secondary | 5,000  | 10,000  | 6,500  |
 *   | 4 tertiary  | 2,000  | 4,000   | 2,600  |
 *   | 5 residential | 1,000 | 2,000  | 1,300  |
 *
 * Indian vehicle split (motorcycle-dominated):
 *   urban: light 45% / medium 8% / heavy 7% / moto 40%
 *   rural: light 55% / medium 10% / heavy 15% / moto 20%
 *
 * Indian cities are enormous — 8 Tier-1 metros (Delhi/Mumbai/Bangalore/
 * Hyderabad/Chennai/Kolkata/Ahmedabad/Pune) + ~30 Tier-2 cities. Bbox multiplier
 * lifts class-default AADT to peak urban values.
 *
 * Exclusion zones for neighbours inside IN bbox: Pakistan, Nepal, Bhutan,
 * Bangladesh, Myanmar, Sri Lanka, China/Tibet.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-in.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-in.ts --force-download
 */

import { readFileSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_IN_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_IN_NATIONAL_ROADS

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/in`)

// IN coarse bbox
const IN_BBOX: [number, number, number, number] = [6.5, 68.0, 37.0, 98.0]

// Exclusion zones for neighbour countries inside IN bbox.
// Tight bboxes to avoid clipping Delhi (28.47°N, Pakistan border at 74°E is 280 km west)
// and Kolkata (22.83°N 88.49°E, Bangladesh border at 88.85°E is 40 km east).
const EXCLUDE_ZONES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Pakistan', bbox: [23.0, 68.0, 37.0, 73.8] },
  { name: 'China', bbox: [30.5, 76.0, 37.0, 98.0] },
  { name: 'Nepal', bbox: [26.5, 80.1, 30.5, 88.2] },
  { name: 'Bhutan', bbox: [26.7, 88.8, 28.3, 92.1] },
  { name: 'Bangladesh', bbox: [20.5, 88.8, 26.7, 92.8] },
  { name: 'Myanmar', bbox: [9.5, 93.6, 28.5, 102.0] },
  { name: 'Sri Lanka', bbox: [5.9, 79.5, 9.9, 82.0] },
]

// Tier-1 Indian cities (bbox lookup)
const TIER1_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Delhi NCR', bbox: [28.3, 76.8, 29.2, 77.6] },
  { name: 'Mumbai', bbox: [18.9, 72.7, 19.3, 73.0] },
  { name: 'Bangalore', bbox: [12.8, 77.4, 13.2, 77.8] },
  { name: 'Hyderabad', bbox: [17.2, 78.2, 17.6, 78.7] },
  { name: 'Chennai', bbox: [12.8, 80.1, 13.2, 80.4] },
  { name: 'Kolkata', bbox: [22.3, 88.2, 22.8, 88.6] },
  { name: 'Ahmedabad', bbox: [22.9, 72.4, 23.2, 72.8] },
  { name: 'Pune', bbox: [18.4, 73.7, 18.7, 74.0] },
]

// Tier-2 Indian cities
const TIER2_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Jaipur', bbox: [26.8, 75.7, 27.0, 76.0] },
  { name: 'Lucknow', bbox: [26.75, 80.8, 27.0, 81.1] },
  { name: 'Kanpur', bbox: [26.35, 80.25, 26.55, 80.45] },
  { name: 'Nagpur', bbox: [21.05, 79.0, 21.25, 79.2] },
  { name: 'Indore', bbox: [22.6, 75.7, 22.8, 75.95] },
  { name: 'Thane', bbox: [19.1, 72.9, 19.3, 73.1] },
  { name: 'Bhopal', bbox: [23.1, 77.3, 23.3, 77.55] },
  { name: 'Visakhapatnam', bbox: [17.6, 83.1, 17.8, 83.4] },
  { name: 'Patna', bbox: [25.55, 85.0, 25.7, 85.2] },
  { name: 'Vadodara', bbox: [22.25, 73.1, 22.4, 73.3] },
  { name: 'Ghaziabad', bbox: [28.6, 77.35, 28.75, 77.55] },
  { name: 'Ludhiana', bbox: [30.85, 75.7, 31.0, 75.95] },
  { name: 'Agra', bbox: [27.1, 77.95, 27.25, 78.15] },
  { name: 'Nashik', bbox: [19.95, 73.7, 20.1, 73.9] },
  { name: 'Faridabad', bbox: [28.35, 77.25, 28.5, 77.45] },
  { name: 'Meerut', bbox: [28.95, 77.65, 29.1, 77.85] },
  { name: 'Rajkot', bbox: [22.25, 70.7, 22.4, 70.9] },
  { name: 'Kalyan', bbox: [19.2, 73.1, 19.35, 73.25] },
  { name: 'Varanasi', bbox: [25.25, 82.95, 25.4, 83.1] },
  { name: 'Srinagar', bbox: [34.05, 74.75, 34.2, 74.9] },
  { name: 'Amritsar', bbox: [31.6, 74.8, 31.75, 74.95] },
  { name: 'Allahabad/Prayagraj', bbox: [25.4, 81.8, 25.55, 81.95] },
  { name: 'Ranchi', bbox: [23.3, 85.25, 23.45, 85.4] },
  { name: 'Howrah', bbox: [22.55, 88.25, 22.7, 88.4] },
  { name: 'Coimbatore', bbox: [10.95, 76.9, 11.1, 77.1] },
  { name: 'Jabalpur', bbox: [23.1, 79.9, 23.25, 80.05] },
  { name: 'Gwalior', bbox: [26.15, 78.15, 26.3, 78.3] },
  { name: 'Vijayawada', bbox: [16.45, 80.55, 16.6, 80.75] },
  { name: 'Jodhpur', bbox: [26.2, 72.9, 26.35, 73.1] },
  { name: 'Madurai', bbox: [9.85, 78.1, 10.0, 78.25] },
  { name: 'Kochi', bbox: [9.9, 76.2, 10.05, 76.35] },
  { name: 'Thiruvananthapuram', bbox: [8.45, 76.85, 8.6, 77.0] },
  { name: 'Surat', bbox: [21.1, 72.75, 21.3, 72.95] },
]

function inAnyZone(lat: number, lon: number): boolean {
  for (const z of EXCLUDE_ZONES) if (inBbox(lat, lon, z.bbox)) return true
  return false
}
function cityTier(lat: number, lon: number): 1 | 2 | 0 {
  for (const c of TIER1_CITIES) if (inBbox(lat, lon, c.bbox)) return 1
  for (const c of TIER2_CITIES) if (inBbox(lat, lon, c.bbox)) return 2
  return 0
}

// ── Load Bharatmala + build spatial grid ──

interface BhFeat {
  coords: [number, number][]
  rdtype: string
}

function loadBharatmala(): BhFeat[] {
  const path = resolve(CACHE_DIR, 'bharatmala-roads.geojson')
  if (!existsSync(path)) throw new Error(`Missing ${path}`)
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: BhFeat[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g) continue
    let coords: [number, number][] = []
    if (g.type === 'LineString') coords = g.coordinates
    else if (g.type === 'MultiLineString') coords = g.coordinates[0] || []
    if (coords.length < 2) continue
    out.push({ coords, rdtype: (f.properties?.rdtype || '').trim() })
  }
  return out
}

function buildGrid(features: BhFeat[]): Map<string, BhFeat[]> {
  const grid = new Map<string, BhFeat[]>()
  for (const feat of features) {
    const seen = new Set<string>()
    for (const [lon, lat] of feat.coords) {
      const key = `${Math.floor(lat * 100)}_${Math.floor(lon * 100)}`  // ~1 km cells
      if (!seen.has(key)) {
        seen.add(key)
        if (!grid.has(key)) grid.set(key, [])
        grid.get(key)!.push(feat)
      }
    }
  }
  return grid
}

function nearestBharatmala(midLat: number, midLon: number, grid: Map<string, BhFeat[]>, radiusM: number): { feat: BhFeat; dist: number } | null {
  const reach = Math.max(1, Math.ceil(radiusM / 1000))
  const baseLat = Math.floor(midLat * 100)
  const baseLon = Math.floor(midLon * 100)
  let best: BhFeat | null = null
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

// ── AADT defaults ──

const BHARATMALA_AADT: Record<string, number> = {
  'Expressway': 80000,
  'National Highway': 35000,
  'State Highway': 15000,
  'Ring Road': 50000,
}

function tierMultiplier(tier: 0 | 1 | 2): number {
  return tier === 1 ? 2.0 : tier === 2 ? 1.3 : 1.0
}

function splitVehicles(aadt: number, tier: 0 | 1 | 2): { light: number; medium: number; heavy: number; moto: number } {
  if (tier === 1) {
    // Tier-1 Indian metro: motorcycle-dominated
    return {
      light: Math.round(aadt * 0.45),
      medium: Math.round(aadt * 0.08),
      heavy: Math.round(aadt * 0.07),
      moto: Math.round(aadt * 0.40),
    }
  }
  if (tier === 2) {
    return {
      light: Math.round(aadt * 0.50),
      medium: Math.round(aadt * 0.09),
      heavy: Math.round(aadt * 0.10),
      moto: Math.round(aadt * 0.31),
    }
  }
  // Rural
  return {
    light: Math.round(aadt * 0.55),
    medium: Math.round(aadt * 0.10),
    heavy: Math.round(aadt * 0.15),
    moto: Math.round(aadt * 0.20),
  }
}

async function main() {
  console.log(`=== IN Roads Enrichment — Bharatmala + Indian-tuned CNOSSOS (${YEAR}) ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache:    ${CACHE_DIR}\n`)

  const bh = loadBharatmala()
  console.log(`  Loaded Bharatmala: ${bh.length} polylines`)
  // count by rdtype
  const counts: Record<string, number> = {}
  for (const f of bh) counts[f.rdtype] = (counts[f.rdtype] || 0) + 1
  for (const [k, v] of Object.entries(counts).sort((a, b) => b[1] - a[1])) console.log(`    ${k}: ${v}`)

  const grid = buildGrid(bh)
  console.log(`  Bharatmala grid cells: ${grid.size}\n`)

  // IN bbox doubles as the hex-scan bbox: iterateCountryHexes filters hexes by
  // centroid-in-bbox; the per-row inBbox/exclusion-zone gate below still runs.
  const hexDirs = iterateCountryHexes(H3R4_DIR, IN_BBOX)
  console.log(`  IN-bbox hexes with roads.arrow: ${hexDirs.length}`)

  let totalRoads = 0, excluded = 0, alreadyEnriched = 0
  let matchedBharatmala = 0
  const matchedClassDefault = 0  // class-default tier never applies (unmatched → source_id=0)
  let hexesUpdated = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hexId = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hexId, 'roads.arrow'),
      (row) => {
        // Fast-exit before the expensive Bharatmala match when a higher-priority
        // dataset already owns the row (writeRoadAadt re-checks the gate).
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { alreadyEnriched++; return null }

        const midLat = row.midLat
        const midLon = row.midLon

        if (!inBbox(midLat, midLon, IN_BBOX)) return null
        if (inAnyZone(midLat, midLon)) { excluded++; return null }

        const tier = cityTier(midLat, midLon) as 0 | 1 | 2
        const mult = tierMultiplier(tier)
        const cls = row.roadClass

        let aadt = 0
        let matchedBy: 'bh' | 'class' | null = null

        // Tier 1: Bharatmala spatial match — only for higher-class roads (0-2)
        // Lower-class roads (residential/tertiary) get class defaults directly for speed.
        if (cls <= 2) {
          const near = nearestBharatmala(midLat, midLon, grid, 300)
          if (near && BHARATMALA_AADT[near.feat.rdtype]) {
            aadt = BHARATMALA_AADT[near.feat.rdtype] * mult
            matchedBy = 'bh'
            matchedBharatmala++
          }
        }

        // Tier 2: Indian class defaults
        if (!matchedBy) return null  // A.1: unmatched → source_id=0 → engine country-tier cascade

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
      console.log(`  [${(elapsed / 1000).toFixed(0)}s] ${hi + 1}/${hexDirs.length} hexes, ${matchedBharatmala} BH + ${matchedClassDefault} class`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  Total roads scanned:        ${totalRoads.toLocaleString()}`)
  console.log(`  Already enriched (skip):    ${alreadyEnriched.toLocaleString()}`)
  console.log(`  Excluded (neighbours):      ${excluded.toLocaleString()}`)
  console.log(`  Matched by Bharatmala:      ${matchedBharatmala.toLocaleString()}`)
  console.log(`  Matched by class default:   ${matchedClassDefault.toLocaleString()}`)
  const tot = matchedBharatmala + matchedClassDefault
  console.log(`  Total enriched:             ${tot.toLocaleString()} (${(100 * tot / Math.max(totalRoads, 1)).toFixed(2)}%)`)
  console.log(`  Hexes updated:              ${hexesUpdated}/${hexDirs.length}`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
