/**
 * Enrich US roads.arrow with FHWA HPMS 2022 (Highway Performance Monitoring System).
 *
 * Source: ArcGIS REST FeatureServer
 *   services.arcgis.com/xOi1kZaI0eWDREZv/.../HPMS_FULL_US_2022_Sysnomulti_view/0
 *   235,257 polyline segments with AADT for all NHS + F_SYSTEM 1-5 (Interstate +
 *   Principal Arterial + Minor Arterial + Major Collector + Minor Collector)
 *
 * Pre-downloaded into hpms-page-N.json files via curl (paginated 2000/request × 119 pages).
 *
 * License: US federal work — public domain (17 USC §105)
 *
 * Vehicle class split derived from F_SYSTEM (CNOSSOS-EU defaults):
 *   F_SYSTEM 1 (Interstate)              → 12% heavy
 *   F_SYSTEM 2 (Principal Arterial Other Freeway) → 10% heavy
 *   F_SYSTEM 3 (Principal Arterial Other) → 8% heavy
 *   F_SYSTEM 4 (Minor Arterial)           → 6% heavy
 *   F_SYSTEM 5 (Major Collector)          → 5% heavy
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-us.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-us.ts --enrich-only
 */

import { readFileSync, writeFileSync, existsSync, mkdirSync, statSync } from 'node:fs'
import { SOURCES_BY_KEY } from './lib/sources.js'
import { shouldOverwrite } from './lib/provenance.js'
import { resolve } from 'node:path'
import { SOURCE_ID_US_FHWA_HPMS } from './lib/source-ids.generated.js'
import { haversineM } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes, osmRoadClassRank, ROAD_CLASS_RANK_TOLERANCE } from './lib/roads-arrow.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_US_FHWA_HPMS

// HPMS surveys F_SYSTEM 1-5 (Interstate → Major Collector) = OSM motorway(0)/
// trunk(1)/primary(2)/secondary(3)/tertiary(4) and their _link variants
// (10/11/12). It never surveys residential(5)/service(7)/etc., so writeRoadAadt
// is gated to this set — a quiet street can no longer inherit a nearby
// interstate's AADT (the Knoxville "Papermill Pointe Way" = 211,587 bug).
const HPMS_COVERAGE = new Set([0, 1, 2, 3, 4, 10, 11, 12])

// The coverage gate stops MINOR (residential/service) OSM roads matching HPMS,
// but an in-coverage secondary/tertiary street still inherits a nearby INTERSTATE
// segment's AADT just because it's within 200 m — the "Papermill Drive" = 184k
// cross-class proximity mismatch (~13% of US secondary/tertiary roads). So the
// match also requires the matched HPMS segment's functional class to be within
// ROAD_CLASS_RANK_TOLERANCE of the OSM road's osmRoadClassRank (lib/roads-arrow).
// F_SYSTEM 1 Interstate→rank 0, 2 Freeway→1, 3 Principal Arterial→2,
// 4 Minor Arterial→3, 5 Major Collector→4.
const FSYSTEM_RANK: Record<number, number> = { 1: 0, 2: 1, 3: 2, 4: 3, 5: 4 }

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/us`)

const enrichOnly = process.argv.includes('--enrich-only')
const forceDownload = process.argv.includes('--force-download')

const PAGE_SIZE = 2000
const PAGE_COUNT = 119
const HPMS_BASE = 'https://services.arcgis.com/xOi1kZaI0eWDREZv/ArcGIS/rest/services/HPMS_FULL_US_2022_Sysnomulti_view/FeatureServer/0'

// Contiguous US + Alaska + Hawaii bbox. [minLat,minLon,maxLat,maxLon] — also the
// hex-scan bbox so iterateCountryHexes skips the rest of the planet.
const US_HEX_BBOX: [number, number, number, number] = [17.5, -180.0, 71.5, -65.0]

interface UsRoadSegment {
  midLat: number
  midLon: number
  aadt: number
  fSystem: number
  aadt_light: number
  aadt_medium: number
  aadt_heavy: number
  aadt_moto: number
}

// Heavy share by F_SYSTEM (CNOSSOS-EU Part 2 Table 2.3 defaults applied to US
// road categories). Only F_SYSTEM 1-5 — parseAllPages drops 6/7 (see below).
const HEAVY_SHARE: Record<number, number> = {
  1: 0.12, // Interstate
  2: 0.10, // Principal Arterial Other Freeway
  3: 0.08, // Principal Arterial Other
  4: 0.06, // Minor Arterial
  5: 0.05, // Major Collector
}

async function downloadAllPages(): Promise<void> {
  mkdirSync(CACHE_DIR, { recursive: true })
  for (let p = 0; p < PAGE_COUNT; p++) {
    const offset = p * PAGE_SIZE
    const path = resolve(CACHE_DIR, `hpms-page-${offset}.json`)
    if (!forceDownload && existsSync(path) && statSync(path).size > 1000) continue
    // --enrich-only is the OFFLINE mode: trust any existing cached page and only
    // abort on a genuinely MISSING one. The last page is a valid but <1 KB empty
    // trailing page (235,257 records fill 118 of the 119 paginated requests), and
    // must not be re-downloaded (network) or mistaken for "not cached".
    if (enrichOnly) {
      if (existsSync(path)) continue
      throw new Error(`--enrich-only but hpms-page-${offset}.json missing`)
    }
    console.log(`  Downloading hpms-page-${offset}...`)
    const url = `${HPMS_BASE}/query?where=AADT%3E0&outFields=AADT,F_SYSTEM,THROUGH_LANES,NHS,FACILITY_TYPE&f=geojson&outSR=4326&resultOffset=${offset}&resultRecordCount=${PAGE_SIZE}&orderByFields=OBJECTID`
    const res = await fetch(url, { signal: AbortSignal.timeout(120_000) })
    if (!res.ok) throw new Error(`HTTP ${res.status} for offset=${offset}`)
    const buf = Buffer.from(await res.arrayBuffer())
    writeFileSync(path, buf)
  }
}

function extractCentroid(geom: any): [number, number] | null {
  if (!geom || !geom.coordinates) return null
  let sumLat = 0, sumLon = 0, n = 0
  if (geom.type === 'LineString') {
    for (const [lon, lat] of geom.coordinates) {
      sumLat += lat; sumLon += lon; n++
    }
  } else if (geom.type === 'MultiLineString') {
    for (const line of geom.coordinates) {
      for (const [lon, lat] of line) {
        sumLat += lat; sumLon += lon; n++
      }
    }
  } else return null
  if (n === 0) return null
  return [sumLat / n, sumLon / n]
}

function parseAllPages(): UsRoadSegment[] {
  const records: UsRoadSegment[] = []
  for (let p = 0; p < PAGE_COUNT; p++) {
    const offset = p * PAGE_SIZE
    const path = resolve(CACHE_DIR, `hpms-page-${offset}.json`)
    if (!existsSync(path)) continue
    const data = JSON.parse(readFileSync(path, 'utf-8'))
    for (const feat of data.features || []) {
      const props = feat.properties || {}
      const aadt = parseInt(props.AADT || '0')
      if (aadt <= 0) continue

      const coords = extractCentroid(feat.geometry)
      if (!coords) continue
      const [lat, lon] = coords
      if (lat < US_HEX_BBOX[0] || lat > US_HEX_BBOX[2] || lon < US_HEX_BBOX[1] || lon > US_HEX_BBOX[3]) continue

      const fSystem = parseInt(props.F_SYSTEM || '7')
      // Source coverage = HPMS F_SYSTEM 1-5 (Interstate → Major Collector). Drop
      // Minor Collector (6) / Local (7) and NaN: those dense minor segments are
      // the ones most prone to mis-matching a nearby major OSM road, and the
      // target class gate (HPMS_COVERAGE) already restricts matches to road_class
      // 0-4 + links — this keeps source ↔ target coverage aligned.
      if (!(fSystem >= 1 && fSystem <= 5)) continue
      const heavyShare = HEAVY_SHARE[fSystem] ?? 0.05

      // CNOSSOS classes
      const aadt_moto = Math.round(aadt * 0.01)
      const totalHeavy = Math.round(aadt * heavyShare)
      const aadt_medium = Math.round(totalHeavy * 0.20) // ~20% buses + light trucks
      const aadt_heavy = totalHeavy - aadt_medium
      const aadt_light = Math.max(0, aadt - totalHeavy - aadt_moto)

      records.push({
        midLat: lat,
        midLon: lon,
        aadt,
        fSystem,
        aadt_light,
        aadt_medium,
        aadt_heavy,
        aadt_moto,
      })
    }
  }
  return records
}

async function enrichArrows(sites: UsRoadSegment[]): Promise<void> {
  // Build spatial grid (1km cells)
  const grid = new Map<string, UsRoadSegment[]>()
  for (const s of sites) {
    const key = `${Math.floor(s.midLat * 100)}_${Math.floor(s.midLon * 100)}`
    if (!grid.has(key)) grid.set(key, [])
    grid.get(key)!.push(s)
  }
  console.log(`\n  Grid cells: ${grid.size}`)

  // Pre-filter US hexes. iterateCountryHexes skips the rest of the planet so the
  // loader doesn't read every roads.arrow on Earth.
  const hexDirs = iterateCountryHexes(H3R4_DIR, US_HEX_BBOX)
  console.log(`  US hexes with roads.arrow: ${hexDirs.length}\n`)

  let totalSeg = 0, matched = 0, preserved = 0, hexesUpdated = 0, skipped = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        totalSeg++

        // Priority gate: if a higher-priority dataset already owns this row, leave it
        // (writeRoadAadt re-checks the gate — this only saves the grid lookup).
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) {
          if (row.existingSourceId !== 0) preserved++
          return null
        }

        const midLat = row.midLat
        const midLon = row.midLon

        const gy = Math.floor(midLat * 100)
        const gx = Math.floor(midLon * 100)
        const rowRank = osmRoadClassRank(row.roadClass)
        let best: UsRoadSegment | null = null
        let bestDist = 200

        for (let dy = -1; dy <= 1; dy++) {
          for (let dx = -1; dx <= 1; dx++) {
            const cell = grid.get(`${gy + dy}_${gx + dx}`)
            if (!cell) continue
            for (const s of cell) {
              // Class-compatible HPMS segments only — a secondary must not match an interstate.
              if (Math.abs(FSYSTEM_RANK[s.fSystem] - rowRank) > ROAD_CLASS_RANK_TOLERANCE) continue
              const d = haversineM(midLat, midLon, s.midLat, s.midLon)
              if (d < bestDist) { bestDist = d; best = s }
            }
          }
        }

        if (!best) return null
        // Whole-row atomic write — payload + dataset_id together (in writeRoadAadt).
        return {
          light: best.aadt_light, medium: best.aadt_medium,
          heavy: best.aadt_heavy, moto: best.aadt_moto, sourceId: MY_SOURCE_ID,
        }
      },
      () => { matched++ },
      HPMS_COVERAGE,
    )
    if (r.updated) hexesUpdated++
    skipped += r.skipped

    if (hi % 200 === 0 || hi === hexDirs.length - 1) {
      const elapsed = ((Date.now() - startTime) / 1000).toFixed(0)
      console.log(`  [${elapsed}s] ${hi + 1}/${hexDirs.length} hexes, ${hexesUpdated} updated, ${matched.toLocaleString()} matched`)
    }
  }

  console.log(`\n=== Enrichment Results ===`)
  console.log(`  In-coverage rows scanned: ${totalSeg.toLocaleString()}`)
  console.log(`  Out-of-coverage rows skipped (class gate): ${skipped.toLocaleString()}`)
  console.log(`  Preserved (continental): ${preserved.toLocaleString()}`)
  console.log(`  Newly matched: ${matched.toLocaleString()} (${(100 * matched / Math.max(totalSeg, 1)).toFixed(2)}%)`)
  console.log(`  Hexes updated: ${hexesUpdated}/${hexDirs.length}`)

  const top = [...sites].sort((a, b) => b.aadt - a.aadt).slice(0, 15)
  console.log(`\n  Top 15 HPMS AADT segments:`)
  for (const s of top) {
    console.log(`    F${s.fSystem}  AADT=${s.aadt.toLocaleString().padStart(8)} (${s.midLat.toFixed(3)}, ${s.midLon.toFixed(3)})`)
  }
}

async function main() {
  console.log(`=== US Road Traffic Enrichment — FHWA HPMS 2022 ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache:    ${CACHE_DIR}\n`)

  if (!existsSync(H3R4_DIR)) throw new Error(`H3R4 directory not found: ${H3R4_DIR}`)

  await downloadAllPages()

  console.log(`  Parsing all HPMS pages...`)
  const records = parseAllPages()
  console.log(`  Parsed ${records.length.toLocaleString()} HPMS road segments`)

  await enrichArrows(records)
  console.log(`\n=== Done ===`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
