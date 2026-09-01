/**
 * Enrich DE roads.arrow with BASt SVZ 2021 traffic census data.
 *
 * Downloads Autobahnen + Bundesstraßen Excel files from BASt, parses
 * per-section vehicle class counts, converts UTM32→WGS84, matches to
 * OSM roads by ref tag + proximity, writes aadt_* columns.
 *
 * Vehicle class mapping (BASt → CNOSSOS):
 *   LVm  (cars + light vans)        → aadt_light   (Category 1)
 *   Bus + LoA (buses + medium trucks) → aadt_medium  (Category 2)
 *   LZ   (heavy trucks + trailer)   → aadt_heavy   (Category 3)
 *   Krad (motorcycles)               → aadt_moto    (Category 4)
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-de.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-de.ts --force-download
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-de.ts --enrich-only
 */

import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs'
import { shouldOverwrite } from './lib/provenance.js'
import { resolve } from 'node:path'
import proj4 from 'proj4'
import { SOURCE_ID_DE_BAST_AUTOBAHN, SOURCE_ID_DE_BAST_BUNDESSTRASSEN } from './lib/source-ids.generated.js'
import { haversineM } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCoastalCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

// CensusSection.ref starts with 'A' for Autobahn, 'B' for Bundesstraßen — pick per row.
const AUTOBAHN_DATASET_ID = SOURCE_ID_DE_BAST_AUTOBAHN
const BUNDESSTR_DATASET_ID = SOURCE_ID_DE_BAST_BUNDESSTRASSEN
const MY_SOURCE_ID = AUTOBAHN_DATASET_ID  // default for gating; actual write picks per row

// DE_HEX_BBOX overlaps western CZ, and ref matches accept up to 15 km — so a German
// "B"-road ref could land on a Bohemian road near the border. Gate by German soil
// (same fix as PL). See pipeline/lib/country-polygon.ts.
const inGermany = makeCoastalCountryGate('DE')

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/de`)
const CACHE_AUTOBAHN = resolve(CACHE_DIR, 'svz-autobahnen-2021.xlsx')
const CACHE_BUNDESSTR = resolve(CACHE_DIR, 'svz-bundesstrassen-2021.xlsx')
const CACHE_JSON = resolve(CACHE_DIR, 'svz-census.json')

const forceDownload = process.argv.includes('--force-download')
const enrichOnly = process.argv.includes('--enrich-only')

// BASt download URLs (need ?__blob=publicationFile parameter)
const AUTOBAHN_URL = 'https://www.bast.de/DE/Publikationen/Statistik/Verkehrsdaten/2021/Autobahnen-2021.xlsx?__blob=publicationFile&v=1'
const BUNDESSTR_URL = 'https://www.bast.de/DE/Publikationen/Statistik/Verkehrsdaten/2021/Bundesstrassen-2021.xlsx?__blob=publicationFile&v=1'

// EPSG:25832 (ETRS89 / UTM zone 32N) → WGS84
proj4.defs('EPSG:25832', '+proj=utm +zone=32 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs')
const toWGS84 = (x: number, y: number): [number, number] => {
  const [lon, lat] = proj4('EPSG:25832', 'WGS84', [x, y])
  return [lat, lon]
}

// ── Types ──

interface CensusSection {
  road: string          // "A 1", "B 4"
  ref: string           // normalized: "A1", "B4"
  tkzst: string         // counting station ID
  lat: number
  lon: number
  dtv: number           // total AADT
  aadt_light: number    // LVm (cars + light vans)
  aadt_medium: number   // Bus + LoA
  aadt_heavy: number    // LZ (heavy trucks + trailer)
  aadt_moto: number     // Krad (motorcycles)
}

// ── Step 1: Download BASt SVZ Excel files ──

async function downloadSvz(): Promise<CensusSection[]> {
  // Check for pre-parsed JSON cache
  if (!forceDownload && existsSync(CACHE_JSON)) {
    console.log(`  Using cached parsed data: ${CACHE_JSON}`)
    // The cache predates the split-required rule below and may still hold
    // sections whose per-class columns are all blank — the same filter must
    // run on BOTH load paths or the cached path re-trips the #31.4 guard.
    const cached: CensusSection[] = JSON.parse(readFileSync(CACHE_JSON, 'utf-8'))
    const usable = cached.filter((s) => s.aadt_light + s.aadt_medium + s.aadt_heavy + s.aadt_moto > 0)
    if (usable.length < cached.length) {
      console.log(`  ${cached.length - usable.length} cached sections dropped (DTV without class split — cannot stamp zeros under a measured id)`)
    }
    return usable
  }
  if (enrichOnly && !existsSync(CACHE_JSON)) {
    // Try to parse from Excel cache
    if (existsSync(CACHE_AUTOBAHN) && existsSync(CACHE_BUNDESSTR)) {
      console.log('  Parsing from cached Excel files...')
      return await parseBothFiles()
    }
    console.error('ERROR: --enrich-only but no cached data found')
    process.exit(1)
  }

  mkdirSync(CACHE_DIR, { recursive: true })

  // Download both Excel files
  for (const [url, path, name] of [
    [AUTOBAHN_URL, CACHE_AUTOBAHN, 'Autobahnen'],
    [BUNDESSTR_URL, CACHE_BUNDESSTR, 'Bundesstraßen'],
  ] as const) {
    if (!forceDownload && existsSync(path)) {
      console.log(`  Using cached ${name}: ${path}`)
      continue
    }
    console.log(`  Downloading ${name} from BASt...`)
    const res = await fetch(url, {
      signal: AbortSignal.timeout(120_000),
      headers: { 'User-Agent': 'Mozilla/5.0 (QuietMap noise enrichment)' },
    })
    if (!res.ok) throw new Error(`HTTP ${res.status} for ${name}`)
    const buf = Buffer.from(await res.arrayBuffer())
    writeFileSync(path, buf)
    console.log(`  Cached ${name}: ${(buf.length / 1024 / 1024).toFixed(1)} MB`)
  }

  return await parseBothFiles()
}

async function parseBothFiles(): Promise<CensusSection[]> {
  const XLSX = (await import('xlsx')).default || await import('xlsx')
  const sections: CensusSection[] = []

  for (const [path, label] of [
    [CACHE_AUTOBAHN, 'Autobahnen'],
    [CACHE_BUNDESSTR, 'Bundesstraßen'],
  ] as const) {
    const wb = XLSX.readFile(path)
    const ws = wb.Sheets['Zeilenformat']
    if (!ws) {
      console.log(`  WARN: No Zeilenformat sheet in ${label}`)
      continue
    }

    const data: any[][] = XLSX.utils.sheet_to_json(ws, { header: 1 })
    const headers = data[0] as string[]

    // Find column indices
    const col = (name: string): number => {
      const idx = headers.indexOf(name)
      if (idx < 0) console.log(`  WARN: Column ${name} not found in ${label}`)
      return idx
    }

    const iStr = col('Str')
    const iTKZST = col('TKZST')
    const iDTV = col('DTV')
    const iLVm = col('DTVLVm')
    const iBus = col('DTVBus')
    const iLoA = col('DTVLoA')
    const iLZ = col('DTVLZ')
    const iKrad = col('DTVKrad')
    const iX = col('X_Koordinate')
    const iY = col('Y_Koordinate')

    let parsed = 0
    let skipped = 0
    let skippedNoSplit = 0
    for (let i = 1; i < data.length; i++) {
      const row = data[i]
      const road = String(row[iStr] || '').trim()
      const x = Number(row[iX])
      const y = Number(row[iY])
      const dtv = Number(row[iDTV])

      if (!road || !x || !y || x < 100000 || y < 5000000 || !dtv) {
        skipped++
        continue
      }

      const [lat, lon] = toWGS84(x, y)
      if (lat < 47 || lat > 55.5 || lon < 5.5 || lon > 15.5) {
        skipped++ // outside Germany
        continue
      }

      const ref = road.replace(/\s+/g, '')  // "A 1" → "A1"

      const aadt_light = Math.round(Number(row[iLVm]) || 0)
      const aadt_medium = Math.round((Number(row[iBus]) || 0) + (Number(row[iLoA]) || 0))
      const aadt_heavy = Math.round(Number(row[iLZ]) || 0)
      const aadt_moto = Math.round(Number(row[iKrad]) || 0)
      // Some SVZ sections publish a DTV total with the per-class columns blank.
      // Our schema stamps the four classes, not the total — stamping 0/0/0/0
      // under a MEASURED id is the R7 zero-write shape the writer now rejects
      // (#31.4 guard tripped exactly here, hex 841e36d row 44033). Fabricating
      // a split under a measured id would overclaim; skip and count instead.
      if (aadt_light + aadt_medium + aadt_heavy + aadt_moto === 0) {
        skippedNoSplit++
        continue
      }

      sections.push({
        road,
        ref,
        tkzst: String(row[iTKZST] || ''),
        lat, lon,
        dtv: Math.round(dtv),
        aadt_light,
        aadt_medium,
        aadt_heavy,
        aadt_moto,
      })
      parsed++
    }

    console.log(`  ${label}: ${parsed} sections parsed, ${skipped} skipped (no coords/DTV), ${skippedNoSplit} skipped (DTV without class split)`)
  }

  // Cache as JSON
  writeFileSync(CACHE_JSON, JSON.stringify(sections))
  console.log(`  Cached ${sections.length} sections to ${CACHE_JSON}`)
  return sections
}

// ── Step 2: Build spatial index ──

function buildSpatialGrid(sections: CensusSection[]): Map<string, CensusSection[]> {
  const grid = new Map<string, CensusSection[]>()
  const CELL = 0.01  // ~1.1 km cells
  for (const s of sections) {
    const key = `${Math.floor(s.lat / CELL)},${Math.floor(s.lon / CELL)}`
    const list = grid.get(key) || []
    list.push(s)
    grid.set(key, list)
  }
  return grid
}

function findNearby(grid: Map<string, CensusSection[]>, lat: number, lon: number, maxDist: number): CensusSection[] {
  const CELL = 0.01
  const r = Math.ceil(maxDist / 1100 / CELL) // search radius in cells
  const gy = Math.floor(lat / CELL)
  const gx = Math.floor(lon / CELL)
  const result: CensusSection[] = []
  for (let dy = -r; dy <= r; dy++) {
    for (let dx = -r; dx <= r; dx++) {
      const key = `${gy + dy},${gx + dx}`
      const list = grid.get(key)
      if (list) result.push(...list)
    }
  }
  return result
}

// ── Step 3: Enrich Arrow files ──

async function enrichArrows(sections: CensusSection[]) {
  const grid = buildSpatialGrid(sections)
  console.log(`\n  Spatial grid: ${grid.size} cells for ${sections.length} sections`)

  // Build ref lookup for fast matching
  const refIndex = new Map<string, CensusSection[]>()
  for (const s of sections) {
    const list = refIndex.get(s.ref) || []
    list.push(s)
    refIndex.set(s.ref, list)
  }
  console.log(`  Ref index: ${refIndex.size} unique road refs`)

  // Germany bbox (+margin); [minLat,minLon,maxLat,maxLon]. iterateCountryHexes skips
  // the rest of the planet so the loader doesn't read every roads.arrow on Earth.
  const DE_HEX_BBOX: [number, number, number, number] = [46, 4, 56, 16]
  const hexDirs = iterateCountryHexes(H3R4_DIR, DE_HEX_BBOX)
  console.log(`  German hexes: ${hexDirs.length}\n`)

  let totalSegments = 0
  let matchedSegments = 0
  let preservedSegments = 0
  let hexesUpdated = 0
  const matchByClass: Record<string, { matched: number; total: number }> = {}
  const classNames = ['motorway', 'trunk', 'primary', 'secondary', 'tertiary', 'residential', 'other']
  for (const c of classNames) matchByClass[c] = { matched: 0, total: 0 }

  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        const className = classNames[Math.min(row.roadClass, 6)]
        matchByClass[className].total++

        // Fast-exit if a higher-priority dataset owns the row (both BASt ids share
        // priority 80, so gating with MY_SOURCE_ID is representative; writeRoadAadt
        // re-checks the gate against the picked id).
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) {
          if (row.existingSourceId !== 0) preservedSegments++
          return null
        }

        const rc = row.roadClass
        const normRef = (row.ref?.toString().trim() || '').replace(/\s+/g, '') // "A 1" → "A1"

        let bestSection: CensusSection | null = null
        let bestDist = Infinity

        // Strategy 1: match by ref.
        if (normRef && refIndex.has(normRef)) {
          for (const c of refIndex.get(normRef)!) {
            const dist = haversineM(row.midLat, row.midLon, c.lat, c.lon)
            if (dist < bestDist) { bestDist = dist; bestSection = c }
          }
        }

        // Strategy 2: motorway/trunk without ref match → nearest Autobahn/Bundesstraße ≤2km
        // (Autobahn only to A-roads, trunk only to B-roads).
        if (!bestSection && (rc === 0 || rc === 1) && row.midLat > 47 && row.midLat < 55.5) {
          for (const c of findNearby(grid, row.midLat, row.midLon, 2000)) {
            const isAutobahn = c.ref.startsWith('A')
            if (rc === 0 && !isAutobahn) continue
            if (rc === 1 && isAutobahn) continue
            const dist = haversineM(row.midLat, row.midLon, c.lat, c.lon)
            if (dist < bestDist && dist < 2000) { bestDist = dist; bestSection = c }
          }
        }

        const maxMatchDist = normRef ? 15000 : 2000 // 15km ref-matched, 2km proximity-only
        if (!bestSection || bestDist >= maxMatchDist) return null
        if (!inGermany(row.midLat, row.midLon)) return null // a road on Czech soil never gets DE data
        // Pick the dataset per row: A* = Autobahn, B* = Bundesstraßen.
        const pickedId = bestSection.ref.startsWith('A') ? AUTOBAHN_DATASET_ID : BUNDESSTR_DATASET_ID
        return {
          light: bestSection.aadt_light, medium: bestSection.aadt_medium,
          heavy: bestSection.aadt_heavy, moto: bestSection.aadt_moto, sourceId: pickedId,
        }
      },
      (row) => {
        matchByClass[classNames[Math.min(row.roadClass, 6)]].matched++
        matchedSegments++
      },
    )
    totalSegments += r.rows
    if (r.updated) hexesUpdated++

    // Progress every 10 hexes (German hexes are large).
    if (hi % 10 === 0) {
      process.stdout.write(`\r  [${Math.round((Date.now() - startTime) / 1000)}s] ${hi + 1}/${hexDirs.length} hexes, ${hexesUpdated} updated, ${matchedSegments} matched`)
    }
  }

  console.log(`\n=== Enrichment Results ===`)
  console.log(`  Total segments: ${totalSegments}`)
  console.log(`  Pre-existing (preserved): ${preservedSegments}`)
  console.log(`  Newly matched: ${matchedSegments} (${(100 * matchedSegments / Math.max(totalSegments, 1)).toFixed(1)}%)`)
  console.log(`  Hexes updated: ${hexesUpdated}`)
  console.log(`\n  By road class:`)
  for (const [cls, stats] of Object.entries(matchByClass)) {
    if (stats.total > 0) {
      console.log(`    ${cls.padEnd(12)}: ${stats.matched}/${stats.total} (${(100 * stats.matched / stats.total).toFixed(1)}%)`)
    }
  }

  // Top AADT spot-check
  const topSections = sections.sort((a, b) => b.dtv - a.dtv).slice(0, 10)
  console.log(`\n  Top 10 highest-AADT sections:`)
  for (const s of topSections) {
    const hv = Math.round(100 * s.aadt_heavy / (s.aadt_light + s.aadt_medium + s.aadt_heavy + s.aadt_moto))
    console.log(`    ${s.road.padEnd(8)} DTV=${s.dtv.toLocaleString().padStart(7)}  HV=${hv}%  (${s.lat.toFixed(3)}, ${s.lon.toFixed(3)})`)
  }
}

// ── Main ──

async function main() {
  console.log(`=== DE Road Traffic Enrichment (BASt SVZ 2021) ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache: ${CACHE_DIR}`)
  console.log(`  Year: ${YEAR}\n`)

  const sections = await downloadSvz()
  console.log(`\n  Total census sections: ${sections.length}`)

  // Summary stats
  const autobahn = sections.filter(s => s.ref.startsWith('A'))
  const bundesstr = sections.filter(s => s.ref.startsWith('B'))
  console.log(`  Autobahn (A): ${autobahn.length} sections`)
  console.log(`  Bundesstraßen (B): ${bundesstr.length} sections`)

  const avgDTV = Math.round(sections.reduce((s, c) => s + c.dtv, 0) / sections.length)
  console.log(`  Average DTV: ${avgDTV.toLocaleString()}`)

  await enrichArrows(sections)

  console.log(`\n=== Done ===`)
}

main().catch(err => { console.error(err); process.exit(1) })
