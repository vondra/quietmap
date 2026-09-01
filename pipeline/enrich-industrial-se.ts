/**
 * Enrich SE industrial.arrow with Swedish wind turbine specs from Vindbrukskollen.
 *
 * Source: Länsstyrelsen Vindbrukskollen
 *   https://ext-dokument.lansstyrelsen.se/gemensamt/geodata/ShapeExport/lst.vbk_vindkraftverk.zip
 *   23,144 wind turbines (filtered to STATUS="Uppfört" — built/operating)
 *   Per-turbine: NAVHOJD (hub height m), MAXEFFEKT (rated power MW), ROTDIAMETE (rotor m)
 *   CRS: WGS84 (auto-reprojected by shpjs)
 *   License: CC0 1.0 Universell (public domain)
 *
 * Matches OSM source_type=10 wind turbines by proximity (<200m), writes
 * hub_height + rated_power_kw columns.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-se.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-se.ts --enrich-only
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-se.ts --force-download
 */

import { readFileSync, writeFileSync, readdirSync, existsSync, mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { tableFromIPC, tableToIPC, vectorFromArray, makeTable, Float32 } from 'apache-arrow'
import { cellToLatLng } from 'h3-js'
import shp from 'shpjs'
import { haversineM } from './lib/spatial.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/se`)
const CACHE_ZIP = resolve(CACHE_DIR, 'vindbrukskollen.zip')

const forceDownload = process.argv.includes('--force-download')
const enrichOnly = process.argv.includes('--enrich-only')

const VBK_URL = 'https://ext-dokument.lansstyrelsen.se/gemensamt/geodata/ShapeExport/lst.vbk_vindkraftverk.zip'

interface WindTurbine {
  verkid: string
  lat: number
  lon: number
  hub_height_m: number
  rated_power_kw: number
  rotor_diameter_m: number
  status: string
}

async function downloadShp(): Promise<WindTurbine[]> {
  if (!forceDownload && !existsSync(CACHE_ZIP)) {
    if (enrichOnly) throw new Error('--enrich-only but Vindbrukskollen ZIP not cached')
    mkdirSync(CACHE_DIR, { recursive: true })
    console.log(`  Downloading Vindbrukskollen wind turbine SHP...`)
    const res = await fetch(VBK_URL, { signal: AbortSignal.timeout(60_000) })
    if (!res.ok) throw new Error(`HTTP ${res.status} for Vindbrukskollen`)
    const buf = Buffer.from(await res.arrayBuffer())
    writeFileSync(CACHE_ZIP, buf)
    console.log(`  Cached: ${(buf.length / 1e6).toFixed(2)} MB`)
  } else {
    console.log(`  Using cached: ${CACHE_ZIP}`)
  }

  console.log(`  Loading SHP via shpjs...`)
  const buf = readFileSync(CACHE_ZIP)
  const result = await shp(buf)
  const fc = Array.isArray(result) ? result[0] : result
  console.log(`  SHP features: ${fc.features.length}`)

  const turbines: WindTurbine[] = []
  let skippedStatus = 0
  let skippedNoCoords = 0
  let skippedNoSpecs = 0

  for (const feat of fc.features) {
    const props = feat.properties || {}
    const status = (props.STATUS || '').toString()
    // Only "Uppfört" (built/operating) turbines
    if (status !== 'Uppfört') { skippedStatus++; continue }

    const coords = feat.geometry?.coordinates
    if (!coords || !Array.isArray(coords) || coords.length < 2) { skippedNoCoords++; continue }
    const [lon, lat] = coords as [number, number]
    if (!lat || !lon) { skippedNoCoords++; continue }

    const hub = parseFloat(props.NAVHOJD || '0')
    const powerMw = parseFloat(props.MAXEFFEKT || '0')
    const rotor = parseFloat(props.ROTDIAMETE || '0')

    if (powerMw <= 0 && hub <= 0) { skippedNoSpecs++; continue }

    turbines.push({
      verkid: (props.VERKID || '').toString(),
      lat,
      lon,
      hub_height_m: hub,
      rated_power_kw: powerMw * 1000, // MW → kW
      rotor_diameter_m: rotor,
      status,
    })
  }

  console.log(`  Built turbines (Uppfört): ${turbines.length}`)
  console.log(`  Skipped: ${skippedStatus} non-built, ${skippedNoCoords} no coords, ${skippedNoSpecs} no specs`)
  return turbines
}

function buildGrid(turbines: WindTurbine[]): Map<string, WindTurbine[]> {
  const grid = new Map<string, WindTurbine[]>()
  for (const t of turbines) {
    const key = `${Math.floor(t.lat * 100)},${Math.floor(t.lon * 100)}`
    if (!grid.has(key)) grid.set(key, [])
    grid.get(key)!.push(t)
  }
  return grid
}

async function main() {
  console.log(`=== SE Wind Turbine Enrichment — Vindbrukskollen ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache:    ${CACHE_DIR}\n`)

  const turbines = await downloadShp()
  const withHub = turbines.filter(t => t.hub_height_m > 0).length
  const withPower = turbines.filter(t => t.rated_power_kw > 0).length
  const meanPower = withPower > 0
    ? Math.round(turbines.filter(t => t.rated_power_kw > 0).reduce((s, t) => s + t.rated_power_kw, 0) / withPower)
    : 0
  console.log(`  With hub height: ${withHub}`)
  console.log(`  With rated power: ${withPower}`)
  console.log(`  Mean rated power: ${meanPower} kW`)

  const grid = buildGrid(turbines)
  console.log(`  Grid cells: ${grid.size}`)

  // Pre-filter Swedish hexes
  const allHexes = readdirSync(H3R4_DIR).filter(d => d.length === 15 && d.endsWith('ffffffff'))
  const hexDirs: string[] = []
  for (const hex of allHexes) {
    try {
      const [lat, lon] = cellToLatLng(hex)
      if (lat >= 55.3 && lat <= 69.1 && lon >= 10.9 && lon <= 24.2) {
        if (existsSync(resolve(H3R4_DIR, hex, 'industrial.arrow'))) hexDirs.push(hex)
      }
    } catch {}
  }
  console.log(`  Swedish hexes with industrial.arrow: ${hexDirs.length}\n`)

  let totalTurbines = 0
  let matched = 0
  let hexesUpdated = 0
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
      let best: WindTurbine | null = null
      let bestDist = 200 // 200m max

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

    if (hi % 50 === 0 || hi === hexDirs.length - 1) {
      const elapsed = ((Date.now() - startTime) / 1000).toFixed(0)
      console.log(`  [${elapsed}s] ${hi + 1}/${hexDirs.length} hexes, ${hexesUpdated} updated, ${matched} matched`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  OSM wind turbines in SE hexes: ${totalTurbines}`)
  console.log(`  Matched to Vindbrukskollen: ${matched} (${(100 * matched / Math.max(totalTurbines, 1)).toFixed(1)}%)`)
  console.log(`  Hexes updated: ${hexesUpdated}`)
  console.log(`\n=== Done ===`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
