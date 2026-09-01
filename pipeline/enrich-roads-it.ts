/**
 * Enrich IT roads.arrow with ANAS TGM (Traffico Giornaliero Medio = AADT) data.
 *
 * Downloads shapefile from MIT open data portal, converts to GeoJSON via ogr2ogr,
 * parses monitoring station points (Strada + TGM value), matches to OSM road
 * segments by road ref + proximity, adds aadt_light + source_id to Arrow.
 *
 * Data format: 767 monitoring stations (Point geometry in UTM 32N), each with:
 *   Strada — road ref (e.g. "A1", "SS106", "RA05")
 *   TGM — Traffico Giornaliero Medio (AADT equivalent)
 *   Km — kilometric position on the road
 *
 * Matching strategy: ref match (mandatory) + nearest station within 30km.
 * This mirrors enrich-roads-cz.ts which also uses ref + proximity.
 *
 * Source: Ministero delle Infrastrutture e dei Trasporti — TGM Nov 2015
 * https://dati.mit.gov.it/catalog/dataset/a9b851f0-cb05-4e7e-ae43-040926a368db
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-it.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-it.ts --force-download
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-it.ts --enrich-only
 */

import { readFileSync, writeFileSync, globSync, existsSync, mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { execSync } from 'node:child_process'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_IT_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { flatDist } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_IT_NATIONAL_ROADS

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/it`)
const CACHE_GEOJSON = resolve(CACHE_DIR, 'tgm-roads.geojson')

const forceDownload = process.argv.includes('--force-download')
const enrichOnly = process.argv.includes('--enrich-only')

const TGM_URL = 'https://dati.mit.gov.it/catalog/dataset/a9b851f0-cb05-4e7e-ae43-040926a368db/resource/09935ff0-da89-4b11-afe9-9902fad9ea57/download/tgm_nov2015.zip'

// Italy bounding box (lat/lon)
const IT_BBOX = { minLat: 35.5, maxLat: 47.1, minLon: 6.6, maxLon: 18.6 }
// Same box as [minLat, minLon, maxLat, maxLon] for iterateCountryHexes — it skips
// the rest of the planet so the loader doesn't read every roads.arrow on Earth.
const IT_HEX_BBOX: [number, number, number, number] = [
  IT_BBOX.minLat, IT_BBOX.minLon, IT_BBOX.maxLat, IT_BBOX.maxLon,
]

// ── Types ──

interface TgmStation {
  ref: string          // normalized road ref
  aadt: number         // TGM value (total AADT)
  lat: number
  lon: number
}

// ── Step 1: Download + convert shapefile ──

async function downloadAndConvert(): Promise<any> {
  if (!forceDownload && existsSync(CACHE_GEOJSON) && !enrichOnly) {
    console.log(`  Using cached GeoJSON: ${CACHE_GEOJSON}`)
    return JSON.parse(readFileSync(CACHE_GEOJSON, 'utf-8'))
  }
  if (enrichOnly) {
    if (!existsSync(CACHE_GEOJSON)) {
      console.error('ERROR: --enrich-only but no cached data found')
      process.exit(1)
    }
    return JSON.parse(readFileSync(CACHE_GEOJSON, 'utf-8'))
  }

  mkdirSync(CACHE_DIR, { recursive: true })

  const zipPath = resolve(CACHE_DIR, 'tgm_nov2015.zip')
  const shpDir = resolve(CACHE_DIR, 'shp')

  // Download ZIP
  console.log('  Downloading TGM shapefile from MIT...')
  const res = await fetch(TGM_URL, { signal: AbortSignal.timeout(120_000) })
  if (!res.ok) throw new Error(`TGM download error: ${res.status} ${res.statusText}`)
  const buf = Buffer.from(await res.arrayBuffer())
  writeFileSync(zipPath, buf)
  console.log(`  Downloaded: ${(buf.length / 1e6).toFixed(1)} MB`)

  // Extract
  mkdirSync(shpDir, { recursive: true })
  execSync(`unzip -o -q "${zipPath}" -d "${shpDir}"`, { timeout: 60_000 })

  // Find .shp file(s)
  const shpFiles = findFiles(shpDir, '.shp')
  if (shpFiles.length === 0) {
    throw new Error(`No .shp files found in extracted archive at ${shpDir}`)
  }
  console.log(`  Found shapefile: ${shpFiles[0].split('/').pop()}`)

  // Convert to GeoJSON via ogr2ogr — source is UTM 32N (EPSG:32632)
  // Use -skipfailures because a few points near zone edges fail reprojection
  const shpFile = shpFiles[0]
  console.log(`  Converting to GeoJSON via ogr2ogr (EPSG:32632 -> 4326)...`)
  execSync(
    `ogr2ogr -f GeoJSON -t_srs EPSG:4326 -skipfailures "${CACHE_GEOJSON}" "${shpFile}"`,
    { timeout: 120_000 }
  )

  // Clean up
  execSync(`rm -rf "${shpDir}" "${zipPath}"`)

  const geojson = JSON.parse(readFileSync(CACHE_GEOJSON, 'utf-8'))
  console.log(`  Converted: ${geojson.features?.length || 0} features`)
  return geojson
}

/** Recursively find files with given extension (case-insensitive). */
function findFiles(dir: string, ext: string): string[] {
  const pattern = `**/*${ext.startsWith('.') ? ext : `.${ext}`}`
  return globSync(pattern, { cwd: dir })
    .filter((rel) => rel.toLowerCase().endsWith(ext.toLowerCase()))
    .sort()
    .map((rel) => resolve(dir, rel))
}

// ── Step 2: Parse GeoJSON into TGM stations grouped by ref ──

function parseStations(geojson: any): Map<string, TgmStation[]> {
  const byRef = new Map<string, TgmStation[]>()
  const features = geojson.features || []

  if (features.length > 0) {
    const props = Object.keys(features[0].properties || {})
    console.log(`  Feature properties: ${props.join(', ')}`)
  }

  let skippedNoRef = 0
  let skippedNoTgm = 0
  let skippedOutOfBounds = 0

  for (const f of features) {
    const props = f.properties || {}
    const geom = f.geometry
    if (!geom || !geom.coordinates) continue

    // Road ref
    const strada = String(props.Strada || props.strada || '').trim()
    if (!strada) { skippedNoRef++; continue }

    // TGM value
    const tgm = props.TGM ?? props.tgm
    const tgmFloat = typeof tgm === 'number' ? tgm : parseFloat(String(tgm || ''))
    // Check the ROUNDED value, not the raw float: a TGM in (0, 0.5) is truthy and > 0
    // pre-rounding but Math.round()s to zero traffic — splitTgm would then stamp
    // 0/0/0/0 under this measured id (#31.4). Zero-traffic TGM joins the same skip
    // bucket as "no TGM" — they mean the same thing downstream.
    const aadt = Math.round(tgmFloat)
    if (!aadt || isNaN(aadt) || aadt <= 0) { skippedNoTgm++; continue }

    // Coordinates (Point geometry: [lon, lat, optional z])
    const lon = geom.coordinates[0]
    const lat = geom.coordinates[1]
    if (lat < IT_BBOX.minLat || lat > IT_BBOX.maxLat ||
        lon < IT_BBOX.minLon || lon > IT_BBOX.maxLon) {
      skippedOutOfBounds++
      continue
    }

    const ref = normalizeAnasRef(strada)
    if (!ref) { skippedNoRef++; continue }

    const station: TgmStation = { ref, aadt, lat, lon }
    if (!byRef.has(ref)) byRef.set(ref, [])
    byRef.get(ref)!.push(station)
  }

  let totalStations = 0
  for (const v of byRef.values()) totalStations += v.length

  console.log(`  Parsed ${totalStations} stations across ${byRef.size} unique road refs`)
  if (skippedNoRef > 0) console.log(`  Skipped (no ref): ${skippedNoRef}`)
  if (skippedNoTgm > 0) console.log(`  Skipped (no TGM): ${skippedNoTgm}`)
  if (skippedOutOfBounds > 0) console.log(`  Skipped (out of bounds): ${skippedOutOfBounds}`)

  // AADT distribution
  const aadts: number[] = []
  for (const v of byRef.values()) for (const s of v) aadts.push(s.aadt)
  aadts.sort((a, b) => a - b)
  if (aadts.length > 0) {
    console.log(`  AADT range: ${aadts[0]} - ${aadts[aadts.length - 1]}, median: ${aadts[Math.floor(aadts.length / 2)]}`)
  }

  return byRef
}

// ── Step 3: Enrich Arrow files ──

async function enrichHexes(stationsByRef: Map<string, TgmStation[]>): Promise<void> {
  const hexDirs = iterateCountryHexes(H3R4_DIR, IT_HEX_BBOX)
  console.log(`  Italian hexes (H3 bbox): ${hexDirs.length}`)

  let totalRoads = 0
  let totalMatched = 0
  let hexesUpdated = 0
  const matchByClass = new Map<number, { matched: number; total: number }>()
  const startTime = Date.now()

  for (const hexId of hexDirs) {
    const roadsPath = resolve(H3R4_DIR, hexId, 'roads.arrow')

    // Per-hex guard (kept from the pre-migration loop, behaviour-identical): a hex
    // whose H3 centroid is inside IT_BBOX can still straddle the border, so only
    // enrich it if at least one of its first ≤20 segment midpoints is inside the
    // box. Dropping it (or applying it per-row) would change which rows match —
    // e.g. cross-border Austrian "A2" segments just north of lat 47.1 sit in
    // qualified hexes and are processed by the original, but a per-row bbox filter
    // would reject them. The sample is a read-only no-op pass (always returns null,
    // so writeRoadAadt leaves the file byte-identical); the real matching pass only
    // runs once the hex qualifies.
    let sawItalianRoad = false
    await writeRoadAadt(roadsPath, (row, i) => {
      if (i < 20 && !sawItalianRoad &&
          row.midLat >= IT_BBOX.minLat && row.midLat <= IT_BBOX.maxLat &&
          row.midLon >= IT_BBOX.minLon && row.midLon <= IT_BBOX.maxLon) {
        sawItalianRoad = true
      }
      return null
    })
    if (!sawItalianRoad) continue

    const r = await writeRoadAadt(
      roadsPath,
      (row) => {
        let cls = matchByClass.get(row.roadClass)
        if (!cls) { cls = { matched: 0, total: 0 }; matchByClass.set(row.roadClass, cls) }
        cls.total++

        // Fast-exit before the expensive ref-match when a higher-priority dataset
        // already owns the row (writeRoadAadt re-checks the gate — this only saves work).
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) return null

        // Ref match is mandatory — no proximity-only fallback.
        if (!row.ref) return null
        const normalized = normalizeOsmRef(row.ref)
        if (!normalized) return null
        const candidates = stationsByRef.get(normalized)
        if (!candidates || candidates.length === 0) return null

        // Pick closest station by distance to road segment midpoint.
        let best = candidates[0]
        let bestDist = flatDist(row.midLat, row.midLon, best.lat, best.lon)
        for (let j = 1; j < candidates.length; j++) {
          const d = flatDist(row.midLat, row.midLon, candidates[j].lat, candidates[j].lon)
          if (d < bestDist) { best = candidates[j]; bestDist = d }
        }

        // Max 30km — Italian TGM stations are sparse (~653 stations for 300K km of road).
        if (bestDist > 30_000) return null

        const split = splitTgm(best.aadt, row.roadClass)
        return {
          light: split.light, medium: split.medium,
          heavy: split.heavy, moto: split.moto, sourceId: MY_SOURCE_ID,
        }
      },
      (row) => { matchByClass.get(row.roadClass)!.matched++ },
    )
    totalRoads += r.rows
    totalMatched += r.matched
    if (r.updated) hexesUpdated++

    // Progress every 10s
    const elapsed = Date.now() - startTime
    if (elapsed > 0 && hexesUpdated % 10 === 0) {
      console.log(`  [${(elapsed / 1000).toFixed(0)}s] ${hexesUpdated} hexes updated, ${totalMatched} segments matched`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  ${totalMatched} / ${totalRoads} segments matched (${totalRoads > 0 ? (totalMatched / totalRoads * 100).toFixed(1) : 0}%)`)
  console.log(`  ${hexesUpdated} hexes updated`)
  console.log(`\n  Per road class:`)
  for (const [cls, stats] of [...matchByClass.entries()].sort((a, b) => a[0] - b[0])) {
    const names = ['motorway', 'trunk', 'primary', 'secondary', 'tertiary', 'residential', 'living_st']
    const pct = stats.total > 0 ? (stats.matched / stats.total * 100).toFixed(1) : '0.0'
    console.log(`    ${(names[cls] || `class_${cls}`).padEnd(12)} ${stats.matched} / ${stats.total} (${pct}%)`)
  }
}

// ── Helpers ──

/** Normalize ANAS road ref: "SS106" -> "SS 106", "A1" -> "A1", etc.
 *  Produces a canonical form that can be matched against OSM refs. */
function normalizeAnasRef(strada: string): string {
  const s = strada.trim()
  if (!s) return ''

  // Remove suffixes like "dir", "bis", "ter", "quater", "radd"
  const base = s.replace(/(dir(-[a-z])?|bis|ter|quater|radd)$/i, '').trim()

  // A-roads (Autostrade): A01, A1, A14, A90 etc. (strip leading zeros)
  const aMatch = base.match(/^A\s*0*(\d+)$/i)
  if (aMatch) return `A${aMatch[1]}`

  // RA-roads (Raccordi Autostradali): RA05 -> RA 5
  const raMatch = base.match(/^RA\s*0*(\d+)$/i)
  if (raMatch) return `RA ${raMatch[1]}`

  // SS-roads (Strade Statali): SS106 -> SS 106
  const ssMatch = base.match(/^SS\s*0*(\d+)$/i)
  if (ssMatch) return `SS ${ssMatch[1]}`

  // NSA-roads: NSA215 -> NSA 215
  const nsaMatch = base.match(/^NSA\s*0*(\d+)$/i)
  if (nsaMatch) return `NSA ${nsaMatch[1]}`

  // Fallback: return as-is
  return base
}

/** Normalize OSM road ref for Italian roads.
 *  OSM uses: "A1", "SS 106", "RA 5", "E45", etc. */
function normalizeOsmRef(ref: string): string {
  // OSM refs can have multiple values separated by ;
  // Try each one
  const parts = ref.split(';').map(s => s.trim()).filter(Boolean)

  for (const r of parts) {
    // Skip E-roads (European numbering)
    if (/^E\s*\d/i.test(r)) continue

    // A-roads
    const aMatch = r.match(/^A\s*(\d+)$/i)
    if (aMatch) return `A${aMatch[1]}`

    // RA-roads
    const raMatch = r.match(/^RA\s*0*(\d+)$/i)
    if (raMatch) return `RA ${raMatch[1]}`

    // SS-roads (with optional "SS " prefix)
    const ssMatch = r.match(/^(?:SS|S\.S\.)\s*0*(\d+)$/i)
    if (ssMatch) return `SS ${ssMatch[1]}`

    // SR-roads (Strade Regionali) — some were former SS roads
    const srMatch = r.match(/^SR\s*0*(\d+)$/i)
    if (srMatch) return `SS ${srMatch[1]}`

    // NSA-roads
    const nsaMatch = r.match(/^NSA\s*0*(\d+)$/i)
    if (nsaMatch) return `NSA ${nsaMatch[1]}`
  }

  return ''
}

/**
 * Split a total TGM (AADT) into CNOSSOS-EU classes. ANAS TGM carries no
 * per-class breakdown, so this is a road-class heuristic (heavy share rises
 * with road importance), consistent with the US/CA enrichers. Italian A-/SS-road
 * freight is ~5–18% of AADT by class; ~4% powered-two-wheelers; of the heavy
 * bucket ~20% are buses + 2-axle rigid (cat2 medium), ~80% artic/heavy rigid
 * (cat3 heavy). Previously the whole TGM was written to light, modelling every
 * truck and bus as a car.
 */
function splitTgm(total: number, roadClass: number): { light: number; medium: number; heavy: number; moto: number } {
  const heavyShare = roadClass <= 1 ? 0.18 : roadClass === 2 ? 0.12 : roadClass === 3 ? 0.08 : 0.05
  const moto = Math.round(total * 0.04)
  const heavyTotal = Math.round(total * heavyShare)
  const medium = Math.round(heavyTotal * 0.20)
  const heavy = heavyTotal - medium
  const light = Math.max(0, total - moto - heavyTotal)
  return { light, medium, heavy, moto }
}

// ── Main ──

async function main() {
  console.log(`=== IT Road Traffic Enrichment — ANAS TGM (${YEAR}) ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache: ${CACHE_DIR}\n`)

  if (!existsSync(H3R4_DIR)) {
    console.error(`ERROR: H3R4 directory not found: ${H3R4_DIR}`)
    process.exit(1)
  }

  const geojson = await downloadAndConvert()
  console.log(`\n  Parsing TGM stations...`)
  const stationsByRef = parseStations(geojson)

  if (stationsByRef.size === 0) {
    console.error('ERROR: No valid TGM stations found. Check shapefile properties.')
    process.exit(1)
  }

  console.log(`\n  Enriching roads.arrow files...`)
  await enrichHexes(stationsByRef)
  console.log(`\n=== Done ===`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
