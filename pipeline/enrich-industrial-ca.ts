/**
 * Enrich CA industrial.arrow with NRCan Canadian Wind Turbine Database.
 *
 * Source: NRCan CanmetENERGY
 *   ftp.cartes.canada.ca/pub/nrcan_rncan/Wind-energy_Energie-eolienne/wind_turbines_database/Wind_Turbine_Database_en.xlsx
 *   ~7,841 turbines with Project Name, Turbine Rated Capacity (kW), Rotor Diameter (m),
 *   Hub Height (m), Manufacturer, Model, Lat, Lon
 *
 * Sheet: 'WTD' (Wind Turbine Database)
 *
 * License: OGL-Canada (Open Government Licence)
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-ca.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-ca.ts --enrich-only
 */

import { readFileSync, writeFileSync, readdirSync, existsSync, mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { tableFromIPC, tableToIPC, makeTable, makeVector } from 'apache-arrow'
import { cellToLatLng } from 'h3-js'
import * as XLSX from 'xlsx'
import { haversineM } from './lib/spatial.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/ca`)
const CACHE_XLSX = resolve(CACHE_DIR, 'wind-turbines-en.xlsx')

const forceDownload = process.argv.includes('--force-download')
const enrichOnly = process.argv.includes('--enrich-only')

const NRCAN_URL = 'https://ftp.cartes.canada.ca/pub/nrcan_rncan/Wind-energy_Energie-eolienne/wind_turbines_database/Wind_Turbine_Database_en.xlsx'

// Canada bbox
const CA_BBOX: [number, number, number, number] = [41.5, -141.0, 84.0, -52.0]

interface Turbine {
  lat: number
  lon: number
  rated_power_kw: number
  hub_height_m: number
  rotor_diameter_m: number
}

async function downloadXlsx(): Promise<Turbine[]> {
  if (!forceDownload && !existsSync(CACHE_XLSX)) {
    if (enrichOnly) throw new Error('--enrich-only but XLSX not cached')
    mkdirSync(CACHE_DIR, { recursive: true })
    console.log(`  Downloading NRCan wind turbine XLSX...`)
    const res = await fetch(NRCAN_URL, { signal: AbortSignal.timeout(120_000) })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    writeFileSync(CACHE_XLSX, Buffer.from(await res.arrayBuffer()))
  } else {
    console.log(`  Using cached: ${CACHE_XLSX}`)
  }

  const buf = readFileSync(CACHE_XLSX)
  const wb = XLSX.read(buf, { type: 'buffer' })
  const sheet = wb.Sheets['WTD']
  if (!sheet) throw new Error('WTD sheet not found')
  const range = XLSX.utils.decode_range(sheet['!ref']!)

  // Column layout (verified):
  // C0: Province_Territory
  // C1: Project Name
  // C2: Total Project Capacity (MW)
  // C3: Turbine Identifier
  // C4: Turbine Number
  // C5: Number of Turbines
  // C6: Turbine Rated Capacity (kW)
  // C7: Rotor Diameter (m)
  // C8: Hub Height (m)
  // C9: Manufacturer
  // C10: Model
  // C11: Commissioning Date
  // C12: Latitude
  // C13: Longitude

  const turbines: Turbine[] = []
  for (let r = 1; r <= range.e.r; r++) {
    const get = (c: number) => sheet[XLSX.utils.encode_cell({ r, c })]?.v
    const num = (c: number) => {
      const v = get(c)
      if (typeof v === 'number') return v
      if (typeof v === 'string') {
        const n = parseFloat(v.replace(',', '.'))
        return isNaN(n) ? 0 : n
      }
      return 0
    }
    const lat = num(12)
    const lon = num(13)
    const cap = num(6)
    const rotor = num(7)
    const hub = num(8)
    if (!lat || !lon || cap <= 0) continue
    if (lat < CA_BBOX[0] || lat > CA_BBOX[2] || lon < CA_BBOX[1] || lon > CA_BBOX[3]) continue
    turbines.push({
      lat,
      lon,
      rated_power_kw: cap,
      hub_height_m: hub,
      rotor_diameter_m: rotor,
    })
  }
  console.log(`  Parsed ${turbines.length} CA wind turbines`)
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
  console.log(`=== CA Wind Turbine Enrichment — NRCan ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache:    ${CACHE_DIR}\n`)

  const turbines = await downloadXlsx()
  const meanCap = Math.round(turbines.reduce((s, t) => s + t.rated_power_kw, 0) / turbines.length)
  const withHub = turbines.filter(t => t.hub_height_m > 0).length
  console.log(`  Mean rated power: ${meanCap} kW`)
  console.log(`  With hub height: ${withHub}`)

  const grid = buildGrid(turbines)
  console.log(`  Grid cells: ${grid.size}`)

  const allHexes = readdirSync(H3R4_DIR).filter(d => d.length === 15 && d.endsWith('ffffffff'))
  const hexDirs: string[] = []
  for (const hex of allHexes) {
    try {
      const [lat, lon] = cellToLatLng(hex)
      if (lat >= CA_BBOX[0] && lat <= CA_BBOX[2] && lon >= CA_BBOX[1] && lon <= CA_BBOX[3]) {
        if (existsSync(resolve(H3R4_DIR, hex, 'industrial.arrow'))) hexDirs.push(hex)
      }
    } catch {}
  }
  console.log(`  CA hexes with industrial.arrow: ${hexDirs.length}\n`)

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
      let bestDist = 500 // CA wind farms in remote terrain — wider tolerance

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
      columns['hub_height'] = makeVector(hubHeights)
      columns['rated_power_kw'] = makeVector(ratedPowers)
      const enriched = makeTable(columns)
      writeFileSync(arrowPath, Buffer.from(tableToIPC(enriched, 'file')))
      hexesUpdated++
    }

    if (hi % 100 === 0 || hi === hexDirs.length - 1) {
      const elapsed = ((Date.now() - startTime) / 1000).toFixed(0)
      console.log(`  [${elapsed}s] ${hi + 1}/${hexDirs.length} hexes, ${hexesUpdated} updated, ${matched} matched`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  OSM wind turbines in CA hexes: ${totalTurbines}`)
  console.log(`  Matched to NRCan: ${matched} (${(100 * matched / Math.max(totalTurbines, 1)).toFixed(1)}%)`)
  console.log(`  Hexes updated: ${hexesUpdated}`)
  console.log(`\n=== Done ===`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
