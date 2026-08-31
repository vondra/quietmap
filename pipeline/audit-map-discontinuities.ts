//! R9 discontinuity scanner — READ-ONLY worldwide sweep for "the red suddenly
//! ends / jumps" (owner 2026-07-10): junction-free steps in the RESOLVED road
//! inputs (reusing the R7 taper detector in report mode) + the two rail
//! signatures from the Trať 162 investigation (fallback-stamped-as-measured
//! train counts; train-count cliffs at degree-2 nodes). No writes — output is
//! a per-country ranking + top locations, the accuracy worklist that feeds
//! R1b/R2/R4/#26 fixes.
//!
//! Country legal speeds come from the same generated table the engine uses;
//! the hex's country comes from prepared/h3r4-admin.bin (the engine's own
//! receiver-country approximation). A country absent from the speed table
//! degrades to the legacy world speeds — exactly what the engine does.
//!
//! Usage:
//!   DATA_YEAR=2026 npx tsx pipeline/audit-map-discontinuities.ts --bbox S,W,N,E [--out report.json]
//!   SHARD=0/8 ... --bbox -90,-180,90,180   # world run, shardable like the enrichers

import { readFileSync, existsSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { tableFromIPC } from 'apache-arrow'
import { buildTaperPlan, type CountrySpeeds } from './lib/roads-taper-plan.js'
import { readSegs } from './enrich-roads-taper.js'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import { nodeKey } from './lib/spatial.js'
import { DATA_YEAR as YEAR } from './lib/data-year.js'
import { readAdminIso } from './lib/admin-iso.js'

const COUNTRY_SPEED_TABLE = JSON.parse(
  readFileSync(resolve(import.meta.dirname, 'lib/country-speed-defaults.generated.json'), 'utf8'),
) as Record<string, CountrySpeeds>
/** Engine fallback when a country has no legal-speed row (admin unknown /
 *  table miss): all-zero buckets → resolveSpeed's legacy world table. */
const NO_COUNTRY: CountrySpeeds = [0, 0, 0, 0]

/** The canonical PRE-2026-07-10 rail-enricher fallback tuples (pax, frt) by
 *  usage / rail_type. The live fallback was purged under task #26 — each
 *  enricher keeps its tuple table only as an OLD_FALLBACK retract signature
 *  (th/ae/kr/cn/in had country-specific tuples this canonical table does NOT
 *  cover). Until the world rail repaint, stamped DATA still carries these:
 *  a row exactly matching its tuple is a legacy fallback masquerading as
 *  measured data. */
const RAIL_FALLBACK: Array<{ railType?: number; usage?: number; pax: number; frt: number }> = [
  { railType: 2, pax: 250, frt: 0 },
  { railType: 1, pax: 200, frt: 0 },
  { railType: 3, pax: 30, frt: 0 },
  { railType: 4, pax: 30, frt: 0 },
  { usage: 1, pax: 80, frt: 0 },
  { usage: 2, pax: 0, frt: 8 },
  { usage: 0, pax: 50, frt: 10 },
]
/** Train-total ratio between adjacent same-line segments that counts as a
 *  cliff — mirrors the road auditor's R5 flow-jump threshold. */
const RAIL_STEP_RATIO = 3

interface CountryAgg {
  hexes: number
  roadBoundaries: number
  kinds: Record<string, number>
  railFallbackRows: number
  railStampedRows: number
  railFullFallbackWays: number
  railWays: number
  railSteps: number
}

interface TopEntry { lat: number; lon: number; db: number; kind: string; iso: string }

interface RailScan { fallbackRows: number; stampedRows: number; fullFallbackWays: number; ways: number; steps: number; top: TopEntry[] }

function scanRailways(arrowPath: string, iso: string): RailScan {
  const r: RailScan = { fallbackRows: 0, stampedRows: 0, fullFallbackWays: 0, ways: 0, steps: 0, top: [] }
  if (!existsSync(arrowPath)) return r
  const t = tableFromIPC(readFileSync(arrowPath))
  const col = (n: string) => t.getChild(n)
  const [osmC, sLatC, sLonC, eLatC, eLonC, rtC, usC, paxC, frtC, srcC] = [
    'osm_id', 'start_lat', 'start_lon', 'end_lat', 'end_lon', 'rail_type', 'usage',
    'trains_passenger', 'trains_freight', 'source_id',
  ].map(col)
  if (!osmC || !sLatC || !paxC) return r

  const isFallback = (rt: number, us: number, pax: number, frt: number): boolean =>
    RAIL_FALLBACK.some((f) =>
      (f.railType !== undefined ? f.railType === rt && rt !== 0 : rt === 0 && f.usage === us) &&
      f.pax === pax && f.frt === frt)

  // Per-way roll-up + endpoint degree for the step detector.
  const wayRows = new Map<number, { rows: number; fallback: number }>()
  const nodeTotals = new Map<string, Array<{ total: number; lat: number; lon: number; way: number }>>()
  const nodeEdges = new Map<string, number>()
  for (let i = 0; i < t.numRows; i++) {
    const src = (srcC?.get(i) as number) ?? 0
    const pax = (paxC?.get(i) as number) ?? 0
    const frt = (frtC?.get(i) as number) ?? 0
    const rt = (rtC?.get(i) as number) ?? 0
    const us = (usC?.get(i) as number) ?? 0
    const way = Number(osmC.get(i))
    const sLat = sLatC.get(i) as number
    const sLon = sLonC?.get(i) as number
    const eLat = (eLatC?.get(i) as number) ?? sLat
    const eLon = (eLonC?.get(i) as number) ?? sLon
    if (src !== 0) {
      r.stampedRows++
      const w = wayRows.get(way) ?? { rows: 0, fallback: 0 }
      w.rows++
      if (isFallback(rt, us, pax, frt)) {
        r.fallbackRows++
        w.fallback++
      }
      wayRows.set(way, w)
    }
    const total = pax + frt
    for (const [k, lat, lon] of [[nodeKey(sLat, sLon), sLat, sLon], [nodeKey(eLat, eLon), eLat, eLon]] as const) {
      nodeEdges.set(k, (nodeEdges.get(k) ?? 0) + 1)
      let arr = nodeTotals.get(k)
      if (!arr) nodeTotals.set(k, (arr = []))
      arr.push({ total, lat, lon, way })
    }
  }
  r.ways = wayRows.size
  for (const w of wayRows.values()) if (w.rows > 0 && w.fallback === w.rows) r.fullFallbackWays++

  // Degree-2 nodes where the train total steps ≥3×: a physically impossible
  // mid-line cliff (trains do not vanish between stations).
  for (const [k, arr] of nodeTotals) {
    if (nodeEdges.get(k) !== 2 || arr.length !== 2) continue
    const [a, b2] = arr
    const hi = Math.max(a.total, b2.total)
    const lo = Math.min(a.total, b2.total)
    if (lo * RAIL_STEP_RATIO < hi && hi > 0 && lo >= 0) {
      r.steps++
      if (r.top.length < 5) {
        r.top.push({ lat: a.lat, lon: a.lon, db: 10 * Math.log10(hi / Math.max(1, lo)), kind: 'rail-step', iso })
      }
    }
  }
  return r
}

async function main() {
  const H3R4_DIR = resolve(import.meta.dirname, `../data/prepared/${YEAR}/h3r4`)
  const bboxArg = process.argv.includes('--bbox') ? process.argv[process.argv.indexOf('--bbox') + 1] : ''
  const outArg = process.argv.includes('--out') ? process.argv[process.argv.indexOf('--out') + 1] : ''
  const BBOX = bboxArg.split(',').map(Number) as [number, number, number, number]
  if (BBOX.length !== 4 || BBOX.some((x) => !Number.isFinite(x))) {
    console.error('Usage: audit-map-discontinuities.ts --bbox minLat,minLon,maxLat,maxLon [--out report.json]')
    process.exit(1)
  }
  const adminIso = readAdminIso(resolve(import.meta.dirname, '../data/prepared/h3r4-admin.bin'))

  let hexes = iterateCountryHexes(H3R4_DIR, BBOX).sort()
  if (process.env.SHARD) {
    const m = /^(\d+)\/(\d+)$/.exec(process.env.SHARD)
    if (!m) { console.error(`invalid SHARD=${process.env.SHARD}`); process.exit(1) }
    const [i, n] = [Number(m[1]), Number(m[2])]
    hexes = hexes.slice(Math.floor((i * hexes.length) / n), Math.floor(((i + 1) * hexes.length) / n))
  }
  console.log(`discontinuity scan: ${hexes.length} hexes in bbox ${BBOX.join(',')}`)

  const byCountry = new Map<string, CountryAgg>()
  const top: TopEntry[] = []
  const agg = (iso: string): CountryAgg => {
    let a = byCountry.get(iso)
    if (!a) byCountry.set(iso, (a = {
      hexes: 0, roadBoundaries: 0, kinds: {}, railFallbackRows: 0, railStampedRows: 0,
      railFullFallbackWays: 0, railWays: 0, railSteps: 0,
    }))
    return a
  }

  for (const hex of hexes) {
    const hexKey = BigInt('0x' + hex).toString(16)
    const iso = adminIso.get(hexKey) ?? '??'
    const a = agg(iso)
    a.hexes++

    const roadsPath = resolve(H3R4_DIR, hex, 'roads.arrow')
    if (existsSync(roadsPath)) {
      const segs = readSegs(roadsPath)
      const { stats } = buildTaperPlan(segs, COUNTRY_SPEED_TABLE[iso] ?? NO_COUNTRY)
      a.roadBoundaries += stats.boundaries
      for (const [k, v] of Object.entries(stats.kindCounts)) a.kinds[k] = (a.kinds[k] ?? 0) + v
      for (const t2 of stats.top.slice(0, 5)) top.push({ ...t2, iso })
    }

    const rail = scanRailways(resolve(H3R4_DIR, hex, 'railways.arrow'), iso)
    a.railFallbackRows += rail.fallbackRows
    a.railStampedRows += rail.stampedRows
    a.railFullFallbackWays += rail.fullFallbackWays
    a.railWays += rail.ways
    a.railSteps += rail.steps
    top.push(...rail.top)
  }

  top.sort((x, y) => y.db - x.db)
  const report = {
    bbox: BBOX,
    shard: process.env.SHARD ?? null,
    byCountry: Object.fromEntries([...byCountry].sort((a, b) => b[1].roadBoundaries - a[1].roadBoundaries)),
    top: top.slice(0, 100),
  }
  if (outArg) writeFileSync(outArg, JSON.stringify(report, null, 1))

  console.log('\n=== per country (road boundaries ≥2 dB / rail fallback share) ===')
  for (const [iso, a] of [...byCountry].sort((x, y) => y[1].roadBoundaries - x[1].roadBoundaries).slice(0, 20)) {
    const fb = a.railStampedRows ? Math.round((100 * a.railFallbackRows) / a.railStampedRows) : 0
    console.log(
      `  ${iso}: roads ${a.roadBoundaries} (${Object.entries(a.kinds).map(([k, v]) => `${k} ${v}`).join(', ')})` +
      ` | rail fallback ${fb}% of stamped (${a.railFullFallbackWays}/${a.railWays} ways full), steps ${a.railSteps}`,
    )
  }
  console.log('\n=== top steps ===')
  for (const e of top.slice(0, 15)) {
    console.log(`  ${e.db.toFixed(1)} dB ${e.kind.padEnd(14)} ${e.iso} #lat=${e.lat}&lng=${e.lon}&z=15`)
  }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch((err) => {
    console.error('Error:', err)
    process.exit(1)
  })
}
