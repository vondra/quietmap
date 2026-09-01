/**
 * Enrich IE roads.arrow with TII (Transport Infrastructure Ireland) traffic counter data.
 *
 * Sources:
 *   Counter sites JSON: https://data.tii.ie/Datasets/TrafficCountData/sites/tmu-sites.json
 *     ~430 counter sites with WGS84 coordinates and TMU descriptive name (road ref).
 *   Per-site daily class aggregate CSV (one date sampled):
 *     https://data.tii.ie/Datasets/TrafficCountData/2019/06/19/per-site-class-aggr-2019-06-19.csv
 *     One typical Wednesday in June 2019 (pre-COVID baseline). Each site has counts
 *     per vehicle class (1-7).
 *
 * Classes:
 *   1 = motorcycle
 *   2 = car
 *   3 = car with trailer / light van (LGV)
 *   4 = 2-axle rigid truck
 *   5 = 3-axle rigid truck
 *   6 = 4-axle rigid truck
 *   7 = articulated truck (HGV)
 *
 * CNOSSOS mapping:
 *   light  = class 2 + 3
 *   medium = class 4
 *   heavy  = class 5 + 6 + 7
 *   moto   = class 1
 *
 * The daily count is treated as AADT (typical weekday is a reasonable proxy).
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-ie.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-ie.ts --enrich-only
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-ie.ts --force-download
 */

import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_IE_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { haversineM } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_IE_NATIONAL_ROADS

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/ie`)
const CACHE_SITES_JSON = resolve(CACHE_DIR, 'tii-tmu-sites.json')
const CACHE_AGGR_CSV = resolve(CACHE_DIR, 'tii-aggr-2019-06-19.csv')
const CACHE_PARSED = resolve(CACHE_DIR, 'tii-parsed-aadt.json')

const enrichOnly = process.argv.includes('--enrich-only')
const forceDownload = process.argv.includes('--force-download')

const SITES_URL = 'https://data.tii.ie/Datasets/TrafficCountData/sites/tmu-sites.json'
// Wednesday 19 June 2019 — pre-COVID typical weekday
const AGGR_URL = 'https://data.tii.ie/Datasets/TrafficCountData/2019/06/19/per-site-class-aggr-2019-06-19.csv'

const IE_BBOX: [number, number, number, number] = [51.4, -10.5, 55.4, -5.4]
// Hex-scan box (same extent as IE_BBOX): [minLat,minLon,maxLat,maxLon]. iterateCountryHexes
// skips the rest of the planet so the loader doesn't read every roads.arrow on Earth.
const IE_HEX_BBOX: [number, number, number, number] = IE_BBOX

interface Site {
  cosit: string
  ref: string         // normalized road ref e.g. "M1", "N7", "R132"
  lat: number
  lon: number
  description: string
}

interface SiteAadt extends Site {
  aadt_total: number
  aadt_light: number
  aadt_medium: number
  aadt_heavy: number
  aadt_moto: number
}

async function downloadIfMissing(url: string, path: string, label: string): Promise<void> {
  if (!forceDownload && existsSync(path)) {
    console.log(`  Using cached ${label}: ${path}`)
    return
  }
  if (enrichOnly) throw new Error(`--enrich-only but ${label} not cached`)
  console.log(`  Downloading ${label}...`)
  const res = await fetch(url, { signal: AbortSignal.timeout(120_000) })
  if (!res.ok) throw new Error(`HTTP ${res.status} for ${label}`)
  const buf = Buffer.from(await res.arrayBuffer())
  writeFileSync(path, buf)
  console.log(`  Cached: ${(buf.length / 1e3).toFixed(1)} KB`)
}

function parseSites(): Map<string, Site> {
  const data = JSON.parse(readFileSync(CACHE_SITES_JSON, 'utf-8'))
  const map = new Map<string, Site>()
  let skipped = 0
  for (const s of data) {
    const lat = s.location?.lat
    const lon = s.location?.lng
    if (!lat || !lon || lat === 0 || lon === 0) { skipped++; continue }
    if (lat < IE_BBOX[0] || lat > IE_BBOX[2] || lon < IE_BBOX[1] || lon > IE_BBOX[3]) { skipped++; continue }

    // Extract road ref from name like "TMU M01 000.0 N", "TMU N01 040.0 S", "M50 0001"
    const name = (s.name || '').toString().trim()
    const refMatch = name.match(/([MNRL]\d+)/i)
    const refRaw = refMatch ? refMatch[1].toUpperCase() : ''
    // Strip leading zeros: M01 → M1, N01 → N1
    const ref = refRaw.replace(/^([MNRL])0*/, '$1')

    map.set(s.cosit, {
      cosit: s.cosit,
      ref,
      lat,
      lon,
      description: s.description || '',
    })
  }
  console.log(`  Parsed ${map.size} sites (skipped ${skipped} test/oob/no-coords)`)
  return map
}

function parseAggregate(sites: Map<string, Site>): SiteAadt[] {
  const lines = readFileSync(CACHE_AGGR_CSV, 'utf-8').split('\n')
  // header: "cosit","class","year","month","day","VehicleCount"
  const counts = new Map<string, Map<number, number>>() // cosit → class → count
  for (let i = 1; i < lines.length; i++) {
    const line = lines[i].trim()
    if (!line) continue
    const parts = line.split(',').map(p => p.replace(/"/g, ''))
    if (parts.length < 6) continue
    const cosit = parts[0]
    const cls = parseInt(parts[1])
    const cnt = parseInt(parts[5])
    if (!cosit || isNaN(cls) || isNaN(cnt)) continue
    if (!counts.has(cosit)) counts.set(cosit, new Map())
    counts.get(cosit)!.set(cls, (counts.get(cosit)!.get(cls) || 0) + cnt)
  }
  console.log(`  Aggregated ${counts.size} unique cosits with class counts`)

  const results: SiteAadt[] = []
  for (const [cosit, classMap] of counts) {
    const site = sites.get(cosit)
    if (!site || !site.ref) continue
    const c1 = classMap.get(1) || 0
    const c2 = classMap.get(2) || 0
    const c3 = classMap.get(3) || 0
    const c4 = classMap.get(4) || 0
    const c5 = classMap.get(5) || 0
    const c6 = classMap.get(6) || 0
    const c7 = classMap.get(7) || 0
    const total = c1 + c2 + c3 + c4 + c5 + c6 + c7
    if (total < 100) continue // skip empty/test counters

    results.push({
      ...site,
      aadt_total: total,
      aadt_light: c2 + c3,
      aadt_medium: c4,
      aadt_heavy: c5 + c6 + c7,
      aadt_moto: c1,
    })
  }
  console.log(`  Built ${results.length} site AADT records`)
  return results
}

async function enrichArrows(sites: SiteAadt[]): Promise<void> {
  const refIndex = new Map<string, SiteAadt[]>()
  for (const s of sites) {
    if (!refIndex.has(s.ref)) refIndex.set(s.ref, [])
    refIndex.get(s.ref)!.push(s)
  }
  console.log(`\n  Ref index: ${refIndex.size} unique refs`)

  const hexDirs = iterateCountryHexes(H3R4_DIR, IE_HEX_BBOX)
  console.log(`  IE hexes with roads.arrow: ${hexDirs.length}\n`)

  let totalSeg = 0
  let matched = 0
  let preserved = 0
  let hexesUpdated = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        // Priority gate: preserve existing if it has higher priority than self.
        // Fast-exit before the ref-match (writeRoadAadt re-checks — this only saves work).
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) {
          preserved++
          return null
        }

        const osmRefRaw = (row.ref?.toString() || '').trim().toUpperCase()
        // Irish OSM refs: "M1", "M50", "N7", "R132", "L1234"
        const firstRef = osmRefRaw.split(';')[0].replace(/[-\s]/g, '').replace(/^([MNRL])0*/, '$1')

        let best: SiteAadt | null = null
        let bestDist = Infinity

        if (firstRef && refIndex.has(firstRef)) {
          for (const s of refIndex.get(firstRef)!) {
            const d = haversineM(row.midLat, row.midLon, s.lat, s.lon)
            if (d < bestDist) { bestDist = d; best = s }
          }
        }

        // Match within 25 km along ref
        if (best && bestDist < 25_000) {
          return {
            light: best.aadt_light, medium: best.aadt_medium,
            heavy: best.aadt_heavy, moto: best.aadt_moto, sourceId: MY_SOURCE_ID,
          }
        }
        return null
      },
      () => { matched++ },
    )
    totalSeg += r.rows
    if (r.updated) hexesUpdated++

    if (hi % 10 === 0 || hi === hexDirs.length - 1) {
      const elapsed = ((Date.now() - startTime) / 1000).toFixed(0)
      console.log(`  [${elapsed}s] ${hi + 1}/${hexDirs.length} hexes, ${hexesUpdated} updated, ${matched} matched`)
    }
  }

  console.log(`\n=== Enrichment Results ===`)
  console.log(`  Total segments scanned: ${totalSeg}`)
  console.log(`  Preserved (continental): ${preserved}`)
  console.log(`  Newly matched: ${matched} (${(100 * matched / Math.max(totalSeg, 1)).toFixed(2)}%)`)
  console.log(`  Hexes updated: ${hexesUpdated}/${hexDirs.length}`)

  // Top corridors
  const top = [...sites].sort((a, b) => b.aadt_total - a.aadt_total).slice(0, 15)
  console.log(`\n  Top 15 AADT counter sites:`)
  for (const s of top) {
    console.log(`    ${s.ref.padEnd(6)} ${s.cosit}  total=${s.aadt_total.toLocaleString().padStart(7)} heavy=${s.aadt_heavy.toLocaleString().padStart(5)} (${s.description.substring(0, 50)})`)
  }
}

async function main() {
  console.log(`=== IE Road Traffic Enrichment — TII Counter Sites (2019 weekday) ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache:    ${CACHE_DIR}\n`)

  if (!existsSync(H3R4_DIR)) throw new Error(`H3R4 directory not found: ${H3R4_DIR}`)

  mkdirSync(CACHE_DIR, { recursive: true })
  await downloadIfMissing(SITES_URL, CACHE_SITES_JSON, 'TII counter sites JSON')
  await downloadIfMissing(AGGR_URL, CACHE_AGGR_CSV, 'TII aggregate counts CSV (2019-06-19)')

  const sites = parseSites()
  const aadt = parseAggregate(sites)

  // Stats
  const refCounts = new Map<string, number>()
  for (const s of aadt) {
    const prefix = s.ref.match(/^[A-Z]/)?.[0] || '?'
    refCounts.set(prefix, (refCounts.get(prefix) || 0) + 1)
  }
  console.log(`  Ref prefix distribution:`)
  for (const [p, n] of [...refCounts.entries()].sort((a, b) => b[1] - a[1])) {
    console.log(`    ${p}: ${n}`)
  }

  await enrichArrows(aadt)
  console.log(`\n=== Done ===`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
