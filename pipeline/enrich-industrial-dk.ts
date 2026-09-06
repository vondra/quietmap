/**
 * Enrich DK industrial.arrow with Energistyrelsen wind turbine master register.
 *
 * Source: ens.dk/media/7828/download (XLSX, ~5.7 MB)
 *   "Stamdataregister for vindmøller" — Danish Energy Agency master register
 *   ~10,680 turbines (active + decommissioned since 1977)
 *   Per-turbine: GSRN id, capacity (kW), rotor diameter (m), hub height (m),
 *   manufacturer, model, X/Y UTM32N coordinates, on/offshore type
 *
 * Updated monthly. License: open data CC BY 4.0 (basisdata.dk).
 *
 * Filters to active turbines (no decommissioning date) and matches OSM source_type=10
 * by proximity (<200m), writes hub_height + rated_power_kw columns.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-dk.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-dk.ts --enrich-only
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-dk.ts --force-download
 */

import { readFileSync, writeFileSync, readdirSync, existsSync, mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { tableFromIPC, tableToIPC, makeTable, makeVector } from 'apache-arrow'
import { cellToLatLng } from 'h3-js'
import * as XLSX from 'xlsx'
import proj4 from 'proj4'
import { buildRegistryGrid, findNearestRegistryRecord, fillMissingTurbineSpecs } from './lib/wind-registry-match.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/dk`)
const CACHE_XLSX = resolve(CACHE_DIR, 'ens-windturbines.xlsx')

/** How far an OSM wind turbine may sit from its ENS record. The register
 *  publishes the mast position, so node and record land within a rotor's
 *  reach of each other. */
const REGISTRY_MATCH_RADIUS_M = 200

const forceDownload = process.argv.includes('--force-download')
const enrichOnly = process.argv.includes('--enrich-only')

const ENS_URL = 'https://ens.dk/media/7828/download'

// EPSG:25832 ETRS89/UTM32N → WGS84
proj4.defs('EPSG:25832', '+proj=utm +zone=32 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs')

interface WindTurbine {
  gsrn: string
  lat: number
  lon: number
  hub_height_m: number
  rated_power_kw: number
  rotor_diameter_m: number
  placement: string  // LAND / HAV
  manufacturer: string
}

async function downloadXlsx(): Promise<WindTurbine[]> {
  if (!forceDownload && !existsSync(CACHE_XLSX)) {
    if (enrichOnly) throw new Error('--enrich-only but ENS XLSX not cached')
    mkdirSync(CACHE_DIR, { recursive: true })
    console.log(`  Downloading Energistyrelsen wind turbine XLSX...`)
    const res = await fetch(ENS_URL, { signal: AbortSignal.timeout(60_000) })
    if (!res.ok) throw new Error(`HTTP ${res.status} for ENS`)
    const buf = Buffer.from(await res.arrayBuffer())
    writeFileSync(CACHE_XLSX, buf)
    console.log(`  Cached: ${(buf.length / 1e6).toFixed(2)} MB`)
  } else {
    console.log(`  Using cached: ${CACHE_XLSX}`)
  }

  const buf = readFileSync(CACHE_XLSX)
  const wb = XLSX.read(buf, { type: 'buffer' })
  const sheet = wb.Sheets['Vindmølledata']
  if (!sheet) throw new Error('Vindmølledata sheet not found')
  const range = XLSX.utils.decode_range(sheet['!ref']!)

  // Find data start row (first row where col 0 has a numeric/digit string)
  let dataStart = -1
  for (let r = 0; r < 30; r++) {
    const cell = sheet[XLSX.utils.encode_cell({ r, c: 0 })]
    if (cell && (typeof cell.v === 'number' || (typeof cell.v === 'string' && /^\d/.test(cell.v)))) {
      dataStart = r
      break
    }
  }
  if (dataStart < 0) throw new Error('Could not find data start row')

  // Column layout (verified):
  // C0: Møllenummer GSRN
  // C1: Date original grid connection (Excel date)
  // C2: Date decommissioning (Excel date — empty if active)
  // C3: Capacity (kW)
  // C4: Rotor diameter (m)
  // C5: Hub height (m) — Navhøjde
  // C6: Manufacturer (Fabrikat)
  // C7: Type designation
  // C8: Municipality
  // C9: LAND / HAV
  // C12: X UTM32N
  // C13: Y UTM32N

  const turbines: WindTurbine[] = []
  let skippedDecom = 0
  let skippedNoCoords = 0
  let skippedNoSpecs = 0

  for (let r = dataStart; r <= range.e.r; r++) {
    const get = (c: number) => sheet[XLSX.utils.encode_cell({ r, c })]?.v
    const num = (c: number) => {
      const v = get(c)
      if (typeof v === 'number') return v
      if (typeof v === 'string') {
        const cleaned = v.replace(',', '.').trim()
        if (!cleaned) return 0
        const n = parseFloat(cleaned)
        return isNaN(n) ? 0 : n
      }
      return 0
    }

    const gsrn = (get(0) || '').toString()
    if (!gsrn) continue

    // Filter: must be active (no decommissioning date)
    const decom = get(2)
    if (decom != null && decom !== '' && decom !== 0) { skippedDecom++; continue }

    const power = num(3)
    const rotor = num(4)
    const hub = num(5)
    const x = num(12)
    const y = num(13)

    if (!x || !y || x < 100000) { skippedNoCoords++; continue }
    if (power <= 0 && hub <= 0) { skippedNoSpecs++; continue }

    const [lon, lat] = proj4('EPSG:25832', 'WGS84', [x, y])
    if (lat < 54 || lat > 58 || lon < 7 || lon > 15) { skippedNoCoords++; continue }

    turbines.push({
      gsrn,
      lat,
      lon,
      hub_height_m: hub,
      rated_power_kw: power, // already in kW
      rotor_diameter_m: rotor,
      placement: (get(9) || '').toString(),
      manufacturer: (get(6) || '').toString(),
    })
  }

  console.log(`  Active turbines: ${turbines.length}`)
  console.log(`  Skipped: ${skippedDecom} decommissioned, ${skippedNoCoords} no coords, ${skippedNoSpecs} no specs`)
  return turbines
}

async function main() {
  console.log(`=== DK Wind Turbine Enrichment — Energistyrelsen Stamdataregister ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache:    ${CACHE_DIR}\n`)

  const turbines = await downloadXlsx()
  const meanPower = Math.round(turbines.reduce((s, t) => s + t.rated_power_kw, 0) / turbines.length)
  const onshore = turbines.filter(t => t.placement === 'LAND').length
  const offshore = turbines.filter(t => t.placement === 'HAV').length
  console.log(`  Mean rated power: ${meanPower} kW`)
  console.log(`  Onshore (LAND): ${onshore}, Offshore (HAV): ${offshore}`)

  const grid = buildRegistryGrid(turbines)
  console.log(`  Grid cells: ${grid.size}`)

  const allHexes = readdirSync(H3R4_DIR).filter(d => d.length === 15 && d.endsWith('ffffffff'))
  const hexDirs: string[] = []
  for (const hex of allHexes) {
    try {
      const [lat, lon] = cellToLatLng(hex)
      if (lat >= 54.5 && lat <= 57.8 && lon >= 8.0 && lon <= 13.0) {
        if (existsSync(resolve(H3R4_DIR, hex, 'industrial.arrow'))) hexDirs.push(hex)
      }
    } catch {}
  }
  console.log(`  Danish hexes with industrial.arrow: ${hexDirs.length}\n`)

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
      if (hubHeights[i] > 0 && ratedPowers[i] > 0) continue

      const lat = lats.get(i) as number ?? 0
      const lon = lons.get(i) as number ?? 0
      if (lat === 0 || lon === 0) continue

      const best = findNearestRegistryRecord(grid, lat, lon, REGISTRY_MATCH_RADIUS_M)
      if (best && fillMissingTurbineSpecs(hubHeights, ratedPowers, i, best.hub_height_m, best.rated_power_kw)) {
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

    if (hi % 10 === 0 || hi === hexDirs.length - 1) {
      const elapsed = ((Date.now() - startTime) / 1000).toFixed(0)
      console.log(`  [${elapsed}s] ${hi + 1}/${hexDirs.length} hexes, ${hexesUpdated} updated, ${filled} filled`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  OSM wind turbines in DK hexes: ${totalTurbines}`)
  console.log(`  Specs filled from ENS register: ${filled} (${(100 * filled / Math.max(totalTurbines, 1)).toFixed(1)}%)`)
  console.log(`  Hexes updated: ${hexesUpdated}`)
  console.log(`\n=== Done ===`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
