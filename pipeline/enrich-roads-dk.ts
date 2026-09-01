/**
 * Enrich DK roads.arrow with Vejdirektoratet Mastra traffic count data.
 *
 * Source: vmgeoserver.vd.dk/geosmastra/opendata/ows (WFS GeoJSON, EPSG:25832)
 *   typeNames=OPEN_DATA_NOEGLETAL_VIEW
 *   ~22,000 measurement records from 2023-2025 (paginated 3000/request)
 *   Per-station: AADT, HDT (weekday), LBIL_AADT (heavy trucks), GNS_HASTIGHED (mean speed)
 *   VEJBESTYRER (road administrator) tells the tier: '0' = Vejdirektoratet state
 *   net, 3-digit code = kommune — 4/5 of the stations are municipal.
 *
 * Pre-downloaded into mastra-page-N.json files via curl. Script aggregates to
 * one record per (VEJNR, KILOMETER) keeping the most recent year.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-dk.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-dk.ts --enrich-only
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-dk.ts --force-download
 */

import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { SOURCES_BY_KEY } from './lib/sources.js'
import { shouldOverwrite } from './lib/provenance.js'
import proj4 from 'proj4'
import { SOURCE_ID_DK_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { haversineM } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes, osmRoadClassRank, ROAD_CLASS_RANK_TOLERANCE } from './lib/roads-arrow.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_DK_NATIONAL_ROADS

// Mastra is NOT the state road network alone: VEJBESTYRER (road administrator)
// splits the motor-vehicle stations into '0' = Vejdirektoratet state net (20.5%,
// median AADT 6,608 — motorways + trunk routes) and 3-digit kommune codes (79.5%,
// median AADT 1,205 — collector-grade municipal counts). writeRoadAadt stays gated
// to OSM motorway(0)..tertiary(4) + _link variants so no count stamps a
// residential/service street, and dkVejbestyrerRank keeps each tier in its class
// neighbourhood — a kommune collector count must never stamp the E20.
const DK_COVERAGE = new Set([0, 1, 2, 3, 4, 10, 11, 12])

// VEJBESTYRER → rank on the OSM 0..4 scale. State '0' → 1 (trunk): the state net
// spans motorway..primary, exactly what ±ROAD_CLASS_RANK_TOLERANCE admits.
// Kommune codes → 4 (tertiary): ±1 admits secondary..tertiary only, mirroring how
// the US ranks its lowest kept tier (Major Collector → 4). The measured kommune
// profile backs it: median 1,205 / p99 16,607 / max 30,408 AADT — collector
// volumes; letting them stamp a primary would mostly UNDER-state real traffic.
const dkVejbestyrerRank = (vejbestyrer: string): number => (vejbestyrer === '0' ? 1 : 4)

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/dk`)

const enrichOnly = process.argv.includes('--enrich-only')
const forceDownload = process.argv.includes('--force-download')

// Vejdirektoratet Mastra WFS — paginated GeoJSON
const PAGE_SIZE = 3000
const PAGE_OFFSETS = [0, 3000, 6000, 9000, 12000, 15000, 18000, 21000]
const MASTRA_URL = (offset: number) =>
  `https://vmgeoserver.vd.dk/geosmastra/opendata/ows?service=WFS&version=2.0.0&request=GetFeature&typeNames=OPEN_DATA_NOEGLETAL_VIEW&outputFormat=json&count=${PAGE_SIZE}&startIndex=${offset}&sortBy=AAR&CQL_FILTER=AAR%3E%3D2023`

// EPSG:25832 ETRS89/UTM32N → WGS84
proj4.defs('EPSG:25832', '+proj=utm +zone=32 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs')

// Denmark bbox [minLat,minLon,maxLat,maxLon] — used both to clip parsed Mastra
// points (parseAllPages) and to scope the hex scan (iterateCountryHexes) so the
// loader skips every roads.arrow outside Denmark.
const DK_BBOX: [number, number, number, number] = [54.5, 8.0, 57.8, 13.0]
const DK_HEX_BBOX: [number, number, number, number] = DK_BBOX

interface MastraRecord {
  vejnr: number
  vejnavn: string
  vejbestyrer: string // '0' = state, 3-digit = kommune
  kilometer: number
  meter: number
  aar: number
  lat: number
  lon: number
  aadt: number
  lbilAadt: number
  speedLimit: number | null
  meanSpeed: number
}

interface SegmentAadt {
  vejnr: number
  ref: string  // normalized
  rank: number // dkVejbestyrerRank of the kept (latest-year) record
  midLat: number
  midLon: number
  aadt_total: number
  aadt_light: number
  aadt_medium: number
  aadt_heavy: number
  aadt_moto: number
}

async function downloadAllPages(): Promise<void> {
  mkdirSync(CACHE_DIR, { recursive: true })
  for (const offset of PAGE_OFFSETS) {
    const path = resolve(CACHE_DIR, `mastra-page-${offset}.json`)
    if (!forceDownload && existsSync(path)) {
      continue
    }
    if (enrichOnly) {
      throw new Error(`--enrich-only but mastra-page-${offset}.json not cached`)
    }
    console.log(`  Downloading mastra page offset=${offset}...`)
    const res = await fetch(MASTRA_URL(offset), { signal: AbortSignal.timeout(120_000) })
    if (!res.ok) throw new Error(`HTTP ${res.status} for offset=${offset}`)
    const buf = Buffer.from(await res.arrayBuffer())
    writeFileSync(path, buf)
    console.log(`  Cached: ${(buf.length / 1e6).toFixed(2)} MB`)
  }
}

function parseAllPages(): MastraRecord[] {
  const records: MastraRecord[] = []
  for (const offset of PAGE_OFFSETS) {
    const path = resolve(CACHE_DIR, `mastra-page-${offset}.json`)
    if (!existsSync(path)) continue
    const data = JSON.parse(readFileSync(path, 'utf-8'))
    for (const feat of data.features || []) {
      const p = feat.properties || {}
      const coords = feat.geometry?.coordinates
      if (!coords || coords.length < 2) continue
      const [x, y] = coords as [number, number]
      const [lon, lat] = proj4('EPSG:25832', 'WGS84', [x, y])
      if (lat < DK_BBOX[0] || lat > DK_BBOX[2] || lon < DK_BBOX[1] || lon > DK_BBOX[3]) continue

      const vejnr = parseInt(p.VEJNR || '0')
      const aadt = parseInt(p.AADT || '0')
      if (!vejnr || aadt <= 0) continue
      // Skip non-motor (cycles, peds)
      if (p.KOERETOEJSART && p.KOERETOEJSART !== 'MOTORKTJ') continue

      records.push({
        vejnr,
        vejnavn: (p.VEJNAVN || '').toString(),
        vejbestyrer: String(p.VEJBESTYRER ?? '').trim(),
        kilometer: parseInt(p.KILOMETER || '0'),
        meter: parseInt(p.METER || '0'),
        aar: parseInt(p.AAR || '0'),
        lat,
        lon,
        aadt,
        lbilAadt: parseInt(p.LBIL_AADT || '0'),
        speedLimit: p.HAST_GRAENSE != null ? parseInt(p.HAST_GRAENSE) : null,
        meanSpeed: parseFloat(p.GNS_HASTIGHED || '0'),
      })
    }
  }
  return records
}

function aggregateLatest(records: MastraRecord[]): SegmentAadt[] {
  // Group by (vejnr, kilometer) and keep most recent year
  const map = new Map<string, MastraRecord>()
  for (const r of records) {
    const key = `${r.vejnr}_${r.kilometer}`
    const existing = map.get(key)
    if (!existing || r.aar > existing.aar) {
      map.set(key, r)
    }
  }

  const out: SegmentAadt[] = []
  for (const r of map.values()) {
    // Vehicle classes:
    //   moto: 1% of AADT (CNOSSOS default)
    //   heavy: LBIL_AADT (lastbil)
    //   medium: 5% of heavy as buses (split heavy)
    //   light: AADT - heavy - moto
    const aadt_moto = Math.round(r.aadt * 0.01)
    const aadt_medium = Math.round(r.lbilAadt * 0.05)
    const aadt_heavy = Math.round(r.lbilAadt - aadt_medium)
    const aadt_light = Math.max(0, r.aadt - r.lbilAadt - aadt_moto)

    // Normalize ref: VEJNR may map to "1" → could be statsvej; we use vejnavn for fallback
    // OSM Danish refs: "E20", "E45", "M3", "1" (numbered statsveje)
    const ref = r.vejnr.toString()

    out.push({
      vejnr: r.vejnr,
      ref,
      rank: dkVejbestyrerRank(r.vejbestyrer),
      midLat: r.lat,
      midLon: r.lon,
      aadt_total: r.aadt,
      aadt_light,
      aadt_medium,
      aadt_heavy,
      aadt_moto,
    })
  }
  return out
}

async function enrichArrows(sites: SegmentAadt[]): Promise<void> {
  // Build spatial grid for proximity matching (1km cells)
  const grid = new Map<string, SegmentAadt[]>()
  for (const s of sites) {
    const key = `${Math.floor(s.midLat * 100)}_${Math.floor(s.midLon * 100)}`
    if (!grid.has(key)) grid.set(key, [])
    grid.get(key)!.push(s)
  }
  console.log(`\n  Grid cells: ${grid.size}`)

  const hexDirs = iterateCountryHexes(H3R4_DIR, DK_HEX_BBOX)
  console.log(`  Danish hexes with roads.arrow: ${hexDirs.length}\n`)

  let totalSeg = 0, matched = 0, preserved = 0, hexesUpdated = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        // Priority gate: preserve existing if it has higher priority than self.
        // writeRoadAadt re-checks the gate — this only saves the grid search.
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) {
          preserved++
          return null
        }

        // Find nearest class-compatible Mastra point within 200m (per-station data, no ref matching).
        const gy = Math.floor(row.midLat * 100)
        const gx = Math.floor(row.midLon * 100)
        const rowRank = osmRoadClassRank(row.roadClass)
        let best: SegmentAadt | null = null
        let bestDist = 200

        for (let dy = -1; dy <= 1; dy++) {
          for (let dx = -1; dx <= 1; dx++) {
            const cell = grid.get(`${gy + dy}_${gx + dx}`)
            if (!cell) continue
            for (const s of cell) {
              // Class-compatible stations only — a tertiary street must not inherit the E20's count.
              if (Math.abs(s.rank - rowRank) > ROAD_CLASS_RANK_TOLERANCE) continue
              const d = haversineM(row.midLat, row.midLon, s.midLat, s.midLon)
              if (d < bestDist) { bestDist = d; best = s }
            }
          }
        }

        if (!best) return null
        // Class split was computed once in aggregateLatest; copy it verbatim.
        return {
          light: best.aadt_light, medium: best.aadt_medium,
          heavy: best.aadt_heavy, moto: best.aadt_moto, sourceId: MY_SOURCE_ID,
        }
      },
      () => { matched++ },
      DK_COVERAGE,
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

  const top = [...sites].sort((a, b) => b.aadt_total - a.aadt_total).slice(0, 15)
  console.log(`\n  Top 15 AADT counter sites:`)
  for (const s of top) {
    console.log(`    VEJNR=${s.vejnr.toString().padStart(8)}  AADT=${s.aadt_total.toLocaleString().padStart(7)} heavy=${s.aadt_heavy.toLocaleString().padStart(5)}  (${s.midLat.toFixed(3)}, ${s.midLon.toFixed(3)})`)
  }
}

async function main() {
  console.log(`=== DK Road Traffic Enrichment — Vejdirektoratet Mastra (2023+) ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache:    ${CACHE_DIR}\n`)

  if (!existsSync(H3R4_DIR)) throw new Error(`H3R4 directory not found: ${H3R4_DIR}`)

  await downloadAllPages()

  const records = parseAllPages()
  console.log(`  Parsed ${records.length} measurement records (motor vehicles only)`)

  const aggregated = aggregateLatest(records)
  console.log(`  Aggregated to ${aggregated.length} unique stations (latest year per location)`)

  await enrichArrows(aggregated)
  console.log(`\n=== Done ===`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
