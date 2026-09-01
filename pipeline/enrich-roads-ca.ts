/**
 * Enrich CA roads.arrow with Quebec MTQ DJMA (Débit Journalier Moyen Annuel = AADT).
 *
 * Source: ws.mapserver.transports.gouv.qc.ca WFS
 *   typeName=ms:circulation_routier
 *   ~7,767 LineString features in WGS84 covering the Quebec state highway network
 *   Per-segment DJMA + DJME (summer) + DJMH (winter) + %cam (truck share)
 *   Multi-year history (annee_en_cours + annee2..annee10)
 *
 * Format: GeoJSON FeatureCollection, parsed from `annee_en_cours` text field
 *   Example: "2024 DJMA:600 / DJME:650 / DJMH:540 / %cam:8.6 / 30e h:70"
 *
 * License: CC-BY 4.0 (donneesquebec.ca)
 *
 * Coverage: Quebec only (Ontario/BC/Alberta have provincial-only data with
 * complex tabular-to-geometry joins, not yet implemented)
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-ca.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-ca.ts --enrich-only
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-ca.ts --force-download
 */

import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { SOURCES_BY_KEY } from './lib/sources.js'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_CA_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { haversineM } from './lib/spatial.js'
import {
  writeRoadAadt,
  iterateCountryHexes,
  osmRoadClassRank,
  ROAD_CLASS_RANK_TOLERANCE,
  type RoadRow,
} from './lib/roads-arrow.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_CA_NATIONAL_ROADS

/** MTQ surveys the numbered provincial network — majors + links only. */
const MTQ_COVERAGE: ReadonlySet<number> = new Set([0, 1, 2, 3, 4, 10, 11, 12])

/** Québec route number → the shared 0..4 class rank: autoroutes (1-99,
 *  400-999) → 0, routes nationales (100-199) → 1, régionales (200-299) → 2,
 *  collectrices (300-399) → 3. Numbers ≥1000 are RTSS inventory codes
 *  (ramps, municipal spurs) with no OSM ref counterpart — excluded upstream. */
export function qcRouteRank(route: number): number {
  if (route <= 99 || (route >= 400 && route <= 999)) return 0
  if (route <= 199) return 1
  if (route <= 299) return 2
  return 3
}

/** Normalize an OSM ref token set to a Québec route number. Québec refs come
 *  as "A-20", "R-138", "20", "138 Est", multi "20;132". Whole-token only —
 *  free text must never reduce to a route number (the same failure family as
 *  CZ's "Zelená 20" → I/20, which put 17.5k AADT on a parking aisle). */
export function normalizeQcRef(ref: string): number | null {
  for (const tok of ref.split(';')) {
    const r = tok.trim()
    const m = r.match(/^(?:A-|R-)?(\d{1,3})(?:\s+(?:Est|Ouest|Nord|Sud))?$/i)
    if (m) return parseInt(m[1], 10)
  }
  return null
}

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/ca`)
const CACHE_QC = resolve(CACHE_DIR, 'qc-djma.geojson')

const enrichOnly = process.argv.includes('--enrich-only')
const forceDownload = process.argv.includes('--force-download')

const QC_URL = 'https://ws.mapserver.transports.gouv.qc.ca/swtq?service=wfs&version=2.0.0&request=getfeature&typename=ms:circulation_routier&srsname=EPSG:4326&outputformat=geojson'

// Canada bbox (continental)
const CA_BBOX: [number, number, number, number] = [41.5, -141.0, 84.0, -52.0]

// Quebec-only scan box (+halo): the DJMA census covers Quebec's state highways,
// so iterateCountryHexes skips the rest of the planet (otherwise the loader reads
// every roads.arrow on Earth). [minLat, minLon, maxLat, maxLon]
const CA_HEX_BBOX: [number, number, number, number] = [44.5, -80, 63, -56]

interface QcSegment {
  midLat: number
  midLon: number
  /** MTQ route number from RTSS (first 5 digits of rtss_debut) + its rank on
   *  the shared 0..4 scale — the ref-match + class-gate pair that replaced the
   *  pure 200 m proximity matcher (44,466 loud stamps on residential/service
   *  streets, worldwide damage census 2026-07-10). */
  route: number
  rank: number
  djma: number
  truckPct: number
  aadt_light: number
  aadt_medium: number
  aadt_heavy: number
  aadt_moto: number
}

async function downloadQc(): Promise<void> {
  if (!forceDownload && existsSync(CACHE_QC)) return
  if (enrichOnly) throw new Error('--enrich-only but qc-djma.geojson not cached')
  mkdirSync(CACHE_DIR, { recursive: true })
  console.log(`  Downloading Quebec DJMA (~50 MB)...`)
  const res = await fetch(QC_URL, { signal: AbortSignal.timeout(300_000) })
  if (!res.ok) throw new Error(`HTTP ${res.status}`)
  const buf = Buffer.from(await res.arrayBuffer())
  writeFileSync(CACHE_QC, buf)
  console.log(`  Cached: ${(buf.length / 1e6).toFixed(2)} MB`)
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

function parseAnneeText(text: string): { djma: number; truckPct: number } | null {
  if (!text) return null
  // Format: "2024 DJMA:600 / DJME:650 / DJMH:540 / %cam:8.6 / 30e h:70"
  const djmaMatch = text.match(/DJMA\s*:\s*(\d+)/)
  const camMatch = text.match(/%cam\s*:\s*([\d.]+)/)
  if (!djmaMatch) return null
  const djma = parseInt(djmaMatch[1])
  if (djma <= 0) return null
  const truckPct = camMatch ? parseFloat(camMatch[1]) : 0
  return { djma, truckPct }
}

function parseQc(): QcSegment[] {
  const data = JSON.parse(readFileSync(CACHE_QC, 'utf-8'))
  const records: QcSegment[] = []
  for (const feat of data.features || []) {
    const props = feat.properties || {}

    // Try annee_en_cours first; fall back to annee2 if missing
    let parsed = parseAnneeText(props.annee_en_cours || '')
    if (!parsed) parsed = parseAnneeText(props.annee2 || '')
    if (!parsed) parsed = parseAnneeText(props.annee3 || '')
    if (!parsed) continue

    const coords = extractCentroid(feat.geometry)
    if (!coords) continue
    const [lat, lon] = coords

    // Route number from RTSS ("00138-92-300-000D" → 138). Codes ≥1000 are
    // inventory identifiers (ramps, municipal spurs) with no OSM ref
    // counterpart — exactly the sections whose proximity matches used to leak
    // highway counts onto side streets. Skipped (their links inherit via
    // class defaults / continuity instead).
    const rtss = String(props.rtss_debut || '')
    const route = /^\d{5}/.test(rtss) ? parseInt(rtss.slice(0, 5), 10) : NaN
    if (!Number.isFinite(route) || route <= 0 || route >= 1000) continue

    // CNOSSOS classes
    const aadt_moto = Math.round(parsed.djma * 0.01)
    const totalHeavy = Math.round(parsed.djma * (parsed.truckPct || 7) / 100)
    const aadt_medium = Math.round(totalHeavy * 0.20)
    const aadt_heavy = totalHeavy - aadt_medium
    const aadt_light = Math.max(0, parsed.djma - totalHeavy - aadt_moto)

    records.push({
      midLat: lat,
      midLon: lon,
      route,
      rank: qcRouteRank(route),
      djma: parsed.djma,
      truckPct: parsed.truckPct,
      aadt_light,
      aadt_medium,
      aadt_heavy,
      aadt_moto,
    })
  }
  return records
}

async function enrichArrows(sites: QcSegment[]): Promise<void> {
  // Sections grouped by ROUTE NUMBER — ref match is mandatory now; the
  // centroid distance only picks WHICH section of the matched route applies.
  const byRoute = new Map<number, QcSegment[]>()
  for (const s of sites) {
    if (!byRoute.has(s.route)) byRoute.set(s.route, [])
    byRoute.get(s.route)!.push(s)
  }
  console.log(`\n  Routes: ${byRoute.size}`)

  const hexDirs = iterateCountryHexes(H3R4_DIR, CA_HEX_BBOX)
  console.log(`  Quebec hexes with roads.arrow: ${hexDirs.length}\n`)

  let totalSeg = 0, matched = 0, preserved = 0, retracted = 0, hexesUpdated = 0
  const startTime = Date.now()

  // ONE claim rule for both the match path and the retract oracle (the CZ
  // pattern): ref → route sections → rank gate → nearest section centroid
  // within 25 km (sections are km-spaced along a route; the old 200 m
  // any-road proximity is what leaked highway counts onto side streets).
  const matchSection = (row: RoadRow): QcSegment | null => {
    if (!MTQ_COVERAGE.has(row.roadClass)) return null
    if (!row.ref) return null
    const route = normalizeQcRef(row.ref)
    if (route === null) return null
    const candidates = byRoute.get(route)
    if (!candidates) return null
    const rowRank = osmRoadClassRank(row.roadClass)
    let best: QcSegment | null = null
    let bestDist = 25_000
    for (const s of candidates) {
      if (Math.abs(rowRank - s.rank) > ROAD_CLASS_RANK_TOLERANCE) continue
      const d = haversineM(row.midLat, row.midLon, s.midLat, s.midLon)
      if (d < bestDist) { bestDist = d; best = s }
    }
    return best
  }

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hexId = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hexId, 'roads.arrow'),
      (row) => {
        // Priority gate: preserve existing if it has higher priority than self
        // (writeRoadAadt re-checks the gate — this only saves the route search).
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) {
          preserved++
          return null
        }
        const best = matchSection(row)
        if (!best) return null
        return {
          light: best.aadt_light, medium: best.aadt_medium,
          heavy: best.aadt_heavy, moto: best.aadt_moto, sourceId: MY_SOURCE_ID,
        }
      },
      () => { matched++ },
      MTQ_COVERAGE,
      // Self-heal: disown every owned row the current rules would not claim —
      // the 44,466 loud residential/service stamps of the proximity era.
      { sourceId: MY_SOURCE_ID, when: (row) => matchSection(row) === null },
    )
    totalSeg += r.rows
    retracted += r.retracted
    if (r.updated) hexesUpdated++

    if (hi % 25 === 0 || hi === hexDirs.length - 1) {
      const elapsed = ((Date.now() - startTime) / 1000).toFixed(0)
      console.log(`  [${elapsed}s] ${hi + 1}/${hexDirs.length} hexes, ${hexesUpdated} updated, ${matched.toLocaleString()} matched`)
    }
  }

  console.log(`\n=== Enrichment Results ===`)
  console.log(`  Total segments scanned: ${totalSeg.toLocaleString()}`)
  console.log(`  Preserved (continental): ${preserved.toLocaleString()}`)
  console.log(`  Newly matched: ${matched.toLocaleString()} (${(100 * matched / Math.max(totalSeg, 1)).toFixed(2)}%)`)
  console.log(`  Retracted (proximity-era heal): ${retracted.toLocaleString()}`)
  console.log(`  Hexes updated: ${hexesUpdated}/${hexDirs.length}`)

  const top = [...sites].sort((a, b) => b.djma - a.djma).slice(0, 15)
  console.log(`\n  Top 15 DJMA segments:`)
  for (const s of top) {
    console.log(`    DJMA=${s.djma.toLocaleString().padStart(6)} cam=${s.truckPct}% (${s.midLat.toFixed(3)}, ${s.midLon.toFixed(3)})`)
  }
}

async function main() {
  console.log(`=== CA Road Traffic Enrichment — Quebec MTQ DJMA ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache:    ${CACHE_DIR}\n`)

  if (!existsSync(H3R4_DIR)) throw new Error(`H3R4 directory not found: ${H3R4_DIR}`)

  await downloadQc()
  console.log(`  Parsing Quebec DJMA GeoJSON...`)
  const records = parseQc()
  console.log(`  Parsed ${records.length.toLocaleString()} DJMA segments`)

  await enrichArrows(records)
  console.log(`\n=== Done ===`)
}

// Import-safe: run only when invoked directly — importing the exported
// helpers (tests) must never trigger a download/enrichment run.
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch(err => { console.error('Error:', err); process.exit(1) })
}
