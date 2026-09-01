/**
 * Enrich PH industrial with GEM Global Integrated Power (Philippines subset).
 *
 * Source: services.arcgis.com/lqRTrQp2HrfnJt8U/.../Global_Integrated_Power_
 * August_2025/FeatureServer/0?where=Country_area='Philippines'
 *
 * 995 PH plants total, 255 operating:
 *   - Coal: 147 (Masinloc, Sual, Pagbilao, Calaca, Mindanao plants)
 *   - Gas: 61 (Ilijan Batangas)
 *   - Oil: 17
 *   - Hydro: 63 (Angat, San Roque, Kalayaan, Pantabangan)
 *   - Geothermal: 65 — **2nd-largest globally** (Tiwi/Mak-Ban/Palinpinon/
 *                     Tongonan/Bacman)
 *   - Solar: 359, Wind: 271 (most in pre-construction), Bioenergy: 10
 *   - Nuclear: 2 (Bataan — mothballed)
 *
 * NACE 35 (Electricity generation) for all.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-ph.ts
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

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/ph`)

const PH_BBOX: [number, number, number, number] = [4.0, 115.8, 22.0, 127.5]
const EXCLUDE_ZONES: Array<[number, number, number, number]> = [
  [21.8, 119.5, 25.5, 122.1],  // Taiwan
  [4.0, 115.8, 7.4, 119.5],    // Malaysia Sabah
  [4.0, 124.0, 5.0, 127.5],    // Indonesia NE
]
function inExcluded(lat: number, lon: number): boolean {
  for (const b of EXCLUDE_ZONES) if (inBbox(lat, lon, b)) return true
  return false
}

interface IndSite {
  lat: number
  lon: number
  name: string
  fuel: string
}

function loadPowerPlants(): IndSite[] {
  const path = resolve(CACHE_DIR, 'power-plants.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: IndSite[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g || g.type !== 'Point') continue
    const [lon, lat] = g.coordinates
    if (lat == null || lon == null) continue
    if (!inBbox(lat, lon, PH_BBOX) || inExcluded(lat, lon)) continue
    const p = f.properties || {}
    const status = (p.Status || '').toString().toLowerCase()
    if (status && status !== 'operating') continue
    out.push({
      lat, lon,
      name: (p.Name || p.name || 'Philippines power plant').toString(),
      fuel: (p.Type || p.Fuel || 'unknown').toString(),
    })
  }
  return out
}

function loadEconomicZones(): IndSite[] {
  const path = resolve(CACHE_DIR, 'economic-zones.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: IndSite[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g) continue
    let lat = 0, lon = 0, n = 0
    if (g.type === 'Point') { [lon, lat] = g.coordinates; n = 1 }
    else if (g.type === 'Polygon') {
      for (const [x, y] of g.coordinates[0]) { lon += x; lat += y; n++ }
    }
    if (n === 0) continue
    lon /= n; lat /= n
    if (!inBbox(lat, lon, PH_BBOX) || inExcluded(lat, lon)) continue
    const p = f.properties || {}
    out.push({
      lat, lon,
      name: (p.Name || 'PEZA Economic Zone').toString(),
      fuel: (p.Zone_Class || 'Manufacturing Economic Zone').toString(),
    })
  }
  return out
}

async function main() {
  // #31 round-2: this direct 330 writer must honour the same national-ownership
  // polygon as stampOneWinner — the shared id's bbox/exclusion gate alone can
  // still stamp a neighbour's rows (R9 cannot see 330: no country identity).
  const inPhCountry = makeCountryGate('PH')
  console.log(`=== PH Industrial Enrichment — GEM + DTI Economic Zones (${YEAR}) ===\n`)

  const plants = loadPowerPlants()
  const zones = loadEconomicZones()
  console.log(`  Operating power plants: ${plants.length}`)
  console.log(`  Economic zones:         ${zones.length}`)

  // Power plants → NACE 35, zones → NACE 25 (general manufacturing)
  const allSites: Array<IndSite & { nace: string }> = [
    ...plants.map(s => ({ ...s, nace: '35' })),
    ...zones.map(s => ({ ...s, nace: '25' })),
  ]

  const grid = new Map<string, Array<IndSite & { nace: string }>>()
  for (const s of allSites) {
    const key = `${Math.floor(s.lat * 10)}_${Math.floor(s.lon * 10)}`
    if (!grid.has(key)) grid.set(key, [])
    grid.get(key)!.push(s)
  }

  const MY_SOURCE_ID = SOURCE_ID_GLOBAL_INDUSTRIAL_NATIONAL_MIX

  const allHexes = readdirSync(H3R4_DIR).filter(d => d.length === 15 && d.endsWith('ffffffff'))
  const hexDirs: string[] = []
  for (const hex of allHexes) {
    try {
      const [lat, lon] = cellToLatLng(hex)
      if (inBbox(lat, lon, PH_BBOX) && existsSync(resolve(H3R4_DIR, hex, 'industrial.arrow'))) hexDirs.push(hex)
    } catch {}
  }
  console.log(`  PH-bbox hexes with industrial.arrow: ${hexDirs.length}`)

  let totalOsm = 0, matched = 0, newEntries = 0

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
          totalOsm++
          const lat = centroidLat.get(i) as number
          const lon = centroidLon.get(i) as number
          if (lat == null || lon == null) continue
          if (!inBbox(lat, lon, PH_BBOX) || inExcluded(lat, lon) || !inPhCountry(lat, lon)) continue

          const baseLat = Math.floor(lat * 10)
          const baseLon = Math.floor(lon * 10)
          let best: (IndSite & { nace: string }) | null = null
          let bestDist = 1500
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
            // NACE values may be 2-digit ('07') or 6-digit ('351100'); pad to 6-digit.
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
    } catch {}
  }

  console.log(`\n=== Results ===`)
  console.log(`  OSM industrial sites scanned: ${totalOsm.toLocaleString()}`)
  console.log(`  Matched:                      ${matched.toLocaleString()}`)
  console.log(`  New/updated arrow rows:       ${newEntries.toLocaleString()}`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
