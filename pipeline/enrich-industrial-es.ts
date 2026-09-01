/**
 * Enrich ES industrial.arrow with Spanish wind turbine specs.
 *
 * Source: Castilla-La Mancha open data
 *   "AEROGENERADORES CLM" — 3,140 wind turbines with hub_height (ALTURA_BUJE),
 *   rated_power_kw (POTENCIA_UNI), rotor radius (RADIO).
 *   UTM-30N (ETRS89, EPSG:25830) → WGS84 via proj4.
 *
 * Matches OSM source_type=10 wind turbines to CLM by proximity (<200m),
 * writes hub_height + rated_power_kw + rotor_diameter columns.
 *
 * National Spanish wind registry (REE/AEE/IDAE) is not openly available with
 * per-turbine specs; only Castilla-La Mancha publishes detailed CSV. Other
 * regions remain at OSM defaults.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-es.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-es.ts --enrich-only
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-es.ts --force-download
 */

import { readFileSync, writeFileSync, readdirSync, existsSync, mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { tableFromIPC, tableToIPC, vectorFromArray, makeTable, Float32 } from 'apache-arrow'
import { cellToLatLng } from 'h3-js'
import proj4 from 'proj4'
import { haversineM } from './lib/spatial.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/es`)
const CACHE_CSV = resolve(CACHE_DIR, 'clm-aerogeneradores.csv')

const forceDownload = process.argv.includes('--force-download')
const enrichOnly = process.argv.includes('--enrich-only')

const CLM_URL = 'https://datosabiertos.castillalamancha.es/sites/datosabiertos.castillalamancha.es/files/AEROGENERADORES%20CLM.csv'

// ETRS89 / UTM zone 30N (Castilla-La Mancha) → WGS84
proj4.defs('EPSG:25830', '+proj=utm +zone=30 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs')

interface WindTurbine {
  installation: string
  lat: number
  lon: number
  rated_power_kw: number
  hub_height_m: number
  rotor_diameter_m: number
}

function parseCsvLine(line: string): string[] {
  const fields: string[] = []
  let current = ''
  let inQuotes = false
  for (let i = 0; i < line.length; i++) {
    const ch = line[i]
    if (inQuotes) {
      if (ch === '"') inQuotes = false
      else current += ch
    } else {
      if (ch === '"') inQuotes = true
      else if (ch === ',') { fields.push(current.trim()); current = '' }
      else current += ch
    }
  }
  fields.push(current.trim())
  return fields
}

async function downloadTurbines(): Promise<WindTurbine[]> {
  if (!forceDownload && !existsSync(CACHE_CSV)) {
    if (enrichOnly) throw new Error('--enrich-only but no CLM CSV cached')
    mkdirSync(CACHE_DIR, { recursive: true })
    console.log(`  Downloading CLM AEROGENERADORES CSV...`)
    const res = await fetch(CLM_URL, { signal: AbortSignal.timeout(60_000) })
    if (!res.ok) throw new Error(`HTTP ${res.status} for CLM CSV`)
    writeFileSync(CACHE_CSV, await res.text())
    console.log(`  Cached ${CACHE_CSV}`)
  } else {
    console.log(`  Using cached CSV: ${CACHE_CSV}`)
  }

  const lines = readFileSync(CACHE_CSV, 'utf-8').split('\n')
  const header = parseCsvLine(lines[0])
  const ci = (name: string) => header.indexOf(name)
  const iInst = ci('INSTALACION')
  const iX = ci('UTM_X')
  const iY = ci('UTM_Y')
  const iRadio = ci('RADIO')
  const iPower = ci('POTENCIA_UNI')
  const iHub = ci('ALTURA_BUJE')

  if (iX < 0 || iY < 0 || iPower < 0) {
    throw new Error(`CSV header missing expected columns. Found: ${header.join(', ')}`)
  }

  const turbines: WindTurbine[] = []
  let skipped = 0
  for (let i = 1; i < lines.length; i++) {
    const line = lines[i].trim()
    if (!line) continue
    const cols = parseCsvLine(line)
    const x = parseFloat(cols[iX])
    const y = parseFloat(cols[iY])
    const power = parseFloat(cols[iPower])
    const radius = parseFloat(cols[iRadio])
    const hub = parseFloat(cols[iHub])
    if (!x || !y || !power) { skipped++; continue }
    const [lon, lat] = proj4('EPSG:25830', 'WGS84', [x, y])
    if (lat < 35 || lat > 44 || lon < -10 || lon > 5) { skipped++; continue }
    turbines.push({
      installation: cols[iInst] || '',
      lat,
      lon,
      rated_power_kw: power,
      hub_height_m: hub || 0,
      rotor_diameter_m: radius ? radius * 2 : 0,
    })
  }
  console.log(`  Parsed ${turbines.length} turbines (${skipped} skipped)`)
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
  console.log(`=== ES Wind Turbine Enrichment — Castilla-La Mancha ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache:    ${CACHE_DIR}\n`)

  const turbines = await downloadTurbines()
  const withHub = turbines.filter(t => t.hub_height_m > 0).length
  const withRotor = turbines.filter(t => t.rotor_diameter_m > 0).length
  const meanPower = Math.round(turbines.reduce((s, t) => s + t.rated_power_kw, 0) / turbines.length)
  console.log(`  CLM turbines: ${turbines.length}`)
  console.log(`  With hub height: ${withHub}`)
  console.log(`  With rotor diameter: ${withRotor}`)
  console.log(`  Mean rated power: ${meanPower} kW`)

  const grid = buildGrid(turbines)
  console.log(`  Grid cells: ${grid.size}`)

  // Pre-filter Spanish hexes (CLM is roughly 38–41 N, -5 to -1 lon)
  const allHexes = readdirSync(H3R4_DIR).filter(d => d.length === 15 && d.endsWith('ffffffff'))
  const hexDirs: string[] = []
  for (const hex of allHexes) {
    try {
      const [lat, lon] = cellToLatLng(hex)
      if (lat >= 37 && lat <= 42 && lon >= -6 && lon <= 0) {
        if (existsSync(resolve(H3R4_DIR, hex, 'industrial.arrow'))) hexDirs.push(hex)
      }
    } catch {}
  }
  console.log(`  CLM-area hexes with industrial.arrow: ${hexDirs.length}\n`)

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
      if (st !== 10) continue // wind turbine
      totalTurbines++

      // Skip if already enriched
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

    if (hi % 25 === 0 || hi === hexDirs.length - 1) {
      const elapsed = ((Date.now() - startTime) / 1000).toFixed(0)
      console.log(`  [${elapsed}s] ${hi + 1}/${hexDirs.length} hexes, ${hexesUpdated} updated, ${matched} turbines matched`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  OSM wind turbines in CLM hexes: ${totalTurbines}`)
  console.log(`  Matched to CLM registry: ${matched} (${(100 * matched / Math.max(totalTurbines, 1)).toFixed(1)}%)`)
  console.log(`  Hexes updated: ${hexesUpdated}`)
  console.log(`\n=== Done ===`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
