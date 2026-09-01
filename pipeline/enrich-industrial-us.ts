/**
 * Enrich US industrial.arrow with USWTDB wind turbine specs.
 *
 * Source: USWTDB (US Wind Turbine Database, USGS + LBNL)
 *   Pre-cached in the shared enrichment cache (global/uswtdb.csv)
 *   75,728 turbines with rated power (kW), hub height (m), rotor diameter (m)
 *
 * Already in pipeline globally but not applied per-hex to US OSM turbines.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-us.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-us.ts --enrich-only
 */

import { readFileSync, writeFileSync, readdirSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { tableFromIPC, tableToIPC, vectorFromArray, makeTable, Float32 } from 'apache-arrow'
import { cellToLatLng } from 'h3-js'
import { haversineM } from './lib/spatial.js'
import { H3R4_DIR } from './lib/data-year.js'

const USWTDB_CSV = resolve(import.meta.dirname, `../data/enrichment/global/uswtdb.csv`)

// US bbox
const US_BBOX: [number, number, number, number] = [17.5, -180.0, 71.5, -65.0]

interface Turbine {
  lat: number
  lon: number
  rated_power_kw: number
  hub_height_m: number
  rotor_diameter_m: number
}

function parseUswtdb(): Turbine[] {
  // CRLF line endings in upstream CSV
  const text = readFileSync(USWTDB_CSV, 'utf-8').replace(/\r/g, '')
  const lines = text.split('\n')
  // Header: case_id,faa_ors,...,xlong,ylat
  const header = lines[0].split(',')
  const iLon = header.indexOf('xlong')
  const iLat = header.indexOf('ylat')
  const iCap = header.indexOf('t_cap') // turbine capacity in kW
  const iHh = header.indexOf('t_hh')   // hub height in m
  const iRd = header.indexOf('t_rd')   // rotor diameter in m

  if (iLat < 0 || iLon < 0 || iCap < 0) {
    throw new Error(`USWTDB header missing required columns. Got: ${header.join(',')}`)
  }

  const turbines: Turbine[] = []
  for (let i = 1; i < lines.length; i++) {
    const cols = lines[i].split(',')
    if (cols.length < header.length) continue
    const lat = parseFloat(cols[iLat])
    const lon = parseFloat(cols[iLon])
    const cap = parseFloat(cols[iCap])
    if (isNaN(lat) || isNaN(lon) || isNaN(cap) || cap <= 0) continue
    turbines.push({
      lat,
      lon,
      rated_power_kw: cap,
      hub_height_m: parseFloat(cols[iHh]) || 0,
      rotor_diameter_m: parseFloat(cols[iRd]) || 0,
    })
  }
  return turbines
}

function buildGrid(turbines: Turbine[]): Map<string, Turbine[]> {
  const grid = new Map<string, Turbine[]>()
  for (const t of turbines) {
    const key = `${Math.floor(t.lat * 100)},${Math.floor(t.lon * 100)}`
    if (!grid.has(key)) grid.set(key, [])
    grid.get(key)!.push(t)
  }
  return grid
}

async function main() {
  console.log(`=== US Wind Turbine Enrichment — USWTDB ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  USWTDB CSV: ${USWTDB_CSV}\n`)

  if (!existsSync(USWTDB_CSV)) throw new Error(`USWTDB cache not found: ${USWTDB_CSV}`)

  const turbines = parseUswtdb()
  console.log(`  Parsed ${turbines.length} USWTDB turbines`)
  const meanCap = Math.round(turbines.reduce((s, t) => s + t.rated_power_kw, 0) / turbines.length)
  const withHh = turbines.filter(t => t.hub_height_m > 0).length
  console.log(`  Mean rated power: ${meanCap} kW`)
  console.log(`  With hub height: ${withHh}`)

  const grid = buildGrid(turbines)
  console.log(`  Grid cells: ${grid.size}`)

  const allHexes = readdirSync(H3R4_DIR).filter(d => d.length === 15 && d.endsWith('ffffffff'))
  const hexDirs: string[] = []
  for (const hex of allHexes) {
    try {
      const [lat, lon] = cellToLatLng(hex)
      if (lat >= US_BBOX[0] && lat <= US_BBOX[2] && lon >= US_BBOX[1] && lon <= US_BBOX[3]) {
        if (existsSync(resolve(H3R4_DIR, hex, 'industrial.arrow'))) hexDirs.push(hex)
      }
    } catch {}
  }
  console.log(`  US hexes with industrial.arrow: ${hexDirs.length}\n`)

  let totalTurbines = 0, matched = 0, hexesUpdated = 0
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

    let hexMatched = 0
    for (let i = 0; i < numRows; i++) {
      const st = sourceTypes.get(i) as number ?? 0
      if (st !== 10) continue
      totalTurbines++
      if (hubHeights[i] > 0 && ratedPowers[i] > 0) continue

      const lat = lats.get(i) as number ?? 0
      const lon = lons.get(i) as number ?? 0
      if (lat === 0 || lon === 0) continue

      const gy = Math.floor(lat * 100)
      const gx = Math.floor(lon * 100)
      let best: Turbine | null = null
      let bestDist = 500 // US wind farms in remote terrain — wider tolerance

      for (let dy = -1; dy <= 1; dy++) {
        for (let dx = -1; dx <= 1; dx++) {
          const cell = grid.get(`${gy + dy},${gx + dx}`)
          if (!cell) continue
          for (const t of cell) {
            const d = haversineM(lat, lon, t.lat, t.lon)
            if (d < bestDist) { bestDist = d; best = t }
          }
        }
      }

      if (best) {
        hubHeights[i] = best.hub_height_m
        ratedPowers[i] = best.rated_power_kw
        hexMatched++
        matched++
      }
    }

    if (hexMatched > 0) {
      const columns: Record<string, any> = {}
      for (const field of table.schema.fields) {
        if (field.name === 'hub_height' || field.name === 'rated_power_kw') continue
        columns[field.name] = table.getChild(field.name)!
      }
      columns['hub_height'] = vectorFromArray(hubHeights, new Float32())
      columns['rated_power_kw'] = vectorFromArray(ratedPowers, new Float32())
      const enriched = makeTable(columns)
      writeFileSync(arrowPath, Buffer.from(tableToIPC(enriched, 'file')))
      hexesUpdated++
    }

    if (hi % 200 === 0 || hi === hexDirs.length - 1) {
      const elapsed = ((Date.now() - startTime) / 1000).toFixed(0)
      console.log(`  [${elapsed}s] ${hi + 1}/${hexDirs.length} hexes, ${hexesUpdated} updated, ${matched.toLocaleString()} matched`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  OSM wind turbines in US hexes: ${totalTurbines.toLocaleString()}`)
  console.log(`  Matched to USWTDB: ${matched.toLocaleString()} (${(100 * matched / Math.max(totalTurbines, 1)).toFixed(1)}%)`)
  console.log(`  Hexes updated: ${hexesUpdated}`)
  console.log(`\n=== Done ===`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
