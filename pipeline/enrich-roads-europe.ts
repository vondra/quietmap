/**
 * Continental road enrichment (Europe): EU city traffic volume dataset.
 *
 * Downloads harmonized traffic data from 36 European cities (Nature Scientific Data, 2025),
 * matches to OSM road segments by nearest-point proximity, writes aadt_light + aadt_medium +
 * aadt_heavy + aadt_moto + source_id into roads.arrow for each matching H3R4 hex.
 *
 * Dataset: "Harmonized Annual Averaged Traffic Data at Street Segment Level for European Cities"
 * GitHub: https://github.com/XavB64/traffic-volume-data-EU-cities
 * License: CC BY 4.0
 *
 * Each city's treated/ folder contains GeoJSON files with AADT + optional TR_AADT (truck AADT).
 * We take each record's point / line-midpoint and match it to the nearest OSM road segment
 * midpoint within a tight 50 m cap (per-hex spatial grid).
 *
 * Usage:
 *   cd pipeline && npx tsx enrich-roads-europe.ts
 *   cd pipeline && npx tsx enrich-roads-europe.ts --force-download
 *   cd pipeline && npx tsx enrich-roads-europe.ts --enrich-only
 */

import { readFileSync, writeFileSync, existsSync, mkdirSync, readdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { latLngToCell } from 'h3-js'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_EU_CITY_TRAFFIC } from './lib/source-ids.generated.js'
import { flatDist } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_EU_CITY_TRAFFIC

const CACHE_DIR = resolve(import.meta.dirname, '../data/enrichment/global/eu-city-traffic')

const forceDownload = process.argv.includes('--force-download')
const enrichOnly = process.argv.includes('--enrich-only')

const REPO_BASE = 'https://raw.githubusercontent.com/XavB64/traffic-volume-data-EU-cities/main'
const API_BASE = 'https://api.github.com/repos/XavB64/traffic-volume-data-EU-cities/contents'

// Cities with treated_data=True from cities_summary.csv.
// Format: [country_folder, city_folder, city_name]
// We pick the latest year available for each city.
const CITIES: [string, string, string][] = [
  ['Austria', 'Vienna', 'Vienna'],
  ['Czechia', 'Brno', 'Brno'],
  ['Denmark', 'Copenhagen', 'Copenhagen'],
  ['Finland', 'Helsinki', 'Helsinki'],
  ['France', 'Paris', 'Paris'],
  ['France', 'Grenoble', 'Grenoble'],
  ['France', 'Toulouse', 'Toulouse'],
  ['France', 'Lyon', 'Lyon'],
  ['France', 'Lille', 'Lille'],
  ['France', 'Bordeaux', 'Bordeaux'],
  ['France', 'Rennes', 'Rennes'],
  ['France', 'Marseille', 'Marseille'],
  ['France', 'Rouen', 'Rouen'],
  ['France', 'Montpellier', 'Montpellier'],
  ['France', 'Tours', 'Tours'],
  ['Germany', 'Berlin', 'Berlin'],
  ['Germany', 'Hamburg', 'Hamburg'],
  ['Ireland', 'Dublin', 'Dublin'],
  ['Italy', 'Milan', 'Milan'],
  ['Luxembourg', 'Luxembourg', 'Luxembourg'],
  ['Netherlands', 'Amsterdam', 'Amsterdam'],
  ['Norway', 'Oslo', 'Oslo'],
  ['Portugal', 'Lisbon', 'Lisbon'],
  ['Spain', 'Valencia', 'Valencia'],
  ['Spain', 'Barcelona', 'Barcelona'],
  ['Spain', 'Madrid', 'Madrid'],
  ['Sweden', 'Malmo', 'Malmo'],
  ['Sweden', 'Stockholm', 'Stockholm'],
  ['Switzerland', 'Zurich', 'Zurich'],
  ['Switzerland', 'Geneva', 'Geneva'],
  ['United Kingdom', 'London', 'London'],
  ['United Kingdom', 'Birmingham', 'Birmingham'],
  ['United Kingdom', 'Manchester', 'Manchester'],
  ['United Kingdom', 'Glasgow', 'Glasgow'],
  ['United Kingdom', 'Edinburgh', 'Edinburgh'],
  ['United Kingdom', 'Cardiff', 'Cardiff'],
]

// ── Types ──

interface TrafficRecord {
  aadt: number          // total AADT (vehicles/day)
  truckAadt: number     // truck AADT (0 if not available)
  twoWheelAadt: number  // 2-wheel AADT (0 if not available)
  isOneway: boolean     // raw_oneway — if true, AADT is one-direction only
  lat: number
  lon: number
}

// ── Step 1: Download GeoJSON files ──

/** Discover treated GeoJSON files for a city via GitHub API */
async function discoverFiles(country: string, city: string): Promise<string[]> {
  const path = encodeURIComponent(`${country}/${city}/treated`)
    .replace(/%2F/g, '/')
  const url = `${API_BASE}/${path}`

  const res = await fetch(url, {
    signal: AbortSignal.timeout(30_000),
    headers: { 'Accept': 'application/vnd.github.v3+json' },
  })
  if (!res.ok) {
    if (res.status === 404) return []
    throw new Error(`GitHub API ${res.status} for ${url}`)
  }

  const items = await res.json() as { name: string; download_url: string }[]
  return items
    .filter(i => i.name.endsWith('.geojson') || i.name.endsWith('.GeoJSON'))
    .map(i => i.name)
    .sort()  // alphabetical = chronological for same-format names
}

/** Pick the latest-year file from a list of GeoJSON filenames */
function pickLatestFile(files: string[]): string | null {
  if (files.length === 0) return null

  // Extract year from filenames like "Berlin_AADT_AAWT_2023.geojson"
  let best: string | null = null
  let bestYear = 0
  for (const f of files) {
    const m = f.match(/(\d{4})\.geojson$/i)
    if (m) {
      const year = parseInt(m[1])
      if (year > bestYear) {
        bestYear = year
        best = f
      }
    }
  }
  return best || files[files.length - 1]  // fallback to last alphabetically
}

/** #31.6: the treated raws stage as `<CityName>_<METRIC>_<YEAR>.geojson`, but
 *  the loader consumes only the normalized `<city>.geojson`. Under --enrich-only
 *  (no network) 34/36 cities were never normalized, so the world chain stamped
 *  a 2-city fragment and passed. Pick the latest-year raw for a city straight
 *  from the cache dir — the exact file `discoverFiles`+`pickLatestFile` would
 *  have chosen online — so --enrich-only reaches 36/36 without a download. */
export function pickLatestRawForCity(cityName: string, dir: string = CACHE_DIR): string | null {
  const prefix = `${cityName.toLowerCase()}_`
  const raws = readdirSync(dir).filter(
    (f) => f.toLowerCase().startsWith(prefix) && /\d{4}\.geojson$/i.test(f),
  )
  const chosen = pickLatestFile(raws)
  return chosen ? resolve(dir, chosen) : null
}

/** Download a single city's GeoJSON, caching locally */
async function downloadCity(
  country: string, city: string, cityName: string
): Promise<any | null> {
  const cacheFile = resolve(CACHE_DIR, `${cityName.toLowerCase()}.geojson`)

  // Check cache
  if (enrichOnly || (!forceDownload && existsSync(cacheFile))) {
    if (!existsSync(cacheFile)) {
      // Normalize from a staged raw before giving up — the 2/36→36/36 fix.
      const raw = pickLatestRawForCity(cityName)
      if (raw) {
        const text = readFileSync(raw, 'utf-8')
        writeFileSync(cacheFile, text) // materialize <city>.geojson for future runs
        console.log(`  ${cityName}: normalized from ${raw.split('/').pop()}`)
        return JSON.parse(text)
      }
      console.log(`  SKIP ${cityName}: --enrich-only but no cache and no staged raw`)
      return null
    }
    return JSON.parse(readFileSync(cacheFile, 'utf-8'))
  }

  // Discover available files
  const files = await discoverFiles(country, city)
  const chosen = pickLatestFile(files)
  if (!chosen) {
    console.log(`  SKIP ${cityName}: no treated GeoJSON files found`)
    return null
  }

  // Download
  const encodedPath = `${country}/${city}/treated/${chosen}`
    .split('/').map(encodeURIComponent).join('/')
  const url = `${REPO_BASE}/${encodedPath}`
  const res = await fetch(url, { signal: AbortSignal.timeout(120_000) })
  if (!res.ok) {
    console.log(`  SKIP ${cityName}: download failed (${res.status})`)
    return null
  }

  const text = await res.text()
  mkdirSync(CACHE_DIR, { recursive: true })
  writeFileSync(cacheFile, text)

  const data = JSON.parse(text)
  const n = data.features?.length || 0
  console.log(`  ${cityName}: downloaded ${chosen} (${n} features, ${(text.length / 1024).toFixed(0)} KB)`)
  return data
}

// ── Step 2: Parse GeoJSON → TrafficRecords grouped by H3R4 hex ──

function parseCity(geojson: any): Map<string, TrafficRecord[]> {
  const byHex = new Map<string, TrafficRecord[]>()
  const features = geojson.features || []

  for (const f of features) {
    const props = f.properties || {}
    const geom = f.geometry

    // Must have AADT
    const aadt = props.AADT ?? props.AAWT ?? 0
    if (!aadt || aadt <= 0) continue

    const truckAadt = props.TR_AADT ?? props.TR_AAWT ?? 0
    const twoWheelAadt = props['2W_AADT'] ?? props['2W_AAWT'] ?? 0
    const isOneway = props.raw_oneway === true

    // Get representative coordinate for H3 hex lookup
    let lat: number, lon: number
    if (!geom || !geom.coordinates) continue

    if (geom.type === 'Point') {
      lon = geom.coordinates[0]
      lat = geom.coordinates[1]
    } else if (geom.type === 'LineString') {
      // Midpoint of line
      const coords = geom.coordinates as number[][]
      const mid = Math.floor(coords.length / 2)
      lon = coords[mid][0]
      lat = coords[mid][1]
    } else {
      continue
    }

    if (isNaN(lat) || isNaN(lon)) continue

    // Compute H3R4 hex
    let h3r4: string
    try {
      h3r4 = latLngToCell(lat, lon, 4)
    } catch {
      continue
    }

    const record: TrafficRecord = {
      aadt: Math.round(aadt),
      truckAadt: Math.max(0, Math.round(truckAadt)),
      twoWheelAadt: Math.max(0, Math.round(twoWheelAadt)),
      isOneway,
      lat,
      lon,
    }

    let list = byHex.get(h3r4)
    if (!list) {
      list = []
      byHex.set(h3r4, list)
    }
    list.push(record)
  }

  return byHex
}

// ── Step 3: Enrich Arrow files ──

// Generous Europe box (Iceland/Canaries → Urals/Cyprus) so the hex scan skips the
// rest of the planet instead of reading every roads.arrow on Earth (~40 min → minutes).
// [minLat, minLon, maxLat, maxLon].
const EU_HEX_BBOX: [number, number, number, number] = [34, -32, 72, 45]

async function enrichHexes(allRecords: Map<string, TrafficRecord[]>): Promise<{
  totalRoads: number
  totalMatched: number
  hexesUpdated: number
  matchByClass: Map<number, { matched: number; total: number }>
}> {
  const hexDirs = iterateCountryHexes(H3R4_DIR, EU_HEX_BBOX)

  let totalRoads = 0
  let totalMatched = 0
  let hexesUpdated = 0
  const matchByClass = new Map<number, { matched: number; total: number }>()

  let lastProgress = Date.now()
  let hexesProcessed = 0

  for (const hexId of hexDirs) {
    hexesProcessed++

    // Progress every 10s
    if (Date.now() - lastProgress > 10_000) {
      console.log(`  progress: ${hexesProcessed}/${hexDirs.length} hexes, ${totalMatched} matched so far`)
      lastProgress = Date.now()
    }

    // Check if this hex has any traffic records
    const records = allRecords.get(hexId)
    if (!records || records.length === 0) continue

    // Build spatial grid from traffic records for fast proximity lookup
    const CELL = 0.001 // ~111m grid cells
    const grid = new Map<string, TrafficRecord[]>()
    for (const r of records) {
      const key = `${Math.floor(r.lat / CELL)},${Math.floor(r.lon / CELL)}`
      if (!grid.has(key)) grid.set(key, [])
      grid.get(key)!.push(r)
    }

    const MAX_DIST = 50 // meters

    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hexId, 'roads.arrow'),
      (row) => {
        let cls = matchByClass.get(row.roadClass)
        if (!cls) { cls = { matched: 0, total: 0 }; matchByClass.set(row.roadClass, cls) }
        cls.total++

        // Priority check: if a higher-priority dataset already owns this row, leave it
        // alone (writeRoadAadt re-checks the gate — this only saves the proximity work).
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) return null

        // Search nearby grid cells for the closest traffic record to the segment midpoint.
        const cy = Math.floor(row.midLat / CELL)
        const cx = Math.floor(row.midLon / CELL)
        let bestDist = MAX_DIST + 1
        let record: TrafficRecord | null = null
        for (let dy = -1; dy <= 1; dy++) {
          for (let dx = -1; dx <= 1; dx++) {
            const candidates = grid.get(`${cy + dy},${cx + dx}`)
            if (!candidates) continue
            for (const c of candidates) {
              const d = flatDist(row.midLat, row.midLon, c.lat, c.lon)
              if (d < bestDist) { bestDist = d; record = c }
            }
          }
        }
        if (!record) return null

        // Convert directional → bidirectional total.
        // Arrow stores bidirectional total; tile-painter applies oneway_factor=0.5.
        // Dataset raw_oneway=true means the measurement is for ONE direction only.
        const dirFactor = record.isOneway ? 2 : 1

        const totalAadt = record.aadt * dirFactor
        const heavyAadt = record.truckAadt * dirFactor
        const motoAadt = record.twoWheelAadt * dirFactor
        // The dataset only splits out trucks (heavy) + two-wheelers (moto); cars and
        // buses share the remainder. Estimate buses (CNOSSOS cat2 medium) at ~2% of
        // total — a typical urban bus share — rather than dumping them into light.
        const mediumAadt = totalAadt * 0.02
        const lightAadt = totalAadt - heavyAadt - motoAadt - mediumAadt

        const light = Math.max(0, Math.round(lightAadt))
        const medium = Math.max(0, Math.round(mediumAadt))
        const heavy = Math.max(0, Math.round(heavyAadt))
        const moto = Math.max(0, Math.round(motoAadt))
        // record.aadt is itself a rounded value (parseCity) and can round below 1
        // for a sub-0.5 raw reading — every class then rounds to zero. Never stamp
        // that under this MEASURED id (#31.4); the row falls through to a
        // lower-priority enricher instead.
        if (light + medium + heavy + moto === 0) return null
        return { light, medium, heavy, moto, sourceId: MY_SOURCE_ID }
      },
      (row) => { matchByClass.get(row.roadClass)!.matched++ },
    )

    totalRoads += r.rows
    totalMatched += r.matched
    if (r.updated) hexesUpdated++
  }

  return { totalRoads, totalMatched, hexesUpdated, matchByClass }
}

// ── Main ──

async function main() {
  console.log(`=== EU City Traffic Enrichment (${YEAR}) ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache: ${CACHE_DIR}\n`)

  if (!existsSync(H3R4_DIR)) {
    console.error(`ERROR: H3R4 directory not found: ${H3R4_DIR}`)
    process.exit(1)
  }

  // Download and parse all cities
  const allRecords = new Map<string, TrafficRecord[]>()
  let totalFeatures = 0
  let citiesLoaded = 0
  let lastProgress = Date.now()

  for (const [country, city, cityName] of CITIES) {
    if (Date.now() - lastProgress > 10_000) {
      console.log(`  download progress: ${citiesLoaded}/${CITIES.length} cities`)
      lastProgress = Date.now()
    }

    try {
      const geojson = await downloadCity(country, city, cityName)
      if (!geojson) continue

      const byHex = parseCity(geojson)
      let cityFeatures = 0

      for (const [hex, records] of byHex) {
        let existing = allRecords.get(hex)
        if (!existing) {
          existing = []
          allRecords.set(hex, existing)
        }
        existing.push(...records)
        cityFeatures += records.length
      }

      totalFeatures += cityFeatures
      citiesLoaded++
      console.log(`  ${cityName}: ${cityFeatures} records in ${byHex.size} hexes`)
    } catch (err) {
      console.log(`  SKIP ${cityName}: ${(err as Error).message}`)
    }
  }

  console.log(`\n  Total: ${totalFeatures} traffic records from ${citiesLoaded} cities in ${allRecords.size} hexes\n`)

  // #31.6 completeness marker → chain status.json + the completeness gate.
  // `complete` iff every declared city loaded; else `partial` (a 2/36 fragment
  // must never silently stamp as done again).
  const compState = citiesLoaded >= CITIES.length ? 'complete' : citiesLoaded === 0 ? 'missing' : 'partial'
  console.log(`QM_COMPLETENESS ${JSON.stringify({ actual: citiesLoaded, state: compState, detail: `${citiesLoaded}/${CITIES.length} cities` })}`)

  if (totalFeatures === 0) {
    console.log('  No traffic records to enrich. Done.')
    return
  }

  // Enrich Arrow files
  console.log('  Enriching Arrow files...')
  const { totalRoads, totalMatched, hexesUpdated, matchByClass } = await enrichHexes(allRecords)

  console.log(`\n=== Results ===`)
  console.log(`  ${totalMatched} / ${totalRoads} segments enriched in matched hexes`)
  console.log(`  ${hexesUpdated} / ${allRecords.size} hexes updated`)
  console.log(`\n  Per road class:`)

  const classNames = ['motorway', 'trunk', 'primary', 'secondary', 'tertiary', 'residential', 'living_st']
  for (const [cls, stats] of [...matchByClass.entries()].sort((a, b) => a[0] - b[0])) {
    const pct = stats.total > 0 ? (stats.matched / stats.total * 100).toFixed(1) : '0.0'
    console.log(`    ${(classNames[cls] || `class_${cls}`).padEnd(12)} ${stats.matched} / ${stats.total} (${pct}%)`)
  }

  // Write provenance
  const provPath = resolve(CACHE_DIR, 'provenance.md')
  const provenance = `# EU City Traffic Enrichment Provenance

## Sources used
- **EU city traffic**: "Harmonized Annual Averaged Traffic Data at Street Segment Level for European Cities"
  - GitHub: https://github.com/XavB64/traffic-volume-data-EU-cities
  - Paper: Nature Scientific Data, 2025
  - License: CC BY 4.0
  - ${citiesLoaded} cities, ${totalFeatures} traffic records
  - Downloaded: ${new Date().toISOString().split('T')[0]}

## Matching
- Nearest-point proximity (segment midpoint → closest record within 50 m)
- ${totalMatched} segments enriched across ${hexesUpdated} H3R4 hexes
- Preserves existing country-specific enrichment (higher-rank source_id from prior runs)
- Directional correction: raw_oneway=true measurements doubled to bidirectional total
- AADT split: medium = 2% of total (bus estimate); light = total - truck - 2wheel - medium; heavy = TR_AADT; moto = 2W_AADT

## Cities included
${CITIES.map(c => `- ${c[2]} (${c[0]})`).join('\n')}

## Gaps
- No vehicle class breakdown for many cities (only total AADT)
- Point geometries (sensor locations) matched via spatial proximity
- Some cities have low OSM matching rates (see dataset's osm_distance column)
`
  writeFileSync(provPath, provenance)

  console.log(`\n=== Done ===`)
}

// Run only as a script — importable for tests (pickLatestRawForCity).
if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch(err => { console.error('Error:', err); process.exit(1) })
}
