/**
 * Enrich TH roads.arrow with two open-data sources + hand-built corridor tables:
 *
 * 1. **DRR Rural Roads AADT 2024** (กรมทางหลวงชนบท, Department of Rural Roads)
 *    3,415 per-segment AADT records with full CNOSSOS-compatible vehicle class split
 *    (MC, SV, SVT, TB2, TB3, T4, ART3-6, BD, DRT, sum_AADT).
 *    Keyed by `road_code` in Thai-province prefix format like `นบ.3021` (= Nonthaburi
 *    rural road 3021). OSM Thailand tags these as `ref=นบ.3021` — exact match.
 *    - Retrieved via MOT CKAN datastore_dump: the primary `dataportal.drr.go.th`
 *      portal is TCP-blocked from our egress, but the reachable `datagov.mot.go.th`
 *      CKAN mirror has the data pre-ingested in its datastore.
 *    - URL pattern: `https://datagov.mot.go.th/datastore/dump/{resource_id}?format=csv`
 *    - Resource ID: `d0675c68-510b-45e1-b865-1ce261814948`
 *
 * 2. **DOH Motorway Traffic 2024** (กรมทางหลวง, Department of Highways)
 *    192 rows of monthly per-toll-plaza vehicle counts on DOH motorways (ทางหลวงพิเศษ
 *    ระหว่างเมือง = intercity special highways). Used for motorway-class enrichment
 *    (Motorway 7 Bangkok↔Pattaya and Motorway 9 Outer Ring Road).
 *
 * 3. **DOH trunk highway hand table** (`DOH_TRUNK_AADT`) — numeric-`ref` match to
 *    corridor AADT values calibrated from DOH 2021 annual vehicle-km rankings +
 *    published corridor volumes (no open per-section counts exist, see blocked
 *    sources below); split via `thaiClassSplit`.
 *
 * 4. **Unmatched segments** (no `ref`, or `ref` unknown to the tables above)
 *    are left untouched by this enricher (`return null`). Rows still
 *    unenriched after all enrichers resolve through the engine's default
 *    cascade (`defaults.rs` TH country arm + Bangkok city arm) at compute
 *    time — this enricher does NOT stamp class defaults, so the cascade
 *    stays the single source of default values. (A `TH_DEFAULTS` table here
 *    was dead code; removed 2026-07-28.)
 *
 * Blocked sources (TCP egress blocked, documented but not usable):
 * - `opendata.doh.go.th/dataset/ed101df4.../aadt-67.csv` (DOH 2024 per-segment)
 * - `opendata.doh.go.th/dataset/ed101df4.../aadt-68.csv` (DOH 2025)
 * - `data.doh.go.th/dataset/.../gps_doh_shp.zip` (DOH highway shapefile)
 * - `dataportal.drr.go.th/dataset/.../drr_road_240706.zip` (DRR shapefile)
 * - `data.go.th` (Cloudflare WAF geoblocks scraping)
 *
 * The DOH 2021 `aadt-64.csv` is accessible via MOT CKAN datastore but reports
 * annual vehicle-km (VKY) not vehicle count, and no section-length data is
 * published openly, so per-section AADT cannot be derived. DOH falls back to
 * class defaults only.
 *
 * Exclusion zones (Myanmar, Laos, Cambodia, Vietnam, Malaysia) are applied at
 * segment-midpoint level.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-th.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-th.ts --force-download
 */

import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_TH_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { inBbox } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_TH_NATIONAL_ROADS

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/th`)

const forceDownload = process.argv.includes('--force-download')

// TH coarse bbox. Used both to scan hexes (TH_HEX_BBOX, H3-centroid gate in
// iterateCountryHexes) and to gate each segment by its midpoint inside the match
// closure — same box, so the set of touched rows is identical to the legacy loop.
const TH_BBOX: [number, number, number, number] = [5.5, 97.3, 20.5, 105.7]
const TH_HEX_BBOX: [number, number, number, number] = TH_BBOX

// Exclusion zones for neighbour countries inside TH bbox.
// Thailand has complex borders — these bboxes are conservative to avoid clipping
// Thai cities (Chiang Mai 18.8N 98.99E, Nakhon Ratchasima 14.97N 102.1E, etc.)
const EXCLUDE_ZONES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  // Myanmar westernmost strip only (Thai-Myanmar border is narrow and east-jutting)
  { name: 'Myanmar', bbox: [10.0, 97.3, 20.5, 98.3] },
  // Laos (north of TH — Laos is north/east of the Mekong)
  { name: 'Laos', bbox: [17.9, 101.0, 22.5, 106.0] },
  // Cambodia (east of Sa Kaeo / Trat provinces)
  { name: 'Cambodia', bbox: [10.0, 103.3, 14.7, 107.0] },
  // Vietnam (far east, rare in TH bbox but defensive)
  { name: 'Vietnam', bbox: [8.0, 104.5, 23.5, 109.5] },
  // Malaysia south of TH Sungai Kolok
  { name: 'Malaysia', bbox: [1.0, 99.5, 6.3, 104.5] },
]

// Bangkok urban bbox (×1.5 AADT multiplier)
const BANGKOK_BBOX: [number, number, number, number] = [13.5, 100.3, 14.2, 100.9]

function inAnyZone(lat: number, lon: number): boolean {
  for (const z of EXCLUDE_ZONES) if (inBbox(lat, lon, z.bbox)) return true
  return false
}

// ── CSV parsing (simple since DRR/DOH files have no quoted commas) ──

function parseCsv(text: string): Record<string, string>[] {
  const lines = text.split(/\r?\n/).filter(l => l.trim() !== '')
  const headers = lines[0].split(',').map(h => h.trim())
  const out: Record<string, string>[] = []
  for (let i = 1; i < lines.length; i++) {
    const vals = lines[i].split(',')
    const row: Record<string, string> = {}
    for (let j = 0; j < headers.length; j++) row[headers[j]] = vals[j] ?? ''
    out.push(row)
  }
  return out
}

// ── Step 1: Load DRR 2024 AADT ──

interface DrrAadt {
  road_code: string
  aadt_total: number
  mc: number        // motorcycles
  sv: number        // small car (sedan)
  svt: number       // small car trailer
  tb2: number       // 2-axle truck
  tb3: number       // 3-axle truck
  t4: number        // 4+ axle truck
  art3: number      // 3-axle articulated
  art4: number      // 4-axle articulated
  art5: number      // 5-axle articulated
  art6: number      // 6-axle articulated
  bd: number        // bus (large + medium)
  drt: number       // DRT = minor / other
}

async function loadDrrAadt(): Promise<Map<string, DrrAadt>> {
  const path = resolve(CACHE_DIR, 'drr-aadt-2024.csv')
  if (!existsSync(path) || forceDownload) {
    mkdirSync(CACHE_DIR, { recursive: true })
    const url = 'https://datagov.mot.go.th/datastore/dump/d0675c68-510b-45e1-b865-1ce261814948?format=csv'
    console.log(`  Downloading DRR 2024 AADT via MOT CKAN datastore...`)
    const res = await fetch(url, { signal: AbortSignal.timeout(60_000), headers: { 'User-Agent': 'Mozilla/5.0' } })
    if (!res.ok) throw new Error(`DRR CKAN HTTP ${res.status}`)
    writeFileSync(path, Buffer.from(await res.arrayBuffer()))
  }
  const text = readFileSync(path, 'utf-8')
  const rows = parseCsv(text)
  const out = new Map<string, DrrAadt>()
  let skippedNoSplit = 0
  for (const r of rows) {
    const code = (r['road_code'] || '').trim()
    if (!code) continue
    const aadt = parseFloat(r['sum_AADT'] || '0')
    if (!isFinite(aadt) || aadt <= 0) continue
    const rec: DrrAadt = {
      road_code: code,
      aadt_total: aadt,
      mc: parseFloat(r['MC'] || '0'),
      sv: parseFloat(r['SV'] || '0'),
      svt: parseFloat(r['SVT'] || '0'),
      tb2: parseFloat(r['TB2'] || '0'),
      tb3: parseFloat(r['TB3'] || '0'),
      t4: parseFloat(r['T4'] || '0'),
      art3: parseFloat(r['ART3'] || '0'),
      art4: parseFloat(r['ART4'] || '0'),
      art5: parseFloat(r['ART5'] || '0'),
      art6: parseFloat(r['ART6'] || '0'),
      bd: parseFloat(r['BD'] || '0'),
      drt: parseFloat(r['DRT'] || '0'),
    }
    // sum_AADT can be positive while every per-class column (MC/SV/SVT/TB2/.../DRT)
    // is blank — the same shape as DE's BASt SVZ sections (#31.4): drrToCnossos
    // would then compute an all-zero split under this MEASURED id. Skip and count
    // rather than fabricate a class breakdown the source never published.
    const split = drrToCnossos(rec)
    if (split.light + split.medium + split.heavy + split.moto === 0) { skippedNoSplit++; continue }
    out.set(code, rec)
  }
  console.log(`  DRR roads loaded: ${out.size}, ${skippedNoSplit} skipped (sum_AADT without class split)`)
  return out
}

// Convert DRR per-class to CNOSSOS categories
function drrToCnossos(d: DrrAadt): { light: number; medium: number; heavy: number; moto: number } {
  // CNOSSOS-EU (Dir 2015/996):
  //   light  = passenger cars (+ car-with-trailer)
  //   medium = 2-axle trucks + buses + minor/other (cat2)
  //   heavy  = 3+ axle trucks + articulated (cat3)
  //   moto   = motorcycles
  // bd (buses) are cat2 medium, NOT heavy — only articulated buses would be cat3.
  return {
    light: Math.round(d.sv + d.svt),
    medium: Math.round(d.tb2 + d.bd + d.drt),
    heavy: Math.round(d.tb3 + d.t4 + d.art3 + d.art4 + d.art5 + d.art6),
    moto: Math.round(d.mc),
  }
}

// ── Step 2: DOH motorway/trunk hand tables ──

// Thai vehicle split for the DOH tables below.
// Thailand has very high motorcycle share: ~25% urban, ~15% rural
// Cars dominate, trucks moderate, buses minor
function thaiClassSplit(aadt: number, isBangkok: boolean): { light: number; medium: number; heavy: number; moto: number } {
  if (isBangkok) {
    return {
      light: Math.round(aadt * 0.60),
      medium: Math.round(aadt * 0.08),
      heavy: Math.round(aadt * 0.07),
      moto: Math.round(aadt * 0.25),
    }
  }
  return {
    light: Math.round(aadt * 0.62),
    medium: Math.round(aadt * 0.10),
    heavy: Math.round(aadt * 0.13),
    moto: Math.round(aadt * 0.15),
  }
}

// Known DOH motorway average AADT (2024 actual operator data + Bangkok reality)
const DOH_MOTORWAY_AADT: Record<string, number> = {
  '7': 120000,   // Motorway 7 Bangkok↔Pattaya (Chonburi)
  '9': 100000,   // Motorway 9 Outer Ring Road
  '81': 50000,   // Motorway 81 Bangyai↔Kanchanaburi (partial open)
  '82': 40000,   // Motorway 82 Bang Khun Thian↔Ekkachai
}

// Known high-volume DOH trunk highways (ref → rough AADT)
// These are Highway 1 Phahonyothin, 2 Mittraphap, 4 Phetkasem etc.
const DOH_TRUNK_AADT: Record<string, { rural: number; bkk: number }> = {
  '1':  { rural: 35000, bkk: 95000 },  // Phahonyothin (Bangkok↔Mae Sai)
  '2':  { rural: 30000, bkk: 85000 },  // Mittraphap (Bangkok↔Nong Khai)
  '3':  { rural: 25000, bkk: 75000 },  // Sukhumvit (Bangkok↔Trat)
  '4':  { rural: 28000, bkk: 80000 },  // Phetkasem (Bangkok↔Padang Besar)
  '11': { rural: 20000, bkk: 40000 },  // Bangkok↔Lampang↔Chiang Rai
  '12': { rural: 15000, bkk: 30000 },  // Tak↔Mukdahan east-west corridor
  '22': { rural: 18000, bkk: 35000 },  // Udon Thani↔Nakhon Phanom
  '24': { rural: 15000, bkk: 25000 },  // Sikhio↔Ubon Ratchathani
  '32': { rural: 45000, bkk: 85000 },  // Bangkok Pak Nam↔Nakhon Sawan (Asian Highway 2)
  '33': { rural: 22000, bkk: 40000 },  // Kabin Buri↔Aranyaprathet
  '34': { rural: 80000, bkk: 120000 }, // Bang Na↔Chachoengsao (Burapha Withi)
  '35': { rural: 90000, bkk: 130000 }, // Dao Khanong↔Pak Tho (Rama II Road)
  '41': { rural: 20000, bkk: 35000 },  // Chumphon↔Phatthalung
  '304': { rural: 15000, bkk: 35000 }, // Bangkok↔Chachoengsao
}

// ── Step 3: iterate hexes and match ──

async function main() {
  console.log(`=== TH Roads Enrichment — DRR census + DOH motorway/trunk tables (${YEAR}) ===\n`)
  mkdirSync(CACHE_DIR, { recursive: true })

  const drrMap = await loadDrrAadt()

  const hexDirs = iterateCountryHexes(H3R4_DIR, TH_HEX_BBOX)
  console.log(`\nTH-bbox hexes with roads.arrow: ${hexDirs.length}`)

  let totalRoads = 0, excluded = 0, alreadyEnriched = 0
  let matchedDrr = 0, matchedDohMotorway = 0, matchedDohTrunk = 0
  let hexesUpdated = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        // Fast-exit before the expensive ref-match when a higher-priority dataset
        // already owns the row (writeRoadAadt re-checks the gate — this only saves work).
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { alreadyEnriched++; return null }

        const midLat = row.midLat
        const midLon = row.midLon

        if (!inBbox(midLat, midLon, TH_BBOX)) return null
        if (inAnyZone(midLat, midLon)) { excluded++; return null }

        const ref = String(row.ref ?? '').trim()
        const isBkk = inBbox(midLat, midLon, BANGKOK_BBOX)

        let aadtLi = 0, aadtMe = 0, aadtHv = 0, aadtMo = 0
        let source: 'drr' | 'motorway' | 'trunk' | null = null

        // Tier 1: DRR exact ref match (Thai prefix like นบ.3021)
        if (ref && drrMap.has(ref)) {
          const d = drrMap.get(ref)!
          const split = drrToCnossos(d)
          aadtLi = split.light
          aadtMe = split.medium
          aadtHv = split.heavy
          aadtMo = split.moto
          source = 'drr'
          matchedDrr++
        }

        // Tier 2: DOH motorway numeric ref
        if (!source && ref) {
          // Split multi-refs: "7;9" → check each
          for (const tok of ref.split(/[;,]/)) {
            const t = tok.trim()
            if (DOH_MOTORWAY_AADT[t]) {
              const aadt = DOH_MOTORWAY_AADT[t]
              const split = thaiClassSplit(aadt, isBkk)
              aadtLi = split.light; aadtMe = split.medium; aadtHv = split.heavy; aadtMo = split.moto
              source = 'motorway'
              matchedDohMotorway++
              break
            }
          }
        }

        // Tier 3: DOH trunk highway numeric ref
        if (!source && ref) {
          for (const tok of ref.split(/[;,]/)) {
            const t = tok.trim()
            if (DOH_TRUNK_AADT[t]) {
              const aadt = isBkk ? DOH_TRUNK_AADT[t].bkk : DOH_TRUNK_AADT[t].rural
              const split = thaiClassSplit(aadt, isBkk)
              aadtLi = split.light; aadtMe = split.medium; aadtHv = split.heavy; aadtMo = split.moto
              source = 'trunk'
              matchedDohTrunk++
              break
            }
          }
        }

        // A.1: no spatial or ref match → leave source_id = 0, engine applies
        // country-tier cascade (TH rural + Bangkok city overrides in defaults.rs).
        if (!source) return null

        return { light: aadtLi, medium: aadtMe, heavy: aadtHv, moto: aadtMo, sourceId: MY_SOURCE_ID }
      },
    )
    totalRoads += r.rows
    if (r.updated) hexesUpdated++

    const elapsed = Date.now() - startTime
    if (elapsed > 10_000 && hi % 20 === 0) {
      console.log(`  [${(elapsed / 1000).toFixed(0)}s] ${hi + 1}/${hexDirs.length} hexes, ${matchedDrr + matchedDohMotorway + matchedDohTrunk} matched`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  Total roads scanned:     ${totalRoads.toLocaleString()}`)
  console.log(`  Already enriched (skip): ${alreadyEnriched.toLocaleString()}`)
  console.log(`  Excluded (neighbours):   ${excluded.toLocaleString()}`)
  console.log(`  Matched by DRR ref:      ${matchedDrr.toLocaleString()} (real AADT + per-class)`)
  console.log(`  Matched by DOH motorway: ${matchedDohMotorway.toLocaleString()}`)
  console.log(`  Matched by DOH trunk:    ${matchedDohTrunk.toLocaleString()}`)
  const total = matchedDrr + matchedDohMotorway + matchedDohTrunk
  console.log(`  Total enriched:          ${total.toLocaleString()} (${(100 * total / Math.max(totalRoads, 1)).toFixed(2)}%)`)
  console.log(`  Hexes updated:           ${hexesUpdated}/${hexDirs.length}`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
