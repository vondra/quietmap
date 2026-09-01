/**
 * Enrich FI roads.arrow with Väylävirasto Liikennemäärät 2024 KVL data.
 *
 * Source: avoinapi.vaylapilvi.fi/vaylatiedot/wfs
 *   typeNames=tiestotiedot:liikennemaarat_2024
 *   18,479 per-segment KVL records on Finnish state highways (maantiet)
 *   Geometry: MultiLineString in WGS84 (3D with elevation)
 *
 * Pre-downloaded into liikennemaarat-page-N.json files via curl (paginated 1000/request).
 *
 * Fields:
 *   kvl              = total AADT
 *   kvl_raskas       = AADT heavy vehicles (trucks + buses)
 *   kvl_yhdistelma   = AADT articulated/HGV combos (subset of kvl_raskas)
 *   alkusijainti_tie = road number (1-32 highways)
 *
 * License: CC-BY 4.0 (Väylävirasto)
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-fi.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-fi.ts --enrich-only
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-fi.ts --force-download
 */

import { readFileSync, writeFileSync, existsSync, mkdirSync, statSync } from 'node:fs'
import { resolve } from 'node:path'
import { SOURCES_BY_KEY } from './lib/sources.js'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_FI_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { haversineM } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes, osmRoadClassRank, ROAD_CLASS_RANK_TOLERANCE } from './lib/roads-arrow.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_FI_NATIONAL_ROADS

// Väylävirasto liikennemäärät measures the Finnish STATE road network (valtatie/kantatie/seututie/
// yhdystie = OSM motorway(0)/trunk(1)/primary(2)/secondary(3)/tertiary(4) + their _link variants).
// It never surveys residential(5)/service(7)/etc., so writeRoadAadt is gated to this set — a quiet
// street can no longer inherit a nearby state road's AADT (the Helsinki "Heinämiehentie" = 103,418 bug).
const FI_COVERAGE = new Set([0, 1, 2, 3, 4, 10, 11, 12])

// Väylävirasto has no functional-class column — the Finnish road NUMBER is the
// class (national numbering: 1-39 valtatie + 40-99 kantatie main roads, 100-999
// seututie regional, ≥1000 yhdystie connector). Measured split of the 2024 feed:
// <100 = 12.6%, 100-999 = 12.7%, ≥1000 = 74.7% of 18,064 segments. Ranked on
// the OSM 0..4 scale so the ±ROAD_CLASS_RANK_TOLERANCE match gate blocks a
// yhdystie count from stamping a motorway and a valtatie count from bleeding
// onto a tertiary street across a junction.
// Kehä I (101) and Kehä II (102): Helsinki's motorway-grade ring roads carry
// seututie NUMBERS but valtatie traffic — Kehä I is Finland's busiest road
// (KVL 108,372). At rank 2 its count legally bled onto adjacent OSM
// secondaries at interchanges (Kaarelantie, verified 2026-07-03) → rank 0.
const fiRoadNumberRank = (tie: number): number =>
  (tie === 101 || tie === 102 || tie < 100 ? 0 : tie < 1000 ? 2 : 4)

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/fi`)

const enrichOnly = process.argv.includes('--enrich-only')
const forceDownload = process.argv.includes('--force-download')

const PAGE_SIZE = 1000
const PAGE_COUNT = 19 // 0..18 → 19000 (covers all 18,479 features)
const FI_BBOX: [number, number, number, number] = [59.7, 19.1, 70.1, 31.6]

const WFS_BASE = 'https://avoinapi.vaylapilvi.fi/vaylatiedot/wfs'

interface FiRoadSegment {
  internalId: number
  tie: number  // road number
  rank: number // fiRoadNumberRank(tie)
  midLat: number
  midLon: number
  kvl: number
  kvlRaskas: number
  kvlYhdistelma: number
  aadt_light: number
  aadt_medium: number
  aadt_heavy: number
  aadt_moto: number
}

async function downloadAllPages(): Promise<void> {
  mkdirSync(CACHE_DIR, { recursive: true })
  for (let p = 0; p < PAGE_COUNT; p++) {
    const offset = p * PAGE_SIZE
    const path = resolve(CACHE_DIR, `liikennemaarat-page-${offset}.json`)
    if (!forceDownload && existsSync(path)) {
      const size = statSync(path).size
      if (size > 1000) continue
    }
    if (enrichOnly) throw new Error(`--enrich-only but liikennemaarat-page-${offset}.json not cached`)
    console.log(`  Downloading page offset=${offset}...`)
    const url = `${WFS_BASE}?service=WFS&version=2.0.0&request=GetFeature&typeNames=tiestotiedot:liikennemaarat_2024&outputFormat=application/json&srsName=EPSG:4326&count=${PAGE_SIZE}&startIndex=${offset}&sortBy=internal_id`
    const res = await fetch(url, { signal: AbortSignal.timeout(120_000) })
    if (!res.ok) throw new Error(`HTTP ${res.status} for offset=${offset}`)
    const buf = Buffer.from(await res.arrayBuffer())
    writeFileSync(path, buf)
    console.log(`  Cached: ${(buf.length / 1e6).toFixed(2)} MB`)
  }
}

function extractCentroid(geom: any): [number, number] | null {
  if (!geom || !geom.coordinates) return null
  let sumLat = 0, sumLon = 0, n = 0
  if (geom.type === 'LineString') {
    for (const c of geom.coordinates) {
      sumLon += c[0]; sumLat += c[1]; n++
    }
  } else if (geom.type === 'MultiLineString') {
    for (const line of geom.coordinates) {
      for (const c of line) {
        sumLon += c[0]; sumLat += c[1]; n++
      }
    }
  } else return null
  if (n === 0) return null
  return [sumLat / n, sumLon / n]
}

function parseAllPages(): FiRoadSegment[] {
  const records: FiRoadSegment[] = []
  for (let p = 0; p < PAGE_COUNT; p++) {
    const offset = p * PAGE_SIZE
    const path = resolve(CACHE_DIR, `liikennemaarat-page-${offset}.json`)
    if (!existsSync(path)) continue
    const data = JSON.parse(readFileSync(path, 'utf-8'))
    for (const feat of data.features || []) {
      const props = feat.properties || {}
      const kvl = parseInt(props.kvl || '0')
      if (kvl <= 0) continue

      const coords = extractCentroid(feat.geometry)
      if (!coords) continue
      const [lat, lon] = coords
      if (lat < FI_BBOX[0] || lat > FI_BBOX[2] || lon < FI_BBOX[1] || lon > FI_BBOX[3]) continue

      const kvlRaskas = parseInt(props.kvl_raskas || '0')
      const kvlYhdistelma = parseInt(props.kvl_yhdistelma || '0')

      // CNOSSOS-EU (Dir 2015/996): cat1 light ≤3.5 t, cat2 medium, cat3 heavy.
      // The Digiroad source gives only kvl_raskas (ALL heavy vehicles ≥3.5 t) and
      // kvl_yhdistelma (articulated subset) — no 2-axle/bus column to populate cat2
      // reliably, so all heavy vehicles default to cat3 (Finland's HGV fleet is
      // long-haul-dominated; the cat2↔cat3 Lw gap is small). The previous mapping
      // forced rigid 3-axle HGVs into medium, under-stating their noise.
      // moto estimated at 1% of kvl (no Finnish motorcycle column).
      const aadt_moto = Math.round(kvl * 0.01)
      const aadt_heavy = kvlRaskas
      const aadt_medium = 0
      const aadt_light = Math.max(0, kvl - kvlRaskas - aadt_moto)

      const tie = parseInt(props.alkusijainti_tie || '0')
      records.push({
        internalId: parseInt(props.internal_id || '0'),
        tie,
        rank: fiRoadNumberRank(tie),
        midLat: lat,
        midLon: lon,
        kvl,
        kvlRaskas,
        kvlYhdistelma,
        aadt_light,
        aadt_medium,
        aadt_heavy,
        aadt_moto,
      })
    }
  }
  return records
}

// Finland bbox (+margin); [minLat,minLon,maxLat,maxLon]. iterateCountryHexes skips
// the rest of the planet so the loader doesn't read every roads.arrow on Earth.
const FI_HEX_BBOX: [number, number, number, number] = [FI_BBOX[0], FI_BBOX[1], FI_BBOX[2], FI_BBOX[3]]

async function enrichArrows(sites: FiRoadSegment[]): Promise<void> {
  // Build spatial grid (1km cells) for nearest-neighbour matching
  const grid = new Map<string, FiRoadSegment[]>()
  for (const s of sites) {
    const key = `${Math.floor(s.midLat * 100)}_${Math.floor(s.midLon * 100)}`
    if (!grid.has(key)) grid.set(key, [])
    grid.get(key)!.push(s)
  }
  console.log(`\n  Grid cells: ${grid.size}`)

  const hexDirs = iterateCountryHexes(H3R4_DIR, FI_HEX_BBOX)
  console.log(`  Finnish hexes with roads.arrow: ${hexDirs.length}\n`)

  let totalSeg = 0, matched = 0, hexesUpdated = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        // Fast-exit if a higher-priority dataset owns the row (writeRoadAadt
        // re-checks the gate — this only saves the nearest-neighbour scan).
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) return null

        const midLat = row.midLat
        const midLon = row.midLon

        // Nearest class-compatible segment within 200m
        const gy = Math.floor(midLat * 100)
        const gx = Math.floor(midLon * 100)
        const rowRank = osmRoadClassRank(row.roadClass)
        let best: FiRoadSegment | null = null
        let bestDist = 200

        for (let dy = -1; dy <= 1; dy++) {
          for (let dx = -1; dx <= 1; dx++) {
            const cell = grid.get(`${gy + dy}_${gx + dx}`)
            if (!cell) continue
            for (const s of cell) {
              // Class-compatible segments only — a tertiary street must not inherit a valtatie's KVL.
              if (Math.abs(s.rank - rowRank) > ROAD_CLASS_RANK_TOLERANCE) continue
              const d = haversineM(midLat, midLon, s.midLat, s.midLon)
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
      FI_COVERAGE,
    )
    totalSeg += r.rows
    if (r.updated) hexesUpdated++

    if (hi % 50 === 0 || hi === hexDirs.length - 1) {
      const elapsed = ((Date.now() - startTime) / 1000).toFixed(0)
      console.log(`  [${elapsed}s] ${hi + 1}/${hexDirs.length} hexes, ${hexesUpdated} updated, ${matched} matched`)
    }
  }

  console.log(`\n=== Enrichment Results ===`)
  console.log(`  Total segments scanned: ${totalSeg}`)
  console.log(`  Newly matched: ${matched} (${(100 * matched / Math.max(totalSeg, 1)).toFixed(2)}%)`)
  console.log(`  Hexes updated: ${hexesUpdated}/${hexDirs.length}`)

  const top = [...sites].sort((a, b) => b.kvl - a.kvl).slice(0, 15)
  console.log(`\n  Top 15 KVL segments:`)
  for (const s of top) {
    console.log(`    Tie ${s.tie.toString().padStart(3)}  KVL=${s.kvl.toLocaleString().padStart(7)} heavy=${s.kvlRaskas.toLocaleString().padStart(5)}  (${s.midLat.toFixed(3)}, ${s.midLon.toFixed(3)})`)
  }
}

async function main() {
  console.log(`=== FI Road Traffic Enrichment — Väylävirasto Liikennemäärät 2024 ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache:    ${CACHE_DIR}\n`)

  if (!existsSync(H3R4_DIR)) throw new Error(`H3R4 directory not found: ${H3R4_DIR}`)

  await downloadAllPages()

  const records = parseAllPages()
  console.log(`  Parsed ${records.length} traffic measurement segments`)

  await enrichArrows(records)
  console.log(`\n=== Done ===`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
