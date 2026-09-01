/**
 * Enrich NO roads.arrow with NVDB Trafikkmengde (Statens vegvesen).
 *
 * Source: nvdbapiles.atlas.vegvesen.no/vegobjekter/540 (Trafikkmengde)
 *   ~47,658 per-segment ÅDT records covering all Norwegian state/county roads
 *   Per-segment: ÅDT (4623), Andel lange % (4624 — heavy share)
 *   Cursor pagination via metadata.neste.href
 *   Requires User-Agent + X-Client headers
 *
 * Geometry: WKT LINESTRING Z (lat lon elev, ...) — note lat-first order
 *
 * License: NLOD 2.0 (Norsk lisens for offentlige data)
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-no.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-no.ts --enrich-only
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-no.ts --force-download
 */

import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_NO_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { haversineM } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes, osmRoadClassRank, ROAD_CLASS_RANK_TOLERANCE } from './lib/roads-arrow.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_NO_NATIONAL_ROADS

// NVDB Trafikkmengde is NOT a state+county-only feed: the vegref prefix splits the
// 47,438 downloaded records into EV 5,096 / RV 2,080 / FV 16,373 / KV 23,173 (49%!)
// / PV 658, plus 42 SV skogsbilveg and 12 under-construction EA/RA/FA — half of it
// sits on MUNICIPAL (KV) and private (PV) roads. nvdbVegrefRank keeps only EV/RV/FV
// and drops the rest at parse, so a municipal side-street count can never stamp the
// state road next to it. The kept records target OSM motorway(0)..tertiary(4) +
// _link variants, never residential(5)/service(7)/etc., so writeRoadAadt is gated
// to this set — a quiet street can no longer inherit a nearby road's AADT (the
// Oslo "Teisenveien" = 55,000 bug).
const NO_COVERAGE = new Set([0, 1, 2, 3, 4, 10, 11, 12])

// Vegkategori (vegref prefix) → rank on the OSM 0..4 scale. EV europaveg / RV
// riksveg → 1 (trunk): many Norwegian E-roads and riksveger are 2-lane rural
// roads tagged trunk/primary in OSM, so trunk-rank ±ROAD_CLASS_RANK_TOLERANCE
// admits motorway..primary without letting an E6 count reach a secondary street.
// FV fylkesveg → 2 (primary): ±1 admits trunk..secondary. null = drop (KV
// kommunal / PV privat / SV skogsbil sit below the classes we stamp).
function nvdbVegrefRank(vegref: string): number | null {
  const prefix = vegref.slice(0, 2).toUpperCase()
  if (prefix === 'EV' || prefix === 'RV') return 1
  if (prefix === 'FV') return 2
  return null
}

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/no`)
const CACHE_PARSED = resolve(CACHE_DIR, 'nvdb-trafikkmengde.json')

const enrichOnly = process.argv.includes('--enrich-only')
const forceDownload = process.argv.includes('--force-download')

const NVDB_BASE = 'https://nvdbapiles.atlas.vegvesen.no/vegobjekter/540'
const PAGE_SIZE = 1000

// Generous box around Norway; [minLat,minLon,maxLat,maxLon]. Used both to drop
// out-of-country NVDB centroids at parse time AND (via iterateCountryHexes) to
// skip the rest of the planet's roads.arrow files during the hex scan.
const NO_HEX_BBOX: [number, number, number, number] = [57.9, 4.5, 71.2, 31.2]

interface NvdbRecord {
  id: number
  vegref: string
  rank: number // nvdbVegrefRank(vegref) — EV/RV 1, FV 2; KV/PV/other never parsed
  midLat: number
  midLon: number
  aadt: number
  heavyPct: number
  year: number
  aadt_light: number
  aadt_medium: number
  aadt_heavy: number
  aadt_moto: number
}

async function downloadAll(): Promise<NvdbRecord[]> {
  if (!forceDownload && existsSync(CACHE_PARSED)) {
    console.log(`  Using cached parsed: ${CACHE_PARSED}`)
    // Re-derive rank from the cached vegref: caches written before the class gate
    // landed carry no rank field and still contain the KV/PV records — normalizing
    // here keeps them usable without a re-download (idempotent on new caches too).
    const cached: NvdbRecord[] = JSON.parse(readFileSync(CACHE_PARSED, 'utf-8'))
    const kept: NvdbRecord[] = []
    for (const r of cached) {
      const rank = nvdbVegrefRank(r.vegref || '')
      if (rank === null) continue
      kept.push({ ...r, rank })
    }
    console.log(`  ${kept.length}/${cached.length} records after the EV/RV/FV class filter`)
    return kept
  }
  if (enrichOnly) throw new Error('--enrich-only but NVDB cache missing')

  mkdirSync(CACHE_DIR, { recursive: true })
  console.log(`  Downloading NVDB Trafikkmengde via cursor pagination...`)

  const records: NvdbRecord[] = []
  let url: string | null = `${NVDB_BASE}?antall=${PAGE_SIZE}&srid=4326&inkluder=egenskaper,geometri,lokasjon`
  let pageNum = 0

  while (url) {
    pageNum++
    const res: Response = await fetch(url, {
      signal: AbortSignal.timeout(120_000),
      headers: {
        'User-Agent': 'quiet-map-noise/1.0',
        'X-Client': 'quiet-map-noise',
        'Accept': 'application/vnd.vegvesen.nvdb-v3-rev1+json',
      },
    })
    if (!res.ok) {
      console.log(`  HTTP ${res.status} on page ${pageNum} — stopping`)
      break
    }
    const data: any = await res.json()
    const objs = data.objekter || []

    for (const obj of objs) {
      const r = parseObject(obj)
      if (r) records.push(r)
    }

    const next = data.metadata?.neste?.href
    console.log(`  page ${pageNum}: ${objs.length} objects (${records.length} valid total)`)
    // NVDB caps at 800/page even when antall=1000 — rely on next cursor to stop
    if (!next || objs.length === 0) break
    url = next
  }

  console.log(`  Downloaded ${records.length} valid Trafikkmengde records`)
  writeFileSync(CACHE_PARSED, JSON.stringify(records))
  console.log(`  Cached parsed to ${CACHE_PARSED}`)
  return records
}

function parseObject(obj: any): NvdbRecord | null {
  const id = obj.id
  if (!id) return null

  // Find ÅDT (4623), heavy % (4624), year (4621) in egenskaper
  let aadt = 0, heavyPct = 0, year = 0
  for (const e of obj.egenskaper || []) {
    if (e.id === 4623) aadt = parseInt(e.verdi || '0')
    else if (e.id === 4624) heavyPct = parseInt(e.verdi || '0')
    else if (e.id === 4621) year = parseInt(e.verdi || '0')
  }
  if (aadt <= 0) return null

  // Geometry: LINESTRING Z (lat lon z, ...)
  const wkt = obj.geometri?.wkt
  if (!wkt) return null
  const lineMatch = wkt.match(/LINESTRING(?:\s*Z)?\s*\(([^)]+)\)/)
  if (!lineMatch) return null
  const coordsStr = lineMatch[1]
  const points = coordsStr.split(',').map((s: string) => s.trim().split(/\s+/).map(Number))
  if (points.length === 0) return null

  let sumLat = 0, sumLon = 0, n = 0
  for (const p of points) {
    if (p.length < 2) continue
    sumLat += p[0]
    sumLon += p[1]
    n++
  }
  if (n === 0) return null
  const midLat = sumLat / n
  const midLon = sumLon / n

  if (midLat < NO_HEX_BBOX[0] || midLat > NO_HEX_BBOX[2] || midLon < NO_HEX_BBOX[1] || midLon > NO_HEX_BBOX[3]) return null

  const vegref = obj.lokasjon?.vegsystemreferanser?.[0]?.kortform || ''
  const rank = nvdbVegrefRank(vegref)
  if (rank === null) return null // KV/PV/SV/anlegg-phase — see nvdbVegrefRank

  // CNOSSOS vehicle classes:
  //   moto = 1% of aadt (no NO moto column)
  //   heavy = aadt * heavyPct/100 (lange kjøretøy includes trucks + buses + articulated)
  //   medium = ~25% of heavy (buses)
  //   light = aadt - heavy - moto
  const aadt_moto = Math.round(aadt * 0.01)
  const totalHeavy = Math.round(aadt * heavyPct / 100)
  const aadt_medium = Math.round(totalHeavy * 0.25)
  const aadt_heavy = totalHeavy - aadt_medium
  const aadt_light = Math.max(0, aadt - totalHeavy - aadt_moto)

  return {
    id,
    vegref,
    rank,
    midLat,
    midLon,
    aadt,
    heavyPct,
    year,
    aadt_light,
    aadt_medium,
    aadt_heavy,
    aadt_moto,
  }
}

async function enrichArrows(sites: NvdbRecord[]): Promise<void> {
  // Build spatial grid (1km cells)
  const grid = new Map<string, NvdbRecord[]>()
  for (const s of sites) {
    const key = `${Math.floor(s.midLat * 100)}_${Math.floor(s.midLon * 100)}`
    if (!grid.has(key)) grid.set(key, [])
    grid.get(key)!.push(s)
  }
  console.log(`\n  Grid cells: ${grid.size}`)

  const hexDirs = iterateCountryHexes(H3R4_DIR, NO_HEX_BBOX)
  console.log(`  Norwegian hexes with roads.arrow: ${hexDirs.length}\n`)

  let totalSeg = 0, matched = 0, preserved = 0, hexesUpdated = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        // Fast-exit if a higher-priority dataset owns the row (writeRoadAadt
        // re-checks the gate — this only saves the nearest-centroid search).
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) {
          preserved++
          return null
        }

        // Nearest class-compatible NVDB centroid within 200 m, via the 1km spatial grid (3×3 cells).
        const gy = Math.floor(row.midLat * 100)
        const gx = Math.floor(row.midLon * 100)
        const rowRank = osmRoadClassRank(row.roadClass)
        let best: NvdbRecord | null = null
        let bestDist = 200

        for (let dy = -1; dy <= 1; dy++) {
          for (let dx = -1; dx <= 1; dx++) {
            const cell = grid.get(`${gy + dy}_${gx + dx}`)
            if (!cell) continue
            for (const s of cell) {
              // Class-compatible records only — a secondary street must not inherit an E6 ÅDT.
              if (Math.abs(s.rank - rowRank) > ROAD_CLASS_RANK_TOLERANCE) continue
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
      NO_COVERAGE,
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
  console.log(`  Preserved (continental): ${preserved}`)
  console.log(`  Newly matched: ${matched} (${(100 * matched / Math.max(totalSeg, 1)).toFixed(2)}%)`)
  console.log(`  Hexes updated: ${hexesUpdated}/${hexDirs.length}`)

  const top = [...sites].sort((a, b) => b.aadt - a.aadt).slice(0, 15)
  console.log(`\n  Top 15 ÅDT segments:`)
  for (const s of top) {
    console.log(`    ${s.vegref.padEnd(20)} ÅDT=${s.aadt.toLocaleString().padStart(7)} heavy=${s.heavyPct}% (${s.midLat.toFixed(3)}, ${s.midLon.toFixed(3)})`)
  }
}

async function main() {
  console.log(`=== NO Road Traffic Enrichment — NVDB Trafikkmengde ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache:    ${CACHE_DIR}\n`)

  if (!existsSync(H3R4_DIR)) throw new Error(`H3R4 directory not found: ${H3R4_DIR}`)

  const records = await downloadAll()
  console.log(`  Total records: ${records.length}`)

  await enrichArrows(records)
  console.log(`\n=== Done ===`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
