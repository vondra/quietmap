/**
 * Enrich IN industrial.arrow with Living Atlas India datasets.
 *
 * Sources:
 *
 * 1. **Power Plants in India** — 1,459 plants with GPS, fuel, capacity, owner.
 *    URL: `livingatlas.esri.in/server/rest/services/India/Power_Plants/MapServer/0`
 *    Breakdown: Coal 251, Gas 68, Oil 16, Hydro 231, Nuclear (few), Wind 106,
 *    Solar 737, Biomass 50. Supplements WRI GPPD (which stopped updating in 2022
 *    and misses post-2022 solar megaprojects).
 *
 * 2. **Industrial Land Parks in India** — 4,924 industrial parks with CPCB
 *    pollution classification (Red / Orange / Green / White). Red = highly
 *    polluting (cement, chemicals, metallurgy), Green = clean industry.
 *    URL: `livingatlas.esri.in/server/rest/services/Industry/Industrial_Land_Park/MapServer/0`
 *    Breakdown: Red=641, Orange=645, Green=1,443, White=297
 *
 * 3. **Cement Plants in India 2024** — 341 plants (polygons, with capacity).
 *    URL: `livingatlas.esri.in/server/rest/services/Cement_Plants_in_India_2024/MapServer/0`
 *
 * This script writes `nace_4digit` + `source_id` directly into
 * `industrial.arrow` for:
 *   - Power plants → NACE 35 (electricity generation)
 *   - Cement plants → NACE 23 (manufacture of non-metallic mineral products)
 *   - Industrial parks → OSM `landuse=industrial` polygons get NACE proxy from
 *     the park's CPCB pollution class (Red→cement-like profile, Orange→chemical,
 *     Green→light manufacturing, White→service)
 *
 * Spatial match: 500m for power/cement plants, 1000m for industrial parks
 * (since parks are large polygons).
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-in.ts
 */

import { readFileSync, readdirSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { tableFromIPC, makeTable, vectorFromArray, Uint16 } from 'apache-arrow'
import { SOURCES_BY_KEY } from './lib/sources.js'
import { shouldOverwrite, withArrowWrite } from './lib/provenance.js'
import { cellToLatLng } from 'h3-js'
import { SOURCE_ID_GLOBAL_INDUSTRIAL_NATIONAL_MIX } from './lib/source-ids.generated.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { flatDistM, inBbox } from './lib/spatial.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/in`)

const IN_BBOX: [number, number, number, number] = [6.5, 68.0, 37.0, 98.0]
const EXCLUDE_ZONES: Array<[number, number, number, number]> = [
  [23.0, 68.0, 37.0, 74.0],   // Pakistan
  [28.0, 76.0, 37.0, 98.0],   // China
  [26.3, 80.0, 30.5, 88.3],   // Nepal
  [26.6, 88.7, 28.6, 92.2],   // Bhutan
  [20.5, 88.0, 26.8, 92.8],   // Bangladesh
  [9.0, 93.5, 28.5, 102.0],   // Myanmar
  [5.9, 79.5, 9.9, 82.0],     // Sri Lanka
]
function inExcluded(lat: number, lon: number): boolean {
  for (const b of EXCLUDE_ZONES) if (inBbox(lat, lon, b)) return true
  return false
}

// ── Load Living Atlas industrial sources ──

interface IndustrialSite {
  lat: number
  lon: number
  nace: string
  name: string
  source: string
}

function loadPowerPlants(): IndustrialSite[] {
  const path = resolve(CACHE_DIR, 'power-plants.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: IndustrialSite[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g || g.type !== 'Point') continue
    const [lon, lat] = g.coordinates
    if (!inBbox(lat, lon, IN_BBOX) || inExcluded(lat, lon)) continue
    out.push({
      lat, lon,
      nace: '35',  // Electricity, gas, steam, air conditioning supply
      name: f.properties?.power_plant || f.properties?.plant_name || 'Power Plant',
      source: 'Living Atlas India Power Plants',
    })
  }
  return out
}

function loadCementPlants(): IndustrialSite[] {
  const path = resolve(CACHE_DIR, 'cement-plants.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: IndustrialSite[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g) continue
    // Polygon — use centroid
    let lat = 0, lon = 0, n = 0
    if (g.type === 'Polygon') {
      for (const [x, y] of g.coordinates[0]) { lon += x; lat += y; n++ }
    } else if (g.type === 'Point') {
      [lon, lat] = g.coordinates
      n = 1
    } else if (g.type === 'MultiPolygon') {
      for (const [x, y] of g.coordinates[0][0]) { lon += x; lat += y; n++ }
    }
    if (n === 0) continue
    lon /= n; lat /= n
    if (!inBbox(lat, lon, IN_BBOX) || inExcluded(lat, lon)) continue
    out.push({
      lat, lon,
      nace: '23',  // Manufacture of other non-metallic mineral products (cement)
      name: f.properties?.plant_name || f.properties?.name || 'Cement Plant',
      source: 'Living Atlas India Cement Plants 2024',
    })
  }
  return out
}

function loadIndustrialParks(): IndustrialSite[] {
  const path = resolve(CACHE_DIR, 'industrial-parks.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: IndustrialSite[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g) continue
    let lat = 0, lon = 0, n = 0
    if (g.type === 'Point') { [lon, lat] = g.coordinates; n = 1 }
    else if (g.type === 'Polygon') {
      for (const [x, y] of g.coordinates[0]) { lon += x; lat += y; n++ }
    } else if (g.type === 'MultiPolygon') {
      for (const [x, y] of g.coordinates[0][0]) { lon += x; lat += y; n++ }
    }
    if (n === 0) continue
    lon /= n; lat /= n
    if (!inBbox(lat, lon, IN_BBOX) || inExcluded(lat, lon)) continue
    // Map pollution_cat to NACE
    const pollutionCat = (f.properties?.pollution_cat || '').toLowerCase()
    let nace = '25'  // default metal products (moderate)
    if (pollutionCat.includes('red')) nace = '24'   // basic metals (highly polluting)
    else if (pollutionCat.includes('orange')) nace = '20'  // chemicals
    else if (pollutionCat.includes('green')) nace = '13'   // textiles (light industry)
    else if (pollutionCat.includes('white')) nace = '62'   // service (IT parks etc.)
    out.push({
      lat, lon,
      nace,
      name: f.properties?.park_name || f.properties?.name || 'Industrial Park',
      source: `Living Atlas India Industrial Land Park (${pollutionCat})`,
    })
  }
  return out
}

// ── Match industrial sites to OSM industrial polygons ──

async function main() {
  // #31 round-2: this direct 330 writer must honour the same national-ownership
  // polygon as stampOneWinner — the shared id's bbox/exclusion gate alone can
  // still stamp a neighbour's rows (R9 cannot see 330: no country identity).
  const inInCountry = makeCountryGate('IN')
  console.log(`=== IN Industrial Enrichment — Living Atlas India (${YEAR}) ===\n`)

  const power = loadPowerPlants()
  const cement = loadCementPlants()
  const parks = loadIndustrialParks()
  console.log(`  Power plants: ${power.length}`)
  console.log(`  Cement plants: ${cement.length}`)
  console.log(`  Industrial parks: ${parks.length}`)

  // Combine all, prioritise in order: cement > power > parks (most specific first)
  const allSites = [...cement, ...power, ...parks]
  console.log(`  Total industrial sites: ${allSites.length}`)

  // Spatial grid
  const grid = new Map<string, IndustrialSite[]>()
  for (const s of allSites) {
    const key = `${Math.floor(s.lat * 10)}_${Math.floor(s.lon * 10)}`  // ~10 km cells
    if (!grid.has(key)) grid.set(key, [])
    grid.get(key)!.push(s)
  }

  const MY_SOURCE_ID = SOURCE_ID_GLOBAL_INDUSTRIAL_NATIONAL_MIX

  // Scan IN hexes industrial.arrow
  const allHexes = readdirSync(H3R4_DIR).filter(d => d.length === 15 && d.endsWith('ffffffff'))
  const hexDirs: string[] = []
  for (const hex of allHexes) {
    try {
      const [lat, lon] = cellToLatLng(hex)
      if (inBbox(lat, lon, IN_BBOX) && existsSync(resolve(H3R4_DIR, hex, 'industrial.arrow'))) hexDirs.push(hex)
    } catch {}
  }
  console.log(`  IN-bbox hexes with industrial.arrow: ${hexDirs.length}`)

  let totalSites = 0, matched = 0, newEntries = 0

  for (const hex of hexDirs) {
    const arrowPath = resolve(H3R4_DIR, hex, 'industrial.arrow')
    if (!existsSync(arrowPath)) continue
    try {
      await withArrowWrite(arrowPath, table => {
        const n = table.numRows
        if (n === 0) return table
        const osmId = table.getChild('osm_id')
        const centroidLat = table.getChild('centroid_lat') ?? table.getChild('lat')
        const centroidLon = table.getChild('centroid_lon') ?? table.getChild('lon')
        const existingNaceCol = table.getChild('nace_4digit')
        const existingDatasetIdCol = table.getChild('source_id')
        if (!osmId || !centroidLat || !centroidLon) return table
        const newNace = new Uint16Array(n)
        const newDatasetId = new Uint16Array(n)
        const existingSourceId = new Uint16Array(n)
        for (let j = 0; j < n; j++) {
          newNace[j] = (existingNaceCol?.get(j) as number) ?? 0
          existingSourceId[j] = (existingDatasetIdCol?.get(j) as number) ?? 0
          newDatasetId[j] = existingSourceId[j]
        }
        let anyChanged = false

        for (let i = 0; i < n; i++) {
          totalSites++
          const lat = centroidLat.get(i) as number
          const lon = centroidLon.get(i) as number
          if (lat == null || lon == null) continue
          if (!inBbox(lat, lon, IN_BBOX) || inExcluded(lat, lon) || !inInCountry(lat, lon)) continue

          // Find nearest industrial site within 1000m
          const baseLat = Math.floor(lat * 10)
          const baseLon = Math.floor(lon * 10)
          let best: IndustrialSite | null = null
          let bestDist = 1000
          for (let dy = -1; dy <= 1; dy++) {
            for (let dx = -1; dx <= 1; dx++) {
              const cell = grid.get(`${baseLat + dy}_${baseLon + dx}`)
              if (!cell) continue
              for (const s of cell) {
                const d = flatDistM(lat, lon, s.lat, s.lon)
                if (d < bestDist) { bestDist = d; best = s }
              }
            }
          }
          if (best) {
            // IndustrialSite.nace values here are 2-digit ('35', '23', etc.); pad to 6-digit.
            const nace6Raw = best.nace.length < 6 ? (best.nace + '0000').substring(0, 6) : best.nace
            const nace6 = parseInt(nace6Raw, 10) || 0
            const nace4 = Math.floor(nace6 / 100)
            const existingId = existingSourceId[i]
            if (shouldOverwrite(existingId, MY_SOURCE_ID)) {
              newNace[i] = nace4
              newDatasetId[i] = MY_SOURCE_ID
              if (existingId === 0) newEntries++
              matched++
              anyChanged = true
            }
          }
        }
        if (!anyChanged) return table
        const columns: Record<string, any> = {}
        for (const field of table.schema.fields) {
          if (field.name === 'nace_4digit' || field.name === 'source_id') continue
          columns[field.name] = table.getChild(field.name)!
        }
        columns['nace_4digit'] = vectorFromArray(newNace, new Uint16())
        columns['source_id'] = vectorFromArray(newDatasetId, new Uint16())
        return makeTable(columns)
      })
    } catch (e: any) {
      console.log(`  ${hex}: ${e.message.substring(0, 60)}`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  OSM industrial sites scanned: ${totalSites.toLocaleString()}`)
  console.log(`  Matched to Living Atlas:      ${matched.toLocaleString()}`)
  console.log(`  New/updated arrow rows:       ${newEntries.toLocaleString()}`)
  console.log(`\n=== Done ===`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
