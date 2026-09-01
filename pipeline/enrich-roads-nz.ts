/**
 * Enrich NZ roads.arrow with NZTA Carriageway + Auckland Transport AADT.
 *
 * Sources:
 *   1. NZTA Waka Kotahi GEO_MASTER_GIS_Carriageway (10,835 segments, state highways)
 *      services.arcgis.com/CXBb7LAjgIIdcsPt/ArcGIS/rest/services/GEO_MASTER_GIS_Carriageway/FeatureServer/0
 *      Per-segment trafficADTEst, loadingPcHeavy, lanes, ONRC
 *   2. Auckland Transport AADT (13,743 points with full vehicle class breakdown)
 *      data-atgis.opendata.arcgis.com — adt, pcheavy, pcbus, pccar, pclcv, pchcvi, pchcvii
 *
 * License: CC-BY 4.0 (NZGOAL Waka Kotahi + AT)
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-nz.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-nz.ts --enrich-only
 */

import { readFileSync, writeFileSync, existsSync, mkdirSync, statSync } from 'node:fs'
import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_NZ_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { haversineM } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes, osmRoadClassRank, ROAD_CLASS_RANK_TOLERANCE } from './lib/roads-arrow.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_NZ_NATIONAL_ROADS

// NZTA State Highways + Auckland Transport AADT cover the surveyed NZ road network (OSM motorway(0)/
// trunk(1)/primary(2)/secondary(3)/tertiary(4) + their _link variants), not residential(5)/service(7)/
// etc., so writeRoadAadt is gated to this set — a quiet street can't inherit a nearby highway's AADT.
const NZ_COVERAGE = new Set([0, 1, 2, 3, 4, 10, 11, 12])

// NZTA ONRC (One Network Road Classification) → rank on the OSM 0..4 scale.
// Measured split of the aadt>0 carriageway rows: High Volume 24.6% / Regional
// 22.4% / Primary Collector 21.2% / Arterial 14.7% / National 12.2% / Secondary
// Collector 3.5% / null 1.2% / Access 0.1%. "High Volume" is ONRC's TOP tier
// (short for National High Volume — median AADT 13,631 vs National's 7,365; the
// Auckland/Wellington/Christchurch motorway network), so it ranks 0 alongside
// National. Secondary Collector (median AADT 881) ranks 4 like the US's lowest
// kept tier. Access and null/unknown → drop at parse: a segment whose class we
// can't place must not stamp anything — unmatched beats confidently wrong.
function onrcRank(onrc: unknown): number | null {
  switch (String(onrc ?? '')) {
    case 'High Volume': case 'National': return 0
    case 'Regional': return 1
    case 'Arterial': return 2
    case 'Primary Collector': return 3
    case 'Secondary Collector': return 4
    default: return null // 'Access', null, anything unrecognized
  }
}

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/nz`)
const AT_AADT = resolve(CACHE_DIR, 'at-aadt.geojson')

const enrichOnly = process.argv.includes('--enrich-only')
const forceDownload = process.argv.includes('--force-download')

const NZTA_BASE = 'https://services.arcgis.com/CXBb7LAjgIIdcsPt/ArcGIS/rest/services/GEO_MASTER_GIS_Carriageway/FeatureServer/0'
const AT_URL = 'https://data-atgis.opendata.arcgis.com/datasets/a204ffd92f7546e898402e064bda6609_0.geojson'

// New Zealand bbox; [minLat,minLon,maxLat,maxLon]. iterateCountryHexes skips the
// rest of the planet so the loader doesn't read every roads.arrow on Earth.
const NZ_HEX_BBOX: [number, number, number, number] = [-47.5, 165, -34, 179]

interface NzRoadSegment {
  midLat: number
  midLon: number
  aadt: number
  heavyPct: number
  /** onrcRank for NZTA segments; null for AT Auckland points (no class field),
   *  which the match gate then takes on proximity alone. */
  rank: number | null
  aadt_light: number
  aadt_medium: number
  aadt_heavy: number
  aadt_moto: number
}

async function downloadNzta(): Promise<void> {
  mkdirSync(CACHE_DIR, { recursive: true })
  for (let p = 0; p < 6; p++) {
    const offset = p * 2000
    const path = resolve(CACHE_DIR, `nzta-page-${offset}.json`)
    if (!forceDownload && existsSync(path) && statSync(path).size > 1000) continue
    if (enrichOnly) throw new Error(`--enrich-only but nzta-page-${offset}.json not cached`)
    console.log(`  Downloading nzta-page-${offset}...`)
    const url = `${NZTA_BASE}/query?where=1%3D1&outFields=trafficADTEst,trafficADTCount,loadingPcHeavy,lanes,roadName,ONRC&outSR=4326&f=geojson&resultOffset=${offset}&resultRecordCount=2000`
    const res = await fetch(url, { signal: AbortSignal.timeout(120_000) })
    if (!res.ok) throw new Error(`HTTP ${res.status} for offset=${offset}`)
    writeFileSync(path, Buffer.from(await res.arrayBuffer()))
  }
}

async function downloadAt(): Promise<void> {
  if (!forceDownload && existsSync(AT_AADT)) return
  if (enrichOnly) throw new Error('--enrich-only but at-aadt.geojson not cached')
  mkdirSync(CACHE_DIR, { recursive: true })
  console.log(`  Downloading AT AADT...`)
  const res = await fetch(AT_URL, { signal: AbortSignal.timeout(120_000) })
  if (!res.ok) throw new Error(`HTTP ${res.status} for AT AADT`)
  writeFileSync(AT_AADT, Buffer.from(await res.arrayBuffer()))
}

function extractCentroid(geom: any): [number, number] | null {
  if (!geom || !geom.coordinates) return null
  if (geom.type === 'Point') return [geom.coordinates[1], geom.coordinates[0]]
  let sumLat = 0, sumLon = 0, n = 0
  if (geom.type === 'LineString') {
    for (const [lon, lat] of geom.coordinates) { sumLat += lat; sumLon += lon; n++ }
  } else if (geom.type === 'MultiLineString') {
    for (const line of geom.coordinates) {
      for (const [lon, lat] of line) { sumLat += lat; sumLon += lon; n++ }
    }
  } else return null
  if (n === 0) return null
  return [sumLat / n, sumLon / n]
}

function makeRecord(lat: number, lon: number, aadt: number, heavyPct: number, rank: number | null): NzRoadSegment {
  const aadt_moto = Math.round(aadt * 0.01)
  const totalHeavy = Math.round(aadt * heavyPct / 100)
  const aadt_medium = Math.round(totalHeavy * 0.20) // buses + light trucks
  const aadt_heavy = totalHeavy - aadt_medium
  const aadt_light = Math.max(0, aadt - totalHeavy - aadt_moto)
  return { midLat: lat, midLon: lon, aadt, heavyPct, rank, aadt_light, aadt_medium, aadt_heavy, aadt_moto }
}

function parseAll(): NzRoadSegment[] {
  const records: NzRoadSegment[] = []

  // 1. NZTA carriageway pages
  let onrcDropped = 0
  for (let p = 0; p < 6; p++) {
    const offset = p * 2000
    const path = resolve(CACHE_DIR, `nzta-page-${offset}.json`)
    if (!existsSync(path)) continue
    const data = JSON.parse(readFileSync(path, 'utf-8'))
    for (const feat of data.features || []) {
      const props = feat.properties || {}
      const aadt = parseInt(props.trafficADTEst || props.trafficADTCount || '0')
      if (aadt <= 0) continue
      const rank = onrcRank(props.ONRC)
      if (rank === null) { onrcDropped++; continue } // Access / null ONRC — see onrcRank
      const heavyPct = parseFloat(props.loadingPcHeavy || '8') // default 8% if missing
      const coords = extractCentroid(feat.geometry)
      if (!coords) continue
      records.push(makeRecord(coords[0], coords[1], aadt, heavyPct, rank))
    }
  }
  console.log(`  NZTA carriageway: ${records.length} segments (${onrcDropped} dropped: Access/unknown ONRC)`)

  // 2. AT Auckland AADT points
  const atBefore = records.length
  if (existsSync(AT_AADT)) {
    const data = JSON.parse(readFileSync(AT_AADT, 'utf-8'))
    for (const feat of data.features || []) {
      const props = feat.properties || {}
      const adt = parseInt(props.adt || '0')
      if (adt <= 0) continue
      const heavyPct = parseFloat(props.pcheavy || '0')
      const coords = extractCentroid(feat.geometry)
      if (!coords) continue
      // AT publishes no road-class field → rank null: the match gate skips the
      // class check for these records. They are dense urban point counters ON
      // the road they measure (≤200 m nearest-match), so the cross-class risk is
      // small, while dropping them would forfeit all Auckland coverage.
      records.push(makeRecord(coords[0], coords[1], adt, heavyPct, null))
    }
  }
  console.log(`  AT Auckland: ${records.length - atBefore} additional points`)

  return records
}

async function enrichArrows(sites: NzRoadSegment[]): Promise<void> {
  const grid = new Map<string, NzRoadSegment[]>()
  for (const s of sites) {
    const key = `${Math.floor(s.midLat * 100)}_${Math.floor(s.midLon * 100)}`
    if (!grid.has(key)) grid.set(key, [])
    grid.get(key)!.push(s)
  }
  console.log(`\n  Grid cells: ${grid.size}`)

  const hexDirs = iterateCountryHexes(H3R4_DIR, NZ_HEX_BBOX)
  console.log(`  NZ hexes with roads.arrow: ${hexDirs.length}\n`)

  let totalSeg = 0, matched = 0, hexesUpdated = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        // Fast-exit before the expensive grid scan when a higher-priority dataset
        // already owns the row (writeRoadAadt re-checks the gate — this only saves work).
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) return null

        // Nearest stored point/centroid within a 3×3 grid neighborhood, 200m cap.
        const gy = Math.floor(row.midLat * 100)
        const gx = Math.floor(row.midLon * 100)
        const rowRank = osmRoadClassRank(row.roadClass)
        let best: NzRoadSegment | null = null
        let bestDist = 200

        for (let dy = -1; dy <= 1; dy++) {
          for (let dx = -1; dx <= 1; dx++) {
            const cell = grid.get(`${gy + dy}_${gx + dx}`)
            if (!cell) continue
            for (const s of cell) {
              // NZTA segments must be class-compatible — a tertiary street must not
              // inherit SH1's AADT. AT Auckland records (rank null) match by proximity
              // alone, exactly as before the gate.
              if (s.rank !== null && Math.abs(s.rank - rowRank) > ROAD_CLASS_RANK_TOLERANCE) continue
              const d = haversineM(row.midLat, row.midLon, s.midLat, s.midLon)
              if (d < bestDist) { bestDist = d; best = s }
            }
          }
        }

        if (!best) return null
        return {
          light: best.aadt_light, medium: best.aadt_medium,
          heavy: best.aadt_heavy, moto: best.aadt_moto, sourceId: MY_SOURCE_ID,
        }
      },
      () => { matched++ },
      NZ_COVERAGE,
    )
    totalSeg += r.rows
    if (r.updated) hexesUpdated++

    if (hi % 50 === 0 || hi === hexDirs.length - 1) {
      const elapsed = ((Date.now() - startTime) / 1000).toFixed(0)
      console.log(`  [${elapsed}s] ${hi + 1}/${hexDirs.length} hexes, ${hexesUpdated} updated, ${matched.toLocaleString()} matched`)
    }
  }

  console.log(`\n=== Enrichment Results ===`)
  console.log(`  Total segments scanned: ${totalSeg.toLocaleString()}`)
  console.log(`  Newly matched: ${matched.toLocaleString()} (${(100 * matched / Math.max(totalSeg, 1)).toFixed(2)}%)`)
  console.log(`  Hexes updated: ${hexesUpdated}/${hexDirs.length}`)

  const top = [...sites].sort((a, b) => b.aadt - a.aadt).slice(0, 10)
  console.log(`\n  Top 10 AADT sites:`)
  for (const s of top) {
    console.log(`    AADT=${s.aadt.toLocaleString().padStart(7)} heavy=${s.heavyPct}% (${s.midLat.toFixed(3)}, ${s.midLon.toFixed(3)})`)
  }
}

async function main() {
  console.log(`=== NZ Road Traffic Enrichment — NZTA Carriageway + AT AADT ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache:    ${CACHE_DIR}\n`)

  if (!existsSync(H3R4_DIR)) throw new Error(`H3R4 directory not found: ${H3R4_DIR}`)

  await downloadNzta()
  await downloadAt()

  const records = parseAll()
  console.log(`  Total records: ${records.length}`)

  await enrichArrows(records)
  console.log(`\n=== Done ===`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
