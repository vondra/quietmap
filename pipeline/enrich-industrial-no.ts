/**
 * Enrich NO industrial.arrow with NVE Vindkraft2 wind turbine data.
 *
 * Source: NVE (Norges vassdrags- og energidirektorat) ArcGIS REST
 *   Layer 4 — Vindturbin (individual turbines, ~1,381 points)
 *   Layer 0 — Vindkraftverk (parks, 61 entries) with effekt_MW_idrift + antallTurbiner
 *
 * Per-turbine power = park_total_MW / number_of_turbines (NVE doesn't publish
 * individual turbine specs, only park totals).
 *
 * License: NLOD 2.0
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-no.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-no.ts --enrich-only
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-no.ts --force-download
 */

import { readFileSync, writeFileSync, readdirSync, existsSync, mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { tableFromIPC, tableToIPC, makeTable, makeVector } from 'apache-arrow'
import { cellToLatLng } from 'h3-js'
import { buildRegistryGrid, findNearestRegistryRecord, fillMissingTurbineSpecs } from './lib/wind-registry-match.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/no`)
const CACHE_TURBINES = resolve(CACHE_DIR, 'nve-vindturbiner.json')
const CACHE_PARKS = resolve(CACHE_DIR, 'nve-vindkraftverk.json')

/** How far an OSM wind turbine may sit from its NVE record. Norwegian wind
 *  farms sit in mountainous terrain where the OSM node is often the pad
 *  rather than the surveyed mast — wider than the 200 m registers. */
const REGISTRY_MATCH_RADIUS_M = 500

const forceDownload = process.argv.includes('--force-download')
const enrichOnly = process.argv.includes('--enrich-only')

const NVE_BASE = 'https://nve.geodataonline.no/arcgis/rest/services/Vindkraft2/MapServer'

interface ParkRecord {
  anleggsNr: number
  name: string
  effekt_MW_idrift: number
  antallTurbiner: number
  status: string
  per_turbine_kw: number
}

interface TurbineRecord {
  anleggsNr: number
  lat: number
  lon: number
  status: string
}

async function fetchAllPaged(layerId: number, label: string, cache: string): Promise<any[]> {
  if (!forceDownload && existsSync(cache)) {
    console.log(`  Using cached ${label}: ${cache}`)
    return JSON.parse(readFileSync(cache, 'utf-8'))
  }
  if (enrichOnly) throw new Error(`--enrich-only but ${label} not cached`)

  console.log(`  Fetching ${label} from NVE layer ${layerId}...`)
  const all: any[] = []
  let offset = 0
  const PAGE = 1000

  while (true) {
    const url = `${NVE_BASE}/${layerId}/query?where=1%3D1&outFields=*&resultRecordCount=${PAGE}&resultOffset=${offset}&f=geojson&outSR=4326`
    const res = await fetch(url, { signal: AbortSignal.timeout(120_000) })
    if (!res.ok) throw new Error(`HTTP ${res.status} for layer ${layerId} offset=${offset}`)
    const data: any = await res.json()
    const feats = data.features || []
    all.push(...feats)
    console.log(`    offset=${offset}: ${feats.length} features (${all.length} total)`)
    if (feats.length < PAGE || !data.exceededTransferLimit) break
    offset += PAGE
  }

  mkdirSync(CACHE_DIR, { recursive: true })
  writeFileSync(cache, JSON.stringify(all))
  console.log(`  Cached ${all.length} ${label} records`)
  return all
}

async function main() {
  console.log(`=== NO Wind Turbine Enrichment — NVE Vindkraft2 ArcGIS ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache:    ${CACHE_DIR}\n`)

  // Step 1: parks layer (effekt + antallTurbiner)
  const parksRaw = await fetchAllPaged(0, 'parks', CACHE_PARKS)
  const parkMap = new Map<number, ParkRecord>()
  for (const f of parksRaw) {
    const p = f.properties || {}
    const anleggsNr = p.anleggsNr
    const status = (p.status || '').toString()
    if (!anleggsNr) continue
    if (status !== 'D') continue // operational only

    const effekt = parseFloat(p.effekt_MW_idrift || p.effekt_MW || '0')
    const count = parseInt(p.antallTurbiner || '0')
    if (effekt <= 0 || count <= 0) continue

    parkMap.set(anleggsNr, {
      anleggsNr,
      name: (p.anleggNavn || '').toString(),
      effekt_MW_idrift: effekt,
      antallTurbiner: count,
      status,
      per_turbine_kw: (effekt / count) * 1000, // MW → kW
    })
  }
  console.log(`  Operational parks: ${parkMap.size}`)
  const totalMW = [...parkMap.values()].reduce((s, p) => s + p.effekt_MW_idrift, 0)
  console.log(`  Total operational capacity: ${totalMW.toFixed(0)} MW`)

  // Step 2: turbine layer
  const turbinesRaw = await fetchAllPaged(4, 'turbines', CACHE_TURBINES)
  const turbines: TurbineRecord[] = []
  for (const f of turbinesRaw) {
    const p = f.properties || {}
    const status = (p.status || '').toString()
    if (status !== 'D') continue // operational only
    const anleggsNr = p.anleggsNr
    if (!anleggsNr) continue
    const coords = f.geometry?.coordinates
    if (!coords) continue
    const [lon, lat] = coords as [number, number]
    if (!lat || !lon) continue
    turbines.push({ anleggsNr, lat, lon, status })
  }
  console.log(`  Operational turbines: ${turbines.length}`)

  // Only a turbine whose park is known can fill a spec; indexing just those is
  // the same "nearest turbine that has a park" the match loop used to compute.
  const grid = buildRegistryGrid(turbines.filter((t) => parkMap.has(t.anleggsNr)))

  // Pre-filter NO hexes
  const allHexes = readdirSync(H3R4_DIR).filter(d => d.length === 15 && d.endsWith('ffffffff'))
  const hexDirs: string[] = []
  for (const hex of allHexes) {
    try {
      const [lat, lon] = cellToLatLng(hex)
      if (lat >= 57.9 && lat <= 71.2 && lon >= 4.5 && lon <= 31.2) {
        if (existsSync(resolve(H3R4_DIR, hex, 'industrial.arrow'))) hexDirs.push(hex)
      }
    } catch {}
  }
  console.log(`  Norwegian hexes with industrial.arrow: ${hexDirs.length}\n`)

  let totalTurbines = 0, filled = 0, hexesUpdated = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const arrowPath = resolve(H3R4_DIR, hex, 'industrial.arrow')
    const buf = readFileSync(arrowPath)
    const table = tableFromIPC(buf)
    const numRows = table.numRows
    if (numRows === 0) continue

    const sourceTypes = table.getChild('source_type')
    const lats = table.getChild('centroid_lat')
    const lons = table.getChild('centroid_lon')
    if (!sourceTypes || !lats || !lons) continue

    const existingHub = table.getChild('hub_height')
    const existingPower = table.getChild('rated_power_kw')

    const hubHeights = new Float32Array(numRows)
    const ratedPowers = new Float32Array(numRows)
    for (let i = 0; i < numRows; i++) {
      hubHeights[i] = (existingHub?.get(i) as number) ?? 0
      ratedPowers[i] = (existingPower?.get(i) as number) ?? 0
    }

    let hexFilled = 0
    for (let i = 0; i < numRows; i++) {
      const st = sourceTypes.get(i) as number ?? 0
      if (st !== 10) continue
      totalTurbines++
      if (ratedPowers[i] > 0) continue

      const lat = lats.get(i) as number ?? 0
      const lon = lons.get(i) as number ?? 0
      if (lat === 0 || lon === 0) continue

      const nearest = findNearestRegistryRecord(grid, lat, lon, REGISTRY_MATCH_RADIUS_M)
      const park = nearest ? parkMap.get(nearest.anleggsNr) : undefined
      // NVE publishes no hub height — 0 leaves the row's own value alone.
      if (park && fillMissingTurbineSpecs(hubHeights, ratedPowers, i, 0, park.per_turbine_kw)) {
        hexFilled++
        filled++
      }
    }

    if (hexFilled > 0) {
      const columns: Record<string, any> = {}
      for (const field of table.schema.fields) {
        if (field.name === 'hub_height' || field.name === 'rated_power_kw') continue
        columns[field.name] = table.getChild(field.name)!
      }
      columns['hub_height'] = makeVector(hubHeights)
      columns['rated_power_kw'] = makeVector(ratedPowers)
      const enriched = makeTable(columns)
      writeFileSync(arrowPath, Buffer.from(tableToIPC(enriched, 'file')))
      hexesUpdated++
    }

    if (hi % 50 === 0 || hi === hexDirs.length - 1) {
      const elapsed = ((Date.now() - startTime) / 1000).toFixed(0)
      console.log(`  [${elapsed}s] ${hi + 1}/${hexDirs.length} hexes, ${hexesUpdated} updated, ${filled} filled`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  OSM wind turbines in NO hexes: ${totalTurbines}`)
  console.log(`  Specs filled from NVE: ${filled} (${(100 * filled / Math.max(totalTurbines, 1)).toFixed(1)}%)`)
  console.log(`  Hexes updated: ${hexesUpdated}`)
  console.log(`\n=== Done ===`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
