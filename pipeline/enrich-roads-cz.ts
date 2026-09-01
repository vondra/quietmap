/**
 * Enrich CZ roads.arrow with ŘSD traffic census data (AADT per vehicle class).
 *
 * Downloads from ŘSD ArcGIS REST API, caches locally, matches to OSM roads
 * by ref tag + proximity, adds aadt_* columns + source_id to Arrow.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-cz.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-cz.ts --force-download
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-cz.ts --enrich-only
 */

import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { SOURCE_ID_CZ_RSD_SCITANI } from './lib/source-ids.generated.js'
import { pointToPolylineDist } from './lib/spatial.js'
import { shouldOverwrite } from './lib/provenance.js'
import {
  writeRoadAadt,
  iterateCountryHexes,
  osmRoadClassRank,
  ROAD_CLASS_RANK_TOLERANCE,
  type RoadRow,
} from './lib/roads-arrow.js'
import { pathToFileURL } from 'node:url'
import { makeCoastalCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_CZ_RSD_SCITANI

/** ŘSD surveys numbered roads only: majors 0-4 + their link classes. */
const RSD_COVERAGE: ReadonlySet<number> = new Set([0, 1, 2, 3, 4, 10, 11, 12])

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/cz`)
const CACHE_FILE = resolve(CACHE_DIR, 'rsd-scitani.json')

const forceDownload = process.argv.includes('--force-download')
const enrichOnly = process.argv.includes('--enrich-only')

// ŘSD ArcGIS REST — ScitaniDopravy MapServer layer 3
// Must use spatial bbox in S-JTSK (EPSG:5514), where=1=1 returns 400.
const RSD_BASE = 'https://geoportal.rsd.cz/arcgis/rest/services/ScitaniDopravy/MapServer/3/query'
const CZ_BBOX_SJTSK = { xmin: -900000, ymin: -1300000, xmax: -400000, ymax: -900000 }
const PAGE_SIZE = 2000

// ── Types ──

// ŘSD class → CNOSSOS-EU category (Annex II, Dir (EU) 2015/996, amended Reg. 2021/1226):
//   cat1 light  = cars + light commercial ≤3.5 t  → O + LN
//   cat2 medium = 2-axle medium trucks + buses    → SN + A (+ TR/TRP tractors, closest bucket)
//   cat3 heavy  = 3+ axle / artic / semitrailer / artic bus → TN + TNP + SNP + NSN + AK
//   cat4 PTW    = M
interface CensusSection {
  ref: string           // normalized road ref
  /** ŘSD road-category rank on the shared 0..4 scale (D→0, I→1, II→3, III→4)
   *  — gated against `osmRoadClassRank` so a count can never land on a road
   *  class it does not belong to, whatever its ref looks like. */
  rank: number
  aadt_light: number
  aadt_medium: number
  aadt_heavy: number
  aadt_moto: number
  coords: [number, number][]   // section polyline as [lon, lat] vertices (matched per-segment, not by centroid)
}

// ── Step 1: Download ŘSD census ──

async function downloadCensus(): Promise<any[]> {
  if (!forceDownload && existsSync(CACHE_FILE) && !enrichOnly) {
    console.log(`  Using cached data: ${CACHE_FILE}`)
    return JSON.parse(readFileSync(CACHE_FILE, 'utf-8'))
  }
  if (enrichOnly) {
    if (!existsSync(CACHE_FILE)) {
      console.error('ERROR: --enrich-only but no cached data found')
      process.exit(1)
    }
    return JSON.parse(readFileSync(CACHE_FILE, 'utf-8'))
  }

  console.log('  Downloading from ŘSD ArcGIS...')
  const geomParam = encodeURIComponent(JSON.stringify(CZ_BBOX_SJTSK))
  const allFeatures: any[] = []
  let offset = 0

  while (true) {
    const url = `${RSD_BASE}?geometry=${geomParam}&geometryType=esriGeometryEnvelope` +
      `&inSR=5514&outFields=*&resultRecordCount=${PAGE_SIZE}&resultOffset=${offset}` +
      `&f=json&outSR=4326`

    const res = await fetch(url, { signal: AbortSignal.timeout(60000) })
    if (!res.ok) throw new Error(`ŘSD API error: ${res.status}`)
    const data = await res.json()

    if (!data.features || data.features.length === 0) break
    allFeatures.push(...data.features)
    console.log(`    fetched ${allFeatures.length} sections (offset=${offset})`)

    if (data.features.length < PAGE_SIZE) break
    offset += PAGE_SIZE
  }

  mkdirSync(CACHE_DIR, { recursive: true })
  writeFileSync(CACHE_FILE, JSON.stringify(allFeatures))
  console.log(`  Cached ${allFeatures.length} sections to ${CACHE_FILE}`)
  return allFeatures
}

// ── Step 2: Parse + normalize ──

/** Un-surveyed (all-zero) census sections skipped by parseCensus — reported once. */
let zeroSections = 0

function parseCensus(features: any[]): Map<string, CensusSection[]> {
  const byRef = new Map<string, CensusSection[]>()

  for (const f of features) {
    const a = f.attributes
    if (!a) continue

    const ref = normalizeRsdRef(String(a.PSILNICE || ''), String(a.PKOD_R || ''))
    if (!ref) continue

    const pts = (f.geometry?.paths || []).flat()
    if (!pts.length) continue

    const section: CensusSection = {
      ref,
      rank: rsdRank(String(a.PSILNICE || ''), String(a.PKOD_R || '')),
      aadt_light: (a.O || 0) + (a.LN || 0),
      aadt_medium: (a.SN || 0) + (a.A || 0) + (a.TR || 0) + (a.TRP || 0),
      aadt_heavy: (a.TN || 0) + (a.TNP || 0) + (a.SNP || 0) + (a.NSN || 0) + (a.AK || 0),
      aadt_moto: a.M || 0,
      coords: pts as [number, number][],
    }

    // GPR publishes un-surveyed III-class sections with all-zero counts.
    // Writing those zeros + the source stamp silently mutes real roads that
    // would otherwise get class defaults (2026-06 audit R7: 44.6k segments).
    if (section.aadt_light + section.aadt_medium + section.aadt_heavy + section.aadt_moto === 0) {
      zeroSections++
      continue
    }

    if (!byRef.has(ref)) byRef.set(ref, [])
    byRef.get(ref)!.push(section)
  }

  return byRef
}

// ── Step 3: Enrich Arrow files ──

// Generous box around Czechia (+~0.3 deg halo) so the hex scan skips the rest of
// the planet — ŘSD refs only match CZ hexes (otherwise the loader reads every
// roads.arrow on Earth, ~40 min vs the ~112 CZ R4 cells). [minLat,minLon,maxLat,maxLon]
const CZ_HEX_BBOX: [number, number, number, number] = [48.2, 11.7, 51.4, 19.2]

async function enrichHexes(censusByRef: Map<string, CensusSection[]>): Promise<void> {
  // Czech road numbers repeat across the border (Slovak I/49 continues CZ I/49)
  // and the 10 km proximity cap doesn't stop a match just across the line — the
  // 2026-06 audit R9 found ~1k Slovak/Polish segments carrying ŘSD AADT. Same
  // gate pattern as enrich-roads-pl.ts; created here because makeCoastalCountryGate
  // may download+convert the CGAZ boundary file on first use.
  const inCzechia = makeCoastalCountryGate('CZ')
  const hexDirs = iterateCountryHexes(H3R4_DIR, CZ_HEX_BBOX)

  let totalRoads = 0
  let totalMatched = 0
  let totalRetracted = 0
  let hexesUpdated = 0
  const matchByClass = new Map<number, { matched: number; total: number }>()

  // ONE claim rule, used from both sides: the match callback stamps rows it
  // returns a section for, and the retract disowns owned rows it returns null
  // for — so "what the current rules claim" cannot drift between the two
  // (/gg Codex: an earlier class-only retract left 2,724 rank-mismatched
  // stamps from the free-text era grandfathered in).
  const matchSection = (row: RoadRow): CensusSection | null => {
    // Coverage, EXPLICITLY (not just via the writer's gate): matchSection is
    // also the retract oracle, so an out-of-coverage row must read as
    // "no claim" here — never silently spared by the writer skipping it
    // (/gg Gemini). Rank 6 for locals would reject them anyway; this makes
    // the invariant independent of that arithmetic.
    if (!RSD_COVERAGE.has(row.roadClass)) return null
    // Ref match is MANDATORY — no proximity-only fallback.
    if (!row.ref) return null
    if (!inCzechia(row.midLat, row.midLon)) return null
    const normalized = normalizeOsmRef(row.ref)
    if (!normalized) return null
    const candidates = censusByRef.get(normalized)
    if (!candidates || candidates.length === 0) return null

    // Rank gate FIRST, then nearest: filtering after picking the closest
    // would let a rank-incompatible section (e.g. the expressway leg of a
    // renumbered corridor) SHADOW a compatible one a few metres farther
    // (/gg Gemini). Distance is point-to-polyline, so the section boundary
    // lands at the real junction, not a centroid bisector.
    const rowRank = osmRoadClassRank(row.roadClass)
    let best: CensusSection | null = null
    let bestDist = 10_000 // avoid wrong matches on same-numbered roads far away
    for (const candidate of candidates) {
      if (Math.abs(rowRank - candidate.rank) > ROAD_CLASS_RANK_TOLERANCE) continue
      const d = pointToPolylineDist(row.midLat, row.midLon, candidate.coords)
      if (d < bestDist) { best = candidate; bestDist = d }
    }
    return best
  }

  for (const hexId of hexDirs) {
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hexId, 'roads.arrow'),
      (row) => {
        let cls = matchByClass.get(row.roadClass)
        if (!cls) { cls = { matched: 0, total: 0 }; matchByClass.set(row.roadClass, cls) }
        cls.total++

        // Fast-exit before the expensive ref-match when a higher-priority dataset
        // already owns the row (writeRoadAadt re-checks the gate — this only saves work).
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) return null

        const best = matchSection(row)
        if (!best) return null

        // Do NOT halve for oneway — ŘSD = bidirectional total; the engine applies
        // oneway_factor=0.5. (The priority gate + atomic write live in writeRoadAadt.)
        return {
          light: best.aadt_light, medium: best.aadt_medium,
          heavy: best.aadt_heavy, moto: best.aadt_moto, sourceId: MY_SOURCE_ID,
        }
      },
      (row) => { matchByClass.get(row.roadClass)!.matched++ },
      // ŘSD surveys numbered roads only — majors + their links. The writer's
      // coverage gate keeps locals (5-9) out of `match` entirely (the same
      // set also lives inside matchSection for the retract oracle).
      RSD_COVERAGE,
      // Self-heal: disown every owned row the CURRENT rules would not claim —
      // the exact negation of the match path above (shared matchSection), so
      // free-text refs, local classes AND rank-mismatched relics all heal.
      // Freed rows fall to service-tree / continuity / engine defaults.
      {
        sourceId: MY_SOURCE_ID,
        when: (row) => matchSection(row) === null,
      },
    )
    totalRoads += r.rows
    totalMatched += r.matched
    totalRetracted += r.retracted
    if (r.updated) hexesUpdated++
  }

  console.log(`\n=== Results ===`)
  console.log(`  ${totalMatched} / ${totalRoads} segments matched (${(totalMatched / totalRoads * 100).toFixed(1)}%)`)
  console.log(`  ${totalRetracted} previously-stamped rows retracted (strict-ref / class heal)`)
  console.log(`  ${hexesUpdated} / ${hexDirs.length} hexes updated`)
  console.log(`\n  Per road class:`)
  for (const [cls, stats] of [...matchByClass.entries()].sort((a, b) => a[0] - b[0])) {
    const names = ['motorway', 'trunk', 'primary', 'secondary', 'tertiary', 'residential', 'living_st']
    const pct = stats.total > 0 ? (stats.matched / stats.total * 100).toFixed(1) : '0.0'
    console.log(`    ${(names[cls] || `class_${cls}`).padEnd(12)} ${stats.matched} / ${stats.total} (${pct}%)`)
  }
}

// ── Helpers ──

/** Normalize ŘSD road ref: PSILNICE + PKOD_R → canonical form */
function normalizeRsdRef(psilnice: string, pkodR: string): string {
  const p = psilnice.trim()
  if (!p) return ''
  if (/^D\d+/.test(p)) return p          // D1, D5 → keep as-is
  const num = p.replace(/\D/g, '')
  if (!num) return p
  // PKOD_R: 1=motorway, 5=expressway → prefix D
  if (pkodR === '1' || pkodR === '5') return `D${num}`
  return num
}

/** ŘSD road category → the shared 0..4 class rank (see `osmRoadClassRank`).
 *  The category lives in PKOD_R — PSILNICE is a BARE number for everything
 *  but D-roads (measured from the 2026 cache: PKOD_R 1='D8', 2='3' (I/3),
 *  3='603' (II/603), 4='11628' (III/11628), 5='D4', 6='34' (I/34, E-route)).
 *  D/expressway → 0, silnice I → 1 (±1 tolerance spans motorway- and
 *  primary-tagged OSM ways), II → 3, III → 4. Unknown codes → 1, the most
 *  restrictive of the plausible readings. */
export function rsdRank(psilnice: string, pkodR: string): number {
  if (pkodR === '1' || pkodR === '5' || /^D\d/.test(psilnice.trim())) return 0
  if (pkodR === '3') return 3
  if (pkodR === '4') return 4
  return 1 // PKOD_R 2/6 — silnice I
}

/** Normalize OSM ref: "I/34" → "34", "II/150" → "150", "D1" → "D1",
 *  "E50" → skip. Multi-value refs ("II/150;E55") take the first token that
 *  normalizes. Every branch is anchored to the WHOLE token — free text like
 *  "Zelená 20" (a street address mis-tagged as ref) must never reduce to "20"
 *  and inherit silnice I/20's count (2026-07 audit, Plzeň parking aisle at
 *  17,531 AADT). */
export function normalizeOsmRef(ref: string): string {
  for (const tok of ref.split(';')) {
    const r = tok.trim()
    if (/^D\d+$/.test(r)) return r          // D1, D5 → keep
    if (/^E\d+$/.test(r)) continue          // E-roads → skip (international numbering)
    // Whole-token road number, with or without the Roman prefix:
    // "I/34" → "34", "II/150" → "150", "III/104a" → "104", "150" → "150".
    const m = r.match(/^(?:[IV]+\/)?(\d+)[a-zA-Z]?$/)
    if (m) return m[1]
  }
  return ''
}

/** Flat-earth distance in meters */
// ── Main ──

async function main() {
  console.log(`=== CZ Road Traffic Enrichment (${YEAR}) ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache: ${CACHE_DIR}\n`)

  if (!existsSync(H3R4_DIR)) {
    console.error(`ERROR: H3R4 directory not found: ${H3R4_DIR}`)
    process.exit(1)
  }

  const features = await downloadCensus()
  console.log(`\n  Parsing ${features.length} census sections...`)
  const censusByRef = parseCensus(features)
  console.log(`  ${censusByRef.size} unique road refs (${zeroSections} un-surveyed all-zero sections skipped)\n`)

  await enrichHexes(censusByRef)
  console.log(`\n=== Done ===`)
}

// Import-safe: run only when invoked directly — the unit tests import the
// exported helpers and must never trigger a download/enrichment (/gg Codex).
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch(err => { console.error('Error:', err); process.exit(1) })
}
