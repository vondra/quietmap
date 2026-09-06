/**
 * Enrich industrial.arrow wind turbines with USWTDB (US Wind Turbine Database).
 *
 * Downloads USWTDB CSV from USGS, parses turbine records, matches to OSM wind
 * turbines (source_type=10) by proximity, updates hub_height and
 * rated_power_kw in industrial.arrow files.
 *
 * WHY: OSM wind turbines often lack hub_height and rated_power_kw. USWTDB has
 * precise data for ~75K US turbines. With real rated_power_kw, the noise model
 * picks the correct Lw class (98-107 dB) instead of the 2000 kW default.
 *
 * Usage:
 *   npx tsx enrich-global-windturbines.ts
 *   npx tsx enrich-global-windturbines.ts --force-download
 *   npx tsx enrich-global-windturbines.ts --enrich-only
 */

import { readFileSync, writeFileSync, readdirSync, existsSync, mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { execSync } from 'node:child_process'
import { parse } from 'csv-parse/sync'
import { tableFromIPC, tableToIPC, makeTable, makeVector } from 'apache-arrow'
import { latLngToCell } from 'h3-js'
import { buildRegistryGrid, findNearestRegistryRecord, fillMissingTurbineSpecs } from './lib/wind-registry-match.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const CACHE_DIR = resolve(import.meta.dirname, '../data/enrichment/global')
const CACHE_CSV = resolve(CACHE_DIR, 'uswtdb.csv')

const forceDownload = process.argv.includes('--force-download')
const enrichOnly = process.argv.includes('--enrich-only')

/** Match radius for a USWTDB record. USWTDB surveys the mast while the OSM
 *  node can sit on the pad or an older survey point. USWTDB is US-only, so
 *  this is the pass's ONLY radius. Widened from 200 m when the duplicate
 *  national US pass was deleted (2026-09-06): the four US turbines only that
 *  pass reached sit 276-423 m from their record — among them OSM
 *  11798022164, which gains an 84 m hub and 4,200 kW. */
const USWTDB_MATCH_RADIUS_M = 500

// USWTDB download — ZIP containing CSV
const USWTDB_ZIP_URL = 'https://energy.usgs.gov/uswtdb/assets/data/uswtdbCSV.zip'

interface Turbine {
  lat: number
  lon: number
  hubHeight: number   // meters (t_hh)
  ratedPowerKw: number // kW (t_cap)
  rotorDiam: number   // meters (t_rd)
  name: string        // project name (p_name)
  model: string       // turbine model (t_model)
  manu: string        // manufacturer (t_manu)
  h3r4: string        // computed H3 R4 hex
}

// ── Step 1: Download and parse USWTDB ──

async function downloadUswtdb(): Promise<Turbine[]> {
  if (!forceDownload && existsSync(CACHE_CSV)) {
    console.log(`  Using cached USWTDB: ${CACHE_CSV}`)
    return parseUswtdbCsv(readFileSync(CACHE_CSV, 'utf-8'))
  }
  if (enrichOnly) {
    if (!existsSync(CACHE_CSV)) {
      console.error('ERROR: --enrich-only but no cache at', CACHE_CSV)
      process.exit(1)
    }
    return parseUswtdbCsv(readFileSync(CACHE_CSV, 'utf-8'))
  }

  mkdirSync(CACHE_DIR, { recursive: true })

  console.log(`  Downloading USWTDB from ${USWTDB_ZIP_URL}...`)
  const res = await fetch(USWTDB_ZIP_URL, { signal: AbortSignal.timeout(120000) })
  if (!res.ok) throw new Error(`USWTDB download failed: ${res.status} ${res.statusText}`)

  const zipBuf = Buffer.from(await res.arrayBuffer())
  const zipPath = resolve(CACHE_DIR, 'uswtdb.zip')
  writeFileSync(zipPath, zipBuf)
  console.log(`  Downloaded: ${(zipBuf.length / 1e6).toFixed(1)} MB`)

  // Extract CSV from ZIP
  const extractDir = resolve(CACHE_DIR, 'uswtdb_extract')
  mkdirSync(extractDir, { recursive: true })
  execSync(`unzip -o -q "${zipPath}" -d "${extractDir}"`, { timeout: 30000 })

  // Find the CSV file inside
  const csvFiles = readdirSync(extractDir).filter(f => f.endsWith('.csv'))
  if (csvFiles.length === 0) throw new Error('No CSV found in USWTDB ZIP')
  const csvFile = csvFiles[0]
  console.log(`  Extracted: ${csvFile}`)

  // Move CSV to cache location
  const extractedCsv = resolve(extractDir, csvFile)
  const csvContent = readFileSync(extractedCsv, 'utf-8')
  writeFileSync(CACHE_CSV, csvContent)

  // Cleanup
  execSync(`rm -rf "${extractDir}" "${zipPath}"`, { timeout: 5000 })

  return parseUswtdbCsv(csvContent)
}

/** RFC4180 parse, not `split(',')`: 656 of the 75,727 rows quote a comma
 *  inside `p_name` (case_id 3034117, "Adams Wind Generations, LLC", splits
 *  into 29 columns instead of 28) and every field after it shifts. */
export function parseUswtdbCsv(text: string): Turbine[] {
  const records = parse(text, {
    columns: true,
    skip_empty_lines: true,
    bom: true,
    relax_column_count: true,
  }) as Record<string, string>[]

  console.log(`  Parsed ${records.length} CSV records`)
  if (records.length > 0) {
    const cols = Object.keys(records[0])
    console.log(`  Columns: ${cols.slice(0, 12).join(', ')}...`)
  }

  const turbines: Turbine[] = []
  let skippedNoCoords = 0
  let skippedBadCoords = 0

  for (const r of records) {
    const lat = parseFloat(r['ylat'] || '')
    const lon = parseFloat(r['xlong'] || '')

    if (!lat || !lon || isNaN(lat) || isNaN(lon)) { skippedNoCoords++; continue }
    // Sanity: must be in continental US / Hawaii / Alaska / territories
    if (lat < 13 || lat > 72 || lon < -180 || lon > -60) { skippedBadCoords++; continue }

    const hubHeight = parseFloat(r['t_hh'] || '') || 0
    const ratedPowerKw = parseFloat(r['t_cap'] || '') || 0
    const rotorDiam = parseFloat(r['t_rd'] || '') || 0

    // Skip turbines with no useful enrichment data
    if (hubHeight <= 0 && ratedPowerKw <= 0) continue

    let h3r4: string
    try { h3r4 = latLngToCell(lat, lon, 4) } catch { continue }

    turbines.push({
      lat, lon,
      hubHeight,
      ratedPowerKw,
      rotorDiam,
      name: (r['p_name'] || '').trim(),
      model: (r['t_model'] || '').trim(),
      manu: (r['t_manu'] || '').trim(),
      h3r4,
    })
  }

  console.log(`  Valid turbines with enrichment data: ${turbines.length}`)
  if (skippedNoCoords > 0) console.log(`  Skipped (no coords): ${skippedNoCoords}`)
  if (skippedBadCoords > 0) console.log(`  Skipped (bad coords): ${skippedBadCoords}`)

  // Stats
  const withHub = turbines.filter(t => t.hubHeight > 0).length
  const withPower = turbines.filter(t => t.ratedPowerKw > 0).length
  console.log(`  With hub_height: ${withHub} (${(withHub / turbines.length * 100).toFixed(1)}%)`)
  console.log(`  With rated_power_kw: ${withPower} (${(withPower / turbines.length * 100).toFixed(1)}%)`)

  return turbines
}

// ── Step 2: Enrich industrial.arrow ──

function enrichHexes(turbines: Turbine[]): void {
  // Group turbines by H3R4 hex for fast lookup
  const turbinesByHex = new Map<string, Turbine[]>()
  for (const t of turbines) {
    if (!turbinesByHex.has(t.h3r4)) turbinesByHex.set(t.h3r4, [])
    turbinesByHex.get(t.h3r4)!.push(t)
  }
  console.log(`  Turbines span ${turbinesByHex.size} H3R4 hexes`)

  const hexDirs = readdirSync(H3R4_DIR).filter(d =>
    d.length === 15 && d.endsWith('ffffffff'))

  let totalWindTurbines = 0
  let totalMatched = 0
  let specsFilled = 0
  let hexesUpdated = 0
  let hexesScanned = 0
  const startTime = Date.now()

  for (const hexId of hexDirs) {
    // Only process hexes that have USWTDB turbines
    const hexTurbines = turbinesByHex.get(hexId)
    if (!hexTurbines) continue

    const indPath = resolve(H3R4_DIR, hexId, 'industrial.arrow')
    if (!existsSync(indPath)) continue

    hexesScanned++
    const buf = readFileSync(indPath)
    const table = tableFromIPC(buf)
    const n = table.numRows
    if (n === 0) continue

    const clat = table.getChild('centroid_lat')
    const clon = table.getChild('centroid_lon')
    const sourceType = table.getChild('source_type')
    if (!clat || !clon || !sourceType) continue

    const existingHubHeight = table.getChild('hub_height')
    const existingRatedPower = table.getChild('rated_power_kw')

    // Build new Float32 arrays for hub_height and rated_power_kw
    // NaN marks unknown here; it is written as the 0 sentinel below.
    const newHubHeight = new Float32Array(n)
    const newRatedPower = new Float32Array(n)

    // Copy existing values
    for (let i = 0; i < n; i++) {
      newHubHeight[i] = existingHubHeight ? (existingHubHeight.get(i) as number ?? NaN) : NaN
      newRatedPower[i] = existingRatedPower ? (existingRatedPower.get(i) as number ?? NaN) : NaN
    }

    let hexFilled = 0
    const grid = buildRegistryGrid(hexTurbines)

    for (let i = 0; i < n; i++) {
      const st = sourceType.get(i) as number
      if (st !== 10) continue // Only wind turbines
      totalWindTurbines++

      const lat = clat.get(i) as number
      const lon = clon.get(i) as number

      const bestTurbine = findNearestRegistryRecord(grid, lat, lon, USWTDB_MATCH_RADIUS_M)
      if (!bestTurbine) continue
      totalMatched++

      if (fillMissingTurbineSpecs(newHubHeight, newRatedPower, i, bestTurbine.hubHeight, bestTurbine.ratedPowerKw)) {
        hexFilled++
        specsFilled++
      }
    }

    if (hexFilled === 0) continue

    // Zero is the "unknown" sentinel: the columns carry no Arrow null bitmap,
    // and the Rust reader treats 0 (or NaN) as unknown and falls back to its
    // defaults (80 m hub, 2000 kW). Only positive values are kept.
    const cleanHubHeight = new Float32Array(n)
    const cleanRatedPower = new Float32Array(n)
    for (let i = 0; i < n; i++) {
      if (newHubHeight[i] > 0) cleanHubHeight[i] = newHubHeight[i]
      if (newRatedPower[i] > 0) cleanRatedPower[i] = newRatedPower[i]
    }

    // Copy ALL existing columns by iterating schema (don't hardcode column list)
    const columns: Record<string, any> = {}
    for (const field of table.schema.fields) {
      if (field.name === 'hub_height') continue
      if (field.name === 'rated_power_kw') continue
      columns[field.name] = table.getChild(field.name)!
    }

    columns['hub_height'] = makeVector(cleanHubHeight)
    columns['rated_power_kw'] = makeVector(cleanRatedPower)

    const newTable = makeTable(columns)
    // MUST use 'file' format — Rust FileReader requires ARROW1 magic bytes.
    writeFileSync(indPath, Buffer.from(tableToIPC(newTable, 'file')))
    hexesUpdated++

    // Progress every 10s
    const elapsed = Date.now() - startTime
    if (elapsed > 0 && hexesScanned % 50 === 0) {
      console.log(`  [${(elapsed / 1000).toFixed(0)}s] ${hexesScanned} hexes scanned, ${totalMatched} turbines matched`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  OSM wind turbines in matched hexes: ${totalWindTurbines}`)
  console.log(`  Matched to USWTDB: ${totalMatched} (${totalWindTurbines > 0 ? (totalMatched / totalWindTurbines * 100).toFixed(1) : 0}%)`)
  console.log(`  Rows whose specs were filled: ${specsFilled}`)
  console.log(`  Hexes updated: ${hexesUpdated}`)
}

// ── Main ──

async function main() {
  console.log(`=== Global Wind Turbine Enrichment — USWTDB ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache: ${CACHE_DIR}`)
  console.log(`  Year: ${YEAR}\n`)

  if (!existsSync(H3R4_DIR)) {
    console.error(`ERROR: H3R4 directory not found: ${H3R4_DIR}`)
    process.exit(1)
  }

  console.log('Step 1: Download USWTDB...')
  const turbines = await downloadUswtdb()

  console.log(`\nStep 2: Enrich industrial.arrow files...`)
  console.log(`  USWTDB turbines: ${turbines.length.toLocaleString()}`)
  enrichHexes(turbines)

  console.log(`\n=== Done ===`)
}

// Import-safe: run only when invoked directly — importing the exported parser
// (tests) must never trigger a download/enrichment run.
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch(err => { console.error('Error:', err); process.exit(1) })
}
