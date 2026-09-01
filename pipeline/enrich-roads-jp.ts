/**
 * Enrich JP roads.arrow from the MLIT 道路交通センサス 令和3年度 (Road Traffic
 * Census 2021) — per-section 24h AADT split into 小型車 (small) / 大型車 (large),
 * published as 47 per-prefecture Shift-JIS CSVs (~230k surveyed sections).
 *
 * Why a name/ref join, not a spatial one: the census is keyed by 交通調査基本区間
 * 番号 (kasyo section id), which resolves to geometry only through the proprietary
 * DRM (Digital Road Map, 道路地図センター) — closed/paid. KSJ N13 road network
 * carries no kasyo id either, so there is no open geometry to join against. What
 * the census DOES carry is each road's real identity — 道路種別 (road type),
 * 路線番号 (route number), 路線名 (route name) — and Japanese OSM tags major roads
 * with exactly those, so we join by identity instead of coordinate:
 *
 *   道路種別 1/2  national + urban expressway  → OSM motorway      → match by NAME
 *   道路種別 3    一般国道 general national hwy → OSM trunk/primary → match by REF (国道番号)
 *   道路種別 4/6  主要地方道 / 一般都道府県道   → OSM secondary/tert → census class-median fallback
 *
 * Each matched road gets the NATIONAL MEDIAN of its route's measured sections.
 * Limitation: a route's AADT varies along its length (国道1号 ≈ 60k in Tokyo, ≈15k
 * rural) and we collapse it to one median — without geometry we cannot place an
 * OSM segment on the route. The median still lands far closer than the global
 * default (21.6k) on the urban corridors where population, and exposure,
 * concentrate. Major roads with no name/ref match fall back to the census-
 * measured median for their OSM class, not a guessed default.
 *
 * Vehicle split: 小型車 → light (CNOSSOS Cat1: cars + light vans); 大型車 (buses +
 * large trucks) → medium/heavy 25/75 (Cat2 buses+2-axle / Cat3 artic — the census
 * gives no axle split, the stream is truck-dominated). Motorcycles are not in the
 * 自動車類 24h count, so moto = 0.
 *
 * Source: MLIT https://www.mlit.go.jp/road/census/r3/ (Government of Japan
 * Standard Terms of Use v2.0, CC-BY-4.0 compatible).
 * Cache: data/enrichment/2026/jp/kasyo{01..47}.csv (Shift-JIS, 159 columns).
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-jp.ts
 */

import { readFileSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { cellToLatLng } from 'h3-js'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_JP_NATIONAL_ROADS, SOURCE_ID_JP_CLASS_MEDIAN_FALLBACK } from './lib/source-ids.generated.js'
import { inBbox } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes, type RoadRow } from './lib/roads-arrow.js'
import { makeCoastalCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_JP_NATIONAL_ROADS
const MEDIAN_FALLBACK_SOURCE_ID = SOURCE_ID_JP_CLASS_MEDIAN_FALLBACK
const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/jp`)

// JP coarse bbox [minLat,minLon,maxLat,maxLon]: Yonaguni (24.4°N,123°E) → Hokkaido
// (45.5°N) / Nemuro (145.8°E). Sweeps in S+N Korea, a China-coast sliver and the
// southern Kuriles — the makeCoastalCountryGate('JP') polygon is the real filter; this
// box only skips the rest of the planet cheaply.
const JP_HEX_BBOX: [number, number, number, number] = [24.0, 122.0, 46.0, 146.0]

// CNOSSOS coverage: major roads only. Residential/service keep the global
// service-tree heuristic — the census never surveyed them.
const JP_COVERAGE: ReadonlySet<number> = new Set([0, 1, 2, 3, 4, 10, 11, 12])

// 大型車 (buses + large trucks) → CNOSSOS medium (Cat2) / heavy (Cat3). The census
// publishes no axle split; 25/75 reflects a truck-dominated large-vehicle stream.
const MEDIUM_OF_LARGE = 0.25

// 0-based column indices in the 159-col R3 census CSV (see KasyoFormat.xlsx):
const C_TYPE = 3 // 道路種別 (1 expwy / 2 urban expwy / 3 国道 / 4 主要地方道 / 6 一般県道)
const C_ROUTE = 4 // 路線番号
const C_NAME = 5 // 路線名
const C_SMALL = 59 // ２４時間自動車類交通量（上下合計）／小型車（台）
const C_LARGE = 60 // ２４時間自動車類交通量（上下合計）／大型車（台）

/** Measured 24h two-way vehicle counts for one census section. */
interface Veh { small: number; large: number }

function median(xs: number[]): number {
  if (xs.length === 0) return 0
  const s = [...xs].sort((a, b) => a - b)
  const m = s.length >> 1
  return s.length % 2 ? s[m] : Math.round((s[m - 1] + s[m]) / 2)
}

/** Full-width digits → half-width, drop whitespace — for robust name/ref keys. */
function norm(s: string): string {
  return s
    .replace(/[０-９]/g, (d) => String.fromCharCode(d.charCodeAt(0) - 0xfee0))
    .replace(/\s+/g, '')
    .trim()
}

/** Leading run of (normalized) digits, e.g. "国道１６号" → "16", "16;17" → "16". */
function leadingDigits(s: string): string {
  const m = norm(s).match(/\d+/)
  return m ? m[0] : ''
}

interface Census {
  natlByRef: Map<string, Veh> // 道路種別3 一般国道: route number → national median
  expwyByName: Map<string, Veh> // 道路種別1+2 expressway: normalized 路線名 → national median
  expwyNames: string[] // expwy keys, longest-first, for prefix matching
  classMed: Map<number, Veh> // OSM road_class → census-measured median (fallback)
  sections: number
}

// census 道路種別 → the OSM road_class buckets it feeds for the class-median
// fallback. National highways (3) appear as both trunk and primary in OSM Japan;
// 主要地方道 (4) as primary and secondary; 一般県道 (6) as tertiary.
const TYPE_TO_CLASSES: Record<string, number[]> = {
  '1': [0],
  '2': [0],
  '3': [1, 2],
  '4': [2, 3],
  '6': [4],
}

function loadCensus(): Census {
  const refRows = new Map<string, Veh[]>()
  const nameRows = new Map<string, Veh[]>()
  const classRows = new Map<number, Veh[]>()
  const pushArr = <K>(m: Map<K, Veh[]>, k: K, v: Veh) => {
    const a = m.get(k)
    if (a) a.push(v)
    else m.set(k, [v])
  }

  let sections = 0
  // #31.2 completeness gate: EVERY prefecture must parse non-empty BEFORE any
  // write. The class-median fallback is computed over ALL loaded sections, so a
  // 1/47 fragment would silently become the NATIONWIDE default for every
  // unmatched major road — a partial cache must hard-fail, never degrade.
  const unusable: string[] = []
  for (let p = 1; p <= 47; p++) {
    const fileName = `kasyo${String(p).padStart(2, '0')}.csv`
    const path = resolve(CACHE_DIR, fileName)
    if (!existsSync(path)) {
      unusable.push(`${fileName} (missing)`)
      continue
    }
    const sectionsBefore = sections
    const text = new TextDecoder('shift_jis').decode(readFileSync(path))
    const lines = text.split(/\r?\n/)
    for (let i = 1; i < lines.length; i++) {
      const line = lines[i]
      if (!line) continue
      const f = line.split(',')
      if (f.length <= C_LARGE) continue
      // Usable = a row the enrichment actually consumes: surveyed AND of a
      // mapped road type. Counting unmapped types let a malformed future
      // fragment pass the 47-file gate while contributing nothing (#31.2).
      const type = f[C_TYPE]
      const classes = TYPE_TO_CLASSES[type] ?? []
      if (classes.length === 0) continue
      const small = parseInt(f[C_SMALL], 10) || 0
      const large = parseInt(f[C_LARGE], 10) || 0
      if (small + large <= 0) continue // 推定不能 / unsurveyed — would poison the median
      const v: Veh = { small, large }
      sections++

      for (const cls of classes) pushArr(classRows, cls, v)
      if (type === '3') {
        const ref = leadingDigits(f[C_ROUTE])
        if (ref) pushArr(refRows, ref, v)
      } else if (type === '1' || type === '2') {
        const name = norm(f[C_NAME])
        if (name) pushArr(nameRows, name, v)
      }
    }
    if (sections === sectionsBefore) unusable.push(`${fileName} (0 usable sections — wrong encoding/format or empty file)`)
  }
  if (unusable.length > 0) {
    throw new Error(
      `JP census incomplete: ${unusable.length}/47 prefecture file(s) unusable in ${CACHE_DIR}: ` +
      `${unusable.join(', ')}. Refusing to enrich from a fragment — re-stage all 47 kasyo CSVs first.`,
    )
  }

  const toMed = (arr: Veh[]): Veh => ({
    small: median(arr.map((v) => v.small)),
    large: median(arr.map((v) => v.large)),
  })
  const natlByRef = new Map([...refRows].map(([k, a]) => [k, toMed(a)]))
  const expwyByName = new Map([...nameRows].map(([k, a]) => [k, toMed(a)]))
  const classMed = new Map([...classRows].map(([k, a]) => [k, toMed(a)]))
  // Link classes fall back to their parent mainline's median — the match
  // arm halves it again (`roadClass >= 10 ? half(fb)`), so store the parent
  // median verbatim. Without the keys the link fallback was dead code and
  // the 9865-retract could zero a link with no re-claim path (/gg M1b).
  for (const [link, parent] of [[10, 0], [11, 1], [12, 2]] as const) {
    const p = classMed.get(parent)
    if (p) classMed.set(link, p)
  }
  const expwyNames = [...expwyByName.keys()].sort((a, b) => b.length - a.length)

  console.log(`  census: 47/47 prefecture files (hard-gated above), ${sections.toLocaleString()} usable sections`)
  console.log(`    routes: ${natlByRef.size} 国道 numbers, ${expwyByName.size} expressway names`)
  return { natlByRef, expwyByName, expwyNames, classMed, sections }
}

/** OSM expressway names vary against the census (新東名 vs 東名, 中央自動車道 vs
 *  中央自動車道富士吉田線), so resolve by exact key then longest containing key.
 *  Memoized per distinct OSM name — the same name recurs across thousands of rows. */
function makeExpwyResolver(c: Census): (osmName: string) => Veh | null {
  const cache = new Map<string, Veh | null>()
  return (osmName: string): Veh | null => {
    const key = norm(osmName)
    if (key.length < 4) return null // too short to disambiguate (e.g. a bare "1")
    const hit = cache.get(key)
    if (hit !== undefined) return hit
    let v = c.expwyByName.get(key) ?? null
    if (!v) {
      // expwyNames is longest-first → the first containment is the most specific
      for (const ck of c.expwyNames) {
        if (ck.length < 4) break
        if (ck.includes(key) || key.includes(ck)) {
          v = c.expwyByName.get(ck) ?? null
          break
        }
      }
    }
    cache.set(key, v)
    return v
  }
}

const half = (v: Veh): Veh => ({ small: Math.round(v.small / 2), large: Math.round(v.large / 2) })

function toAadt(v: Veh, tier: Tier) {
  const medium = Math.round(v.large * MEDIUM_OF_LARGE)
  return {
    light: v.small,
    medium,
    heavy: v.large - medium,
    moto: 0,
    // Census-matched rows are measured; the class-median fallback is a
    // census-derived proxy — they must not share one measured id (owner
    // review 2026-07-28: a proxy fallback stamped "measured" would skip
    // access_factor and outrank real measurements of neighbours).
    sourceId: tier === 'class-fallback' ? MEDIAN_FALLBACK_SOURCE_ID : MY_SOURCE_ID,
  }
}

type Tier = 'expwy-name' | 'natl-ref' | 'natl-name' | 'class-fallback'

async function main() {
  console.log(`=== JP Roads Enrichment — MLIT R3 道路交通センサス name/ref join (${YEAR}) ===\n`)
  const census = loadCensus()
  const resolveExpwy = makeExpwyResolver(census)

  // National-highway resolver: ref number (国道番号) for trunks, explicit 国道N号
  // name for primaries (a bare ref on a primary may be a colliding 県道 number).
  const resolveNatl = (row: RoadRow): { v: Veh; tier: Tier } | null => {
    if (row.roadClass === 1 || row.roadClass === 11) {
      for (const tok of (row.ref ?? '').split(/[;,/]/)) {
        const d = leadingDigits(tok)
        const v = d ? census.natlByRef.get(d) : undefined
        if (v) return { v, tier: 'natl-ref' }
      }
    }
    if (row.name) {
      const m = norm(row.name).match(/(?:一般)?国道(\d+)号/)
      const v = m ? census.natlByRef.get(m[1]) : undefined
      if (v) return { v, tier: 'natl-name' }
    }
    return null
  }

  // Created here (not module scope): makeCoastalCountryGate may download+convert the CGAZ
  // boundary on a fresh host. JP shares the bbox with the Korean peninsula and a
  // China-coast sliver; the polygon stops JP AADT bleeding onto their roads.
  const inJP = makeCoastalCountryGate('JP')

  const hexDirs = iterateCountryHexes(H3R4_DIR, JP_HEX_BBOX)
  console.log(`\n  JP-bbox hexes with roads.arrow: ${hexDirs.length}\n`)

  let totalRoads = 0
  let enriched = 0
  let hexesUpdated = 0
  let preserved = 0
  let retractedTotal = 0
  let outsideJP = 0
  const byClass: Record<number, number> = {}
  const byTier: Record<Tier, number> = { 'expwy-name': 0, 'natl-ref': 0, 'natl-name': 0, 'class-fallback': 0 }
  let sumHeavy = 0
  let sumAll = 0
  let pendingTier: Tier = 'class-fallback'
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    // Interior-hex fast path: if the hex centroid and a coarse cross of probes are
    // all inside JP, skip the per-row polygon test. Border hexes (Tsushima ↔ Busan,
    // Kuriles) fail a probe and fall back to per-row gating.
    const [clat, clon] = cellToLatLng(hex)
    const interior = [
      [0, 0], [0.45, 0.45], [0.45, -0.45], [-0.45, 0.45], [-0.45, -0.45],
    ].every(([dy, dx]) => inJP(clat + dy, clon + dx))

    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }

        // Find candidate AADT by tier before the expensive polygon gate.
        let v: Veh | null = null
        let tier: Tier = 'class-fallback'
        if (row.roadClass === 0 || row.roadClass === 10) {
          const ev = row.name ? resolveExpwy(row.name) : null
          if (ev) { v = row.roadClass === 10 ? half(ev) : ev; tier = 'expwy-name' }
        } else if (row.roadClass <= 2 || row.roadClass === 11 || row.roadClass === 12) {
          const nv = resolveNatl(row)
          if (nv) { v = row.roadClass >= 11 ? half(nv.v) : nv.v; tier = nv.tier }
        }
        if (!v) {
          const fb = census.classMed.get(row.roadClass)
          if (fb) { v = row.roadClass >= 10 ? half(fb) : fb; tier = 'class-fallback' }
        }
        if (!v) return null

        if (!inBbox(row.midLat, row.midLon, JP_HEX_BBOX)) return null
        if (!interior && !inJP(row.midLat, row.midLon)) { outsideJP++; return null }

        pendingTier = tier
        return toAadt(v, tier)
      },
      (row, _i, applied) => {
        enriched++
        byClass[row.roadClass] = (byClass[row.roadClass] || 0) + 1
        byTier[pendingTier]++
        sumHeavy += applied.heavy
        sumAll += applied.light + applied.medium + applied.heavy
      },
      JP_COVERAGE,
      {
        // Legacy rows stamped 9392 by the class-median fallback must move to
        // the proxy id 9865 (9392 is now measured-tier and would otherwise
        // outrank its own proxy rows). Zero + re-claim in the same pass.
        sourceId: MY_SOURCE_ID,
        when: (row) => {
          // Rows the pass can never claim (outside coverage — legacy
          // mis-stamps) must be disowned, not silently promoted to measured.
          if (!JP_COVERAGE.has(row.roadClass)) return true
          if (row.roadClass === 0 || row.roadClass === 10) {
            return !(row.name && resolveExpwy(row.name))
          }
          if (row.roadClass <= 2 || row.roadClass === 11 || row.roadClass === 12) {
            return !resolveNatl(row)
          }
          return census.classMed.has(row.roadClass)
        },
      },
    )
    totalRoads += r.rows
    retractedTotal += r.retracted
    if (r.updated) hexesUpdated++

    const elapsed = Date.now() - startTime
    if (elapsed > 10_000 && hi % 100 === 0) {
      console.log(`  [${(elapsed / 1000).toFixed(0)}s] ${hi + 1}/${hexDirs.length} hexes, ${enriched.toLocaleString()} enriched`)
    }
  }

  const CLASS_NAME: Record<number, string> = {
    0: 'motorway', 1: 'trunk', 2: 'primary', 3: 'secondary', 4: 'tertiary',
    10: 'motorway_link', 11: 'trunk_link', 12: 'primary_link',
  }
  console.log(`\n=== Results ===`)
  console.log(`  Total roads scanned:     ${totalRoads.toLocaleString()}`)
  console.log(`  Preserved (higher prio): ${preserved.toLocaleString()}`)
  console.log(`  Outside JP polygon:      ${outsideJP.toLocaleString()}`)
  console.log(`  Enriched:                ${enriched.toLocaleString()} (${(100 * enriched / Math.max(totalRoads, 1)).toFixed(1)}%)`)
  console.log(`  Retracted 9392→9865/0:   ${retractedTotal.toLocaleString()}`)
  console.log(`  Hexes updated:           ${hexesUpdated}/${hexDirs.length}`)
  console.log(`\n  By road class:`)
  for (const cls of Object.keys(byClass).map(Number).sort((a, b) => a - b)) {
    console.log(`    ${CLASS_NAME[cls] ?? cls}: ${byClass[cls].toLocaleString()}`)
  }
  console.log(`\n  By match tier:`)
  for (const [k, n] of Object.entries(byTier).sort((a, b) => b[1] - a[1])) {
    console.log(`    ${k}: ${n.toLocaleString()}`)
  }
  console.log(`\n  Mean heavy share (heavy/(light+med+heavy)): ${(100 * sumHeavy / Math.max(sumAll, 1)).toFixed(1)}%  (census ~15-37% by class)`)
}

main().catch((err) => { console.error('Error:', err); process.exit(1) })
