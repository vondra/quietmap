/**
 * Enrich GB roads.arrow with DfT AADF traffic counts.
 *
 * Downloads DfT road traffic statistics ZIP, extracts the most recent AADF per
 * count point, matches to OSM roads by road ref + proximity.
 *
 * Vehicle class mapping (DfT → CNOSSOS):
 *   cars_and_taxis + LGVs          → aadt_light   (Category 1)
 *   buses_and_coaches              → aadt_medium  (Category 2)
 *   all_HGVs                       → aadt_heavy   (Category 3)
 *   two_wheeled_motor_vehicles     → aadt_moto    (Category 4)
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-gb.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-gb.ts --enrich-only
 */

import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { execSync } from 'node:child_process'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_GB_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { haversineM } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_GB_NATIONAL_ROADS

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/gb`)
const CACHE_ZIP = resolve(CACHE_DIR, 'dft-aadf.zip')
const CACHE_JSON = resolve(CACHE_DIR, 'dft-aadf.json')

const enrichOnly = process.argv.includes('--enrich-only')
const forceDownload = process.argv.includes('--force-download')

const DFT_URL = 'https://storage.googleapis.com/dft-statistics/road-traffic/downloads/data-gov-uk/dft_traffic_counts_aadf.zip'

interface CountPoint {
  ref: string
  lat: number
  lon: number
  road_category: string  // M, A, B, C, U
  aadt_light: number
  aadt_medium: number
  aadt_heavy: number
  aadt_moto: number
  total: number
  year: number
}

async function downloadDft(): Promise<CountPoint[]> {
  if (!forceDownload && existsSync(CACHE_JSON)) {
    console.log(`  Using cached: ${CACHE_JSON}`)
    // The cache predates the class-split filter in parseCsv below — a count point
    // can carry a positive all_motor_vehicles total while the five individual class
    // columns are blank (the BASt/DE shape, #31.4); the same filter must run on
    // BOTH load paths or a stale cache re-trips writeRoadAadt's measured-zero guard.
    const cached: CountPoint[] = JSON.parse(readFileSync(CACHE_JSON, 'utf-8'))
    const usable = cached.filter((p) => p.aadt_light + p.aadt_medium + p.aadt_heavy + p.aadt_moto > 0)
    if (usable.length < cached.length) {
      console.log(`  ${cached.length - usable.length} cached points dropped (AADF total without class split — cannot stamp zeros under a measured id)`)
    }
    return usable
  }

  mkdirSync(CACHE_DIR, { recursive: true })

  if (!existsSync(CACHE_ZIP) || forceDownload) {
    if (enrichOnly) { console.error('ERROR: --enrich-only but no cache'); process.exit(1) }
    console.log('  Downloading DfT AADF...')
    const res = await fetch(DFT_URL, { signal: AbortSignal.timeout(120_000) })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    writeFileSync(CACHE_ZIP, Buffer.from(await res.arrayBuffer()))
    console.log(`  Cached ZIP: ${(readFileSync(CACHE_ZIP).length / 1e6).toFixed(1)} MB`)
  }

  // Extract CSV
  const csvPath = resolve(CACHE_DIR, 'dft_traffic_counts_aadf.csv')
  if (!existsSync(csvPath)) {
    execSync(`unzip -o "${CACHE_ZIP}" -d "${CACHE_DIR}"`, { stdio: 'pipe' })
  }

  return parseCsv(csvPath)
}

function parseCsv(csvPath: string): CountPoint[] {
  console.log('  Parsing DfT AADF CSV...')
  const lines = readFileSync(csvPath, 'utf-8').split('\n')
  const header = lines[0].split(',').map(h => h.replace(/"/g, ''))

  const ci = (name: string) => header.indexOf(name)
  const iCpId = ci('count_point_id')
  const iYear = ci('year')
  const iRoad = ci('road_name')
  const iCat = ci('road_category')
  const iLat = ci('latitude')
  const iLon = ci('longitude')
  const iCars = ci('cars_and_taxis')
  const iLGV = ci('LGVs')
  const iBus = ci('buses_and_coaches')
  const iHGV = ci('all_HGVs')
  const iMoto = ci('two_wheeled_motor_vehicles')
  const iTotal = ci('all_motor_vehicles')

  // Keep most recent year per count point
  const latest = new Map<string, CountPoint>()

  for (let i = 1; i < lines.length; i++) {
    const cols = lines[i].split(',').map(c => c.replace(/"/g, ''))
    const cpId = cols[iCpId]
    const year = parseInt(cols[iYear])
    const lat = parseFloat(cols[iLat])
    const lon = parseFloat(cols[iLon])

    if (!cpId || !year || !lat || !lon || lat < 49 || lat > 61) continue

    const existing = latest.get(cpId)
    if (existing && existing.year >= year) continue

    const road = cols[iRoad] || ''
    // Normalize: "M1" stays "M1", "A1(M)" → "A1(M)", "A3111" → "A3111"
    const ref = road.replace(/\s+/g, '')

    latest.set(cpId, {
      ref,
      lat, lon,
      road_category: cols[iCat] || '',
      aadt_light: (parseInt(cols[iCars]) || 0) + (parseInt(cols[iLGV]) || 0),
      aadt_medium: parseInt(cols[iBus]) || 0,
      aadt_heavy: parseInt(cols[iHGV]) || 0,
      aadt_moto: parseInt(cols[iMoto]) || 0,
      total: parseInt(cols[iTotal]) || 0,
      year,
    })
  }

  const all = [...latest.values()].filter(p => p.total > 0)
  // Age gate: keep only points re-counted within the dataset's last 10 years (year >
  // maxYear−10); anything older is a ghost of a re-routed road — DfT never re-counts
  // detrunked sections. Live case: A168 at Wetherby,
  // last counted 2006 (69,891 = the old A1) vs its 2024 neighbours at 1,540-4,112; the
  // A1(M) that took the traffic opened 2009. 10,582 of 44,319 points (24 %) are that old,
  // 186 of them motorway-calibre (>30k) — measured 2026-07-03, task #14.
  const maxYear = Math.max(...all.map(p => p.year))
  const recent = all.filter(p => p.year > maxYear - 10)
  // all_motor_vehicles (total, checked above) is a separate DfT column from the five
  // per-class columns — a blank cars/LGV/bus/HGV/moto cell silently becomes 0 via
  // `|| 0` while total stays positive (the BASt/DE shape, #31.4): stamping that under
  // this measured id would write 0/0/0/0. Skip and count instead.
  const points = recent.filter(p => p.aadt_light + p.aadt_medium + p.aadt_heavy + p.aadt_moto > 0)
  const skippedNoSplit = recent.length - points.length
  writeFileSync(CACHE_JSON, JSON.stringify(points))
  console.log(`  ${points.length} count points (most recent year per point; dropped ${all.length - recent.length} not re-counted since ${maxYear - 10}, ${skippedNoSplit} skipped (AADF total without class split))`)
  console.log(`  By category: M=${points.filter(p=>p.road_category==='M').length} A=${points.filter(p=>p.road_category==='PA'||p.road_category==='TA').length} B=${points.filter(p=>p.road_category==='PB'||p.road_category==='TB').length}`)
  return points
}

// UK bbox; [minLat,minLon,maxLat,maxLon]. iterateCountryHexes skips the rest of
// the planet so the loader doesn't read every roads.arrow on Earth. (Same numeric
// bounds as the old readdir filter — DfT refs only match GB hexes.)
const GB_HEX_BBOX: [number, number, number, number] = [49, -8.5, 61, 2.5]

async function enrichArrows(points: CountPoint[]) {
  const refIndex = new Map<string, CountPoint[]>()
  for (const p of points) {
    if (!p.ref) continue
    const list = refIndex.get(p.ref) || []
    list.push(p)
    refIndex.set(p.ref, list)
  }
  console.log(`\n  Ref index: ${refIndex.size} unique road refs`)

  const hexDirs = iterateCountryHexes(H3R4_DIR, GB_HEX_BBOX)
  console.log(`  UK hexes: ${hexDirs.length}\n`)

  let totalSeg = 0, matched = 0, preserved = 0, hexesUpdated = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        totalSeg++
        // Priority gate: preserve existing if it has higher priority than self.
        // (writeRoadAadt re-checks the gate — this fast-exit only saves work and
        // keeps the `preserved` counter identical to the pre-helper code.)
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) {
          preserved++
          return null
        }

        const osmRef = row.ref?.toString().trim() || ''
        const normRef = osmRef.replace(/\s+/g, '')

        let best: CountPoint | null = null
        let bestDist = Infinity

        if (normRef && refIndex.has(normRef)) {
          for (const c of refIndex.get(normRef)!) {
            const dist = haversineM(row.midLat, row.midLon, c.lat, c.lon)
            if (dist < bestDist) { bestDist = dist; best = c }
          }
        }

        if (best && bestDist < 15000) {
          return {
            light: best.aadt_light, medium: best.aadt_medium,
            heavy: best.aadt_heavy, moto: best.aadt_moto, sourceId: MY_SOURCE_ID,
          }
        }
        return null
      },
      () => { matched++ },
    )
    if (r.updated) hexesUpdated++

    if (hi % 10 === 0) {
      console.log(`  [${Math.round((Date.now() - startTime) / 1000)}s] ${hi + 1}/${hexDirs.length} hexes, ${hexesUpdated} updated, ${matched} matched`)
    }
  }

  console.log(`\n=== Enrichment Results ===`)
  console.log(`  Total segments: ${totalSeg}`)
  console.log(`  Preserved: ${preserved}`)
  console.log(`  Newly matched: ${matched} (${(100 * matched / Math.max(totalSeg, 1)).toFixed(1)}%)`)
  console.log(`  Hexes updated: ${hexesUpdated}`)

  const top = points.sort((a, b) => b.total - a.total).slice(0, 10)
  console.log(`\n  Top 10 AADF:`)
  for (const p of top) {
    const hv = Math.round(100 * p.aadt_heavy / Math.max(p.total, 1))
    console.log(`    ${p.ref.padEnd(10)} AADF=${p.total.toLocaleString().padStart(7)} HV=${hv}% (${p.lat.toFixed(2)}, ${p.lon.toFixed(2)}) [${p.year}]`)
  }
}

async function main() {
  console.log(`=== GB Road Traffic Enrichment (DfT AADF) ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache: ${CACHE_DIR}\n`)

  const points = await downloadDft()
  console.log(`  Total count points: ${points.length}`)

  await enrichArrows(points)
  console.log(`\n=== Done ===`)
}

main().catch(err => { console.error(err); process.exit(1) })
