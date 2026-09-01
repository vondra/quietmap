/**
 * Enrich DE industrial.arrow with MaStR wind turbine specs.
 *
 * Matches MaStR wind turbines (41K) to OSM source_type=10 turbines
 * by proximity (<200m), writes hub_height and rated_power_kw.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-de.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-de.ts --enrich-only
 */

import { readFileSync, writeFileSync, readdirSync, existsSync, mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { tableFromIPC, tableToIPC, vectorFromArray, Float32 } from 'apache-arrow'
import { cellToLatLng } from 'h3-js'
import { haversineM } from './lib/spatial.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/de`)
const CACHE_CSV = resolve(CACHE_DIR, 'mastr-wind.csv')

interface MastrTurbine {
  lat: number
  lon: number
  rated_power_kw: number
  hub_height_m: number
}

function loadTurbines(): MastrTurbine[] {
  if (!existsSync(CACHE_CSV)) {
    console.error('ERROR: No cached MaStR wind CSV. Run MaStR extraction first.')
    process.exit(1)
  }

  const lines = readFileSync(CACHE_CSV, 'utf-8').split('\n')
  const turbines: MastrTurbine[] = []
  for (let i = 1; i < lines.length; i++) {
    const [lon, lat, power, hub] = lines[i].split(',')
    const fLon = parseFloat(lon)
    const fLat = parseFloat(lat)
    const fPower = parseFloat(power)
    const fHub = parseFloat(hub)
    if (fLat > 47 && fLat < 56 && fLon > 5 && fLon < 16 && fPower > 0) {
      turbines.push({ lat: fLat, lon: fLon, rated_power_kw: fPower, hub_height_m: fHub || 0 })
    }
  }
  return turbines
}

function buildGrid(turbines: MastrTurbine[]): Map<string, MastrTurbine[]> {
  const grid = new Map<string, MastrTurbine[]>()
  for (const t of turbines) {
    const key = `${Math.floor(t.lat * 100)},${Math.floor(t.lon * 100)}`
    const list = grid.get(key) || []
    list.push(t)
    grid.set(key, list)
  }
  return grid
}

async function main() {
  console.log(`=== DE Wind Turbine Enrichment (MaStR) ===\n`)

  const turbines = loadTurbines()
  console.log(`  MaStR turbines: ${turbines.length}`)
  console.log(`  With hub height: ${turbines.filter(t => t.hub_height_m > 0).length}`)
  console.log(`  Mean power: ${Math.round(turbines.reduce((s, t) => s + t.rated_power_kw, 0) / turbines.length)} kW`)

  const grid = buildGrid(turbines)
  console.log(`  Grid cells: ${grid.size}`)

  // Pre-filter German hexes
  const allHexes = readdirSync(H3R4_DIR).filter(d => !d.startsWith('.'))
  const hexDirs: string[] = []
  for (const hex of allHexes) {
    try {
      const [lat, lon] = cellToLatLng(hex)
      if (lat > 46 && lat < 56 && lon > 4 && lon < 16) {
        if (existsSync(resolve(H3R4_DIR, hex, 'industrial.arrow'))) hexDirs.push(hex)
      }
    } catch {}
  }
  console.log(`  German hexes with industrial.arrow: ${hexDirs.length}\n`)

  let totalTurbines = 0, matched = 0, hexesUpdated = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const arrowPath = resolve(H3R4_DIR, hex, 'industrial.arrow')
    const buf = readFileSync(arrowPath)
    const table = tableFromIPC(buf)
    const numRows = table.numRows

    const sourceTypes = table.getChild('source_type')
    const lats = table.getChild('centroid_lat')
    const lons = table.getChild('centroid_lon')
    const existingHub = table.getChild('hub_height')
    const existingPower = table.getChild('rated_power_kw')

    if (!sourceTypes || !lats || !lons) continue

    const hubHeights = new Float32Array(numRows)
    const ratedPowers = new Float32Array(numRows)
    let hexMatched = 0

    // Copy existing values
    for (let i = 0; i < numRows; i++) {
      hubHeights[i] = existingHub?.get(i) ?? 0
      ratedPowers[i] = existingPower?.get(i) ?? 0
    }

    for (let i = 0; i < numRows; i++) {
      const st = sourceTypes.get(i) ?? 0
      if (st !== 10) continue // Only wind turbines
      totalTurbines++

      // Skip if already enriched with non-zero values
      if (hubHeights[i] > 0 && ratedPowers[i] > 0) continue

      const lat = lats.get(i) ?? 0
      const lon = lons.get(i) ?? 0
      if (lat === 0 || lon === 0) continue

      // Find nearest MaStR turbine within 200m
      const gy = Math.floor(lat * 100)
      const gx = Math.floor(lon * 100)
      let best: MastrTurbine | null = null
      let bestDist = 200 // max 200m

      for (let dy = -1; dy <= 1; dy++) {
        for (let dx = -1; dx <= 1; dx++) {
          const key = `${gy + dy},${gx + dx}`
          const candidates = grid.get(key)
          if (!candidates) continue
          for (const c of candidates) {
            const dist = haversineM(lat, lon, c.lat, c.lon)
            if (dist < bestDist) { bestDist = dist; best = c }
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
      const enriched = new table.constructor(columns)
      writeFileSync(arrowPath, tableToIPC(enriched, 'file'))
      hexesUpdated++
    }

    if (hi % 50 === 0) {
      console.log(`  [${Math.round((Date.now() - startTime) / 1000)}s] ${hi + 1}/${hexDirs.length} hexes, ${matched} turbines matched`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  OSM wind turbines in DE: ${totalTurbines}`)
  console.log(`  Matched to MaStR: ${matched} (${(100 * matched / Math.max(totalTurbines, 1)).toFixed(1)}%)`)
  console.log(`  Hexes updated: ${hexesUpdated}`)
  console.log(`\n=== Done ===`)
}

main().catch(err => { console.error(err); process.exit(1) })
