//! Enrichment status — THE entry point answering "what does country X have and
//! where is it weak" per layer (roads / railways): provenance-tier shares per
//! class band / rail family, top sources by rows, speed + timetable coverage,
//! and an actionable GAPS list (src=0 concentrations and undeclared registry
//! fields). Read-only over the country's
//! h3r4 arrows; the hex set comes from prepared/h3r4-admin.bin (the engine's
//! own receiver-country approximation, so border hexes carry some neighbour
//! rows). Powers owner questions and contributor onboarding walkthroughs.
//!
//! The railways section also prints a rail CONTINUITY block (2026-07-15
//! railway continuity plan, Phase 2): pax/freight measured km, silent km,
//! engine-default km and seam/jump endpoint counts, split main vs branch —
//! row collection is SHARED with audit-enrichment-invariants.ts's R15/R16 via
//! lib/rail-endpoint-rows.ts (collectRailEndpointRows) so the two tools'
//! numbers can never drift. Unlike the rest of this file's hex-admin scoping,
//! the continuity block gates each ROW by its own CGAZ midpoint (lib/
//! country-polygon.ts) — a border hex's admin-iso approximation would
//! otherwise bleed a neighbour's track into this country's km totals.
//!
//! Usage:
//!   DATA_YEAR=2026 npx tsx pipeline/enrichment-status.ts --country CZ \
//!     [--layer roads|railways|all] [--json out.json]

import { existsSync, readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { tableFromIPC, type Table } from 'apache-arrow'
import { readAdminIso } from './lib/admin-iso.js'
import { SOURCES, SOURCES_BY_ID, PROVENANCE_RANK, SOURCE_ID_CZ_TIMETABLE_SILENT } from './lib/sources.js'
import { DATASETS, type Dataset } from './lib/enrichment-datasets.js'
import { DATA_YEAR as YEAR } from './lib/data-year.js'
import { collectRailEndpointRows, loadRailStopsIndex, RAIL_STOPS_UNAVAILABLE_WARNING } from './lib/rail-endpoint-rows.js'
import { findRailFlowJumps, findRailContinuityGaps } from './lib/rail-graph-metrics.js'
import { makeOwnershipGate, hasCountryPolygon } from './lib/country-polygon.js'

// ── Provenance tiers ──
// Tier index 0 none · 1 baseline (roads: R7 taper 9862, rail: timetable-silent
// 9863) · 2 heuristic · 3 national-proxy · 4 any measured tier — built once as
// a u16-indexed table so the per-row hot loop is a plain array read.
const TIER_MEASURED = PROVENANCE_RANK['global-measured']
const tierIdxBySourceId = new Uint8Array(0x10000)
for (const s of SOURCES) {
  if (s.id > 0) tierIdxBySourceId[s.id] = Math.min(PROVENANCE_RANK[s.provenance], TIER_MEASURED)
}
const ROAD_TIER_LABELS = ['none', 'taper', 'heuristic', 'proxy', 'measured'] as const
const RAIL_TIER_LABELS = ['none', 'silent', 'heuristic', 'proxy', 'measured'] as const

/** engine inputs.rs road_class codes (10/11/12 = motorway/trunk/primary links). */
const ROAD_CLASS_NAMES = [
  'motorway', 'trunk', 'primary', 'secondary', 'tertiary', 'residential',
  'living_street', 'service', 'track', 'unclassified', 'motorway_link', 'trunk_link', 'primary_link',
]
const isMajorRoadClass = (c: number): boolean => c <= 4 || (c >= 10 && c <= 12)
const RAIL_USAGE_NAMES = ['main', 'branch', 'industrial']
/** rail_type → family: 0 rail · 1/2 tram (incl. light_rail) · 3/4 narrow_gauge+funicular. */
const RAIL_FAMILY_NAMES = ['rail', 'tram', 'other']
const railFamilyIdx = (rt: number): number => (rt === 0 ? 0 : rt <= 2 ? 1 : 2)
const DATASETS_BY_ID = new Map<number, Dataset>(DATASETS.map((d) => [d.id, d]))

/** Per-record-batch typed-array views of numeric columns, in `names` order
 *  (null = column absent from this file's schema — caller defaults). Zero-copy
 *  for the single-batch files withArrowWrite/osm-extract produce. */
function* numericBatches(
  table: Table,
  names: readonly string[],
): Generator<{ rows: number; cols: (ArrayLike<number> | null)[] }> {
  for (const batch of table.batches) {
    yield {
      rows: batch.numRows,
      cols: names.map((n) => (batch.getChild(n)?.toArray() as ArrayLike<number> | undefined) ?? null),
    }
  }
}

// ── Roads scan ──

interface RoadBandAgg {
  segs: number
  meters: number
  tiers: Float64Array // [5] rows per tier index
  srcRows: Float64Array // [65536] rows per source_id
  speedTagged: number // speed_limit > 0
  speedTaper: number // untagged but speed_taper > 0; the rest = engine default cascade
}
const newRoadBand = (): RoadBandAgg =>
  ({ segs: 0, meters: 0, tiers: new Float64Array(5), srcRows: new Float64Array(0x10000), speedTagged: 0, speedTaper: 0 })

interface RoadsAgg {
  files: number
  majors: RoadBandAgg
  locals: RoadBandAgg
  classRows: Float64Array // [16] rows per road_class
  classSrc0: Float64Array // [16] source_id=0 rows per road_class
}

const ROAD_COLS = ['length_m', 'road_class', 'source_id', 'speed_limit', 'speed_taper'] as const

function scanRoadsHex(arrowPath: string, agg: RoadsAgg): void {
  agg.files++
  for (const { rows, cols: [lenM, cls, src, spd, tpr] } of numericBatches(tableFromIPC(readFileSync(arrowPath)), ROAD_COLS)) {
    for (let i = 0; i < rows; i++) {
      const c = cls ? cls[i] : 9
      const id = src ? src[i] : 0
      const band = isMajorRoadClass(c) ? agg.majors : agg.locals
      band.segs++
      band.meters += lenM ? lenM[i] : 0
      band.tiers[tierIdxBySourceId[id]]++
      band.srcRows[id]++
      if (spd && spd[i] > 0) band.speedTagged++
      else if (tpr && tpr[i] > 0) band.speedTaper++
      const ci = Math.min(c, 15)
      agg.classRows[ci]++
      if (id === 0) agg.classSrc0[ci]++
    }
  }
}

// ── Railways scan ──
// Service rows (yards/sidings, `service > 0`) can never be stamped — the
// writeRailTrains gate skips them — so every share below is over NON-SERVICE
// rows; the service share itself is reported once.

interface RailFamilyAgg { segs: number; meters: number; timetable: number; silent: number; src0: number }

interface RailAgg {
  files: number
  segs: number // all rows incl. service
  meters: number
  serviceSegs: number
  tiers: Float64Array // [5]
  srcRows: Float64Array // [65536]
  families: RailFamilyAgg[] // [3]
  usageRows: Float64Array // [4]
  usageSrc0: Float64Array // [4]
  divisorHist: Float64Array // [256] rows per parallel_divisor (absent column = 1)
}

const RAIL_COLS = ['length_m', 'rail_type', 'usage', 'service', 'parallel_divisor', 'source_id'] as const

function scanRailwaysHex(arrowPath: string, agg: RailAgg): void {
  agg.files++
  for (const { rows, cols: [lenM, rt, us, svc, div, src] } of numericBatches(tableFromIPC(readFileSync(arrowPath)), RAIL_COLS)) {
    for (let i = 0; i < rows; i++) {
      agg.segs++
      agg.meters += lenM ? lenM[i] : 0
      if (svc && svc[i] > 0) {
        agg.serviceSegs++
        continue
      }
      const id = src ? src[i] : 0
      agg.tiers[tierIdxBySourceId[id]]++
      agg.srcRows[id]++
      const f = agg.families[railFamilyIdx(rt ? rt[i] : 0)]
      f.segs++
      f.meters += lenM ? lenM[i] : 0
      if (id === SOURCE_ID_CZ_TIMETABLE_SILENT) f.silent++
      else if (id !== 0) f.timetable++
      else f.src0++
      const ui = Math.min(us ? us[i] : 0, 3)
      agg.usageRows[ui]++
      if (id === 0) agg.usageSrc0[ui]++
      agg.divisorHist[Math.min(div ? div[i] : 1, 255)]++
    }
  }
}

// ── Formatting ──

const fmtInt = (x: number): string => Math.round(x).toLocaleString('en-US')
const kmOf = (meters: number): string => fmtInt(meters / 1000)
const pct = (part: number, total: number): string => (total > 0 ? ((100 * part) / total).toFixed(1) + '%' : '—')
const tierShareLine = (tiers: Float64Array, total: number, labels: readonly string[]): string =>
  [4, 3, 2, 1, 0].map((t) => `${labels[t]} ${pct(tiers[t], total).padStart(6)}`).join(' · ')

function topSourceRows(srcRows: Float64Array, limit = 5): Array<{ id: number; rows: number }> {
  const found: Array<{ id: number; rows: number }> = []
  for (let id = 1; id < srcRows.length; id++) if (srcRows[id] > 0) found.push({ id, rows: srcRows[id] })
  found.sort((a, b) => b.rows - a.rows)
  return found.slice(0, limit)
}

const sourceLine = (id: number, rows: number, total: number): string => {
  const s = SOURCES_BY_ID.get(id)
  return `${fmtInt(rows).padStart(11)}  ${pct(rows, total).padStart(6)}  ${(s?.provenance ?? 'NOT IN REGISTRY').padEnd(20)}  ${String(s?.year ?? '—').padEnd(4)}  ${s?.name ?? `id ${id}`}`
}

/** Print `label` on the first line, matching indent on continuations. */
function printBlock(indent: string, label: string, lines: string[]): void {
  lines.forEach((l, i) => console.log(`${indent}${(i === 0 ? label : '').padEnd(label.length)}${l}`))
}

// ── Report sections ──

function printRoads(agg: RoadsAgg): void {
  console.log(`\nROADS  ${fmtInt(agg.majors.segs + agg.locals.segs)} seg · ${kmOf(agg.majors.meters + agg.locals.meters)} km`)
  for (const [label, b] of [['majors 0-4+links', agg.majors], ['locals 5-9', agg.locals]] as const) {
    console.log(`  ${label.padEnd(16)}  ${fmtInt(b.segs)} seg · ${kmOf(b.meters)} km`)
    console.log(`    provenance  ${tierShareLine(b.tiers, b.segs, ROAD_TIER_LABELS)}`)
    const dflt = b.segs - b.speedTagged - b.speedTaper
    console.log(`    speed       tagged ${pct(b.speedTagged, b.segs).padStart(6)} · taper ${pct(b.speedTaper, b.segs).padStart(6)} · default ${pct(dflt, b.segs).padStart(6)}`)
    const top = topSourceRows(b.srcRows)
    printBlock('    ', 'sources  ', top.length ? top.map((t) => sourceLine(t.id, t.rows, b.segs)) : ['every row src=0'])
  }
}

function printRailways(agg: RailAgg, registry: Dataset[]): void {
  const live = agg.segs - agg.serviceSegs
  console.log(`\nRAILWAYS  ${fmtInt(agg.segs)} seg · ${kmOf(agg.meters)} km`)
  console.log(`  service yards/sidings, never stamped  ${fmtInt(agg.serviceSegs)} seg · ${pct(agg.serviceSegs, agg.segs)} — shares below are over the rest`)
  console.log(`  provenance  ${tierShareLine(agg.tiers, live, RAIL_TIER_LABELS)}`)
  agg.families.forEach((f, fi) => {
    if (f.segs === 0) return
    console.log(
      `  ${RAIL_FAMILY_NAMES[fi].padEnd(5)} ${fmtInt(f.segs).padStart(10)} seg · ${kmOf(f.meters).padStart(7)} km · ` +
      `timetable ${pct(f.timetable, f.segs).padStart(6)} · silent ${pct(f.silent, f.segs).padStart(6)} · src=0 ${pct(f.src0, f.segs).padStart(6)}`,
    )
  })
  const divs: string[] = []
  for (let d = 0; d < 256; d++) if (agg.divisorHist[d] > 0) divs.push(`÷${d} ${pct(agg.divisorHist[d], live)}`)
  console.log(`  parallel_divisor  ${divs.join(' · ')}`)
  const top = topSourceRows(agg.srcRows)
  printBlock('  ', 'sources   ', top.length ? top.map((t) => sourceLine(t.id, t.rows, live)) : ['every row src=0'])
  const regLines = registry.map((d) => {
    const tc = (d as { timetableCoverage?: unknown }).timetableCoverage // future field (#26B)
    return `${String(d.id).padStart(5)}  ${d.key.padEnd(26)} ${(SOURCES_BY_ID.get(d.id)?.provenance ?? '?').padEnd(20)} ` +
      `measurement ${(d.measurement ?? '—').padEnd(8)} families ${(d.railFamilies?.join('+') ?? '—').padEnd(9)} timetableCoverage ${tc === undefined ? '—' : String(tc)}`
  })
  printBlock('  ', 'registry  ', regLines.length ? regLines : ['no railway sources declared for this country'])
}

// ── Rail continuity block (2026-07-15 railway continuity plan, Phase 2) ──
// Row collection is SHARED with audit-enrichment-invariants.ts's R15/R16 via
// lib/rail-endpoint-rows.ts so the two tools' numbers can never drift.

interface RailContinuityBand {
  paxMeasuredM: number
  frtMeasuredM: number
  bothMeasuredM: number
  effectiveDefaultFreightM: number
  silentM: number
  defaultM: number
  seamEndpoints: number
  jumpEndpoints: number
}
const newRailContinuityBand = (): RailContinuityBand => ({
  paxMeasuredM: 0, frtMeasuredM: 0, bothMeasuredM: 0, effectiveDefaultFreightM: 0,
  silentM: 0, defaultM: 0, seamEndpoints: 0, jumpEndpoints: 0,
})

interface RailContinuityResult {
  bands: { main: RailContinuityBand; branch: RailContinuityBand }
  stopsAvailable: boolean
  countryGateAvailable: boolean
}

/** usage 0 → main, 1 → branch, everything else (2 industrial, unknown) is
 *  outside the main/branch split this block reports. */
const railContinuityBandOf = (usage: number): 'main' | 'branch' | null =>
  usage === 0 ? 'main' : usage === 1 ? 'branch' : null

function computeRailContinuity(h3r4Dir: string, hexes: string[], country: string): RailContinuityResult {
  const { rows, ioErrors } = collectRailEndpointRows(h3r4Dir, hexes)
  if (ioErrors.length > 0) {
    console.error(`  rail-continuity: ${ioErrors.length} hex(es) with unreadable/damaged railways.arrow (informational — status is read-only, not a gate):`)
    for (const e of ioErrors.slice(0, 10)) console.error(`    ${e.hex}: ${e.error}`)
  }

  // Row-level CGAZ ownership gate — the hex set above comes from
  // h3r4-admin.bin (H3-centroid country approximation), which bleeds a
  // border hex's neighbour rows into this country's totals; gating each
  // row's own midpoint is the same fix R9/the auditor use for segments.
  const countryGateAvailable = hasCountryPolygon(country)
  let countryGate: ((lat: number, lon: number) => boolean) | null = null
  if (countryGateAvailable) {
    try {
      countryGate = makeOwnershipGate(country)
    } catch {
      countryGate = null
    }
  }
  const inCountry = (lat: number, lon: number): boolean => !countryGate || countryGate(lat, lon)

  const stopsIndex = loadRailStopsIndex(h3r4Dir)

  const bands = { main: newRailContinuityBand(), branch: newRailContinuityBand() }
  for (const r of rows) {
    const midLat = (r.startLat + r.endLat) / 2, midLon = (r.startLon + r.endLon) / 2
    if (!inCountry(midLat, midLon)) continue
    const band = railContinuityBandOf(r.usage)
    if (!band) continue
    const b = bands[band]
    if (r.sourceId === 0) { b.defaultM += r.lengthM; continue }
    if (SOURCES_BY_ID.get(r.sourceId)?.key.endsWith('-timetable-silent')) { b.silentM += r.lengthM; continue }
    if (r.pax > 0) b.paxMeasuredM += r.lengthM
    if (r.frt > 0) b.frtMeasuredM += r.lengthM
    if (r.pax > 0 && r.frt > 0) b.bothMeasuredM += r.lengthM
    // source_id>0, frt==0 — the engine's per-column zero-defaulting re-adds
    // class-default freight; a pax-only GTFS country must show up here, not
    // just look "measured" via pax_measured_km.
    if (r.frt === 0) b.effectiveDefaultFreightM += r.lengthM
  }

  // A violation's two sides can straddle main/branch (a branch spur off a
  // main line) — attributed to the LOWER usage code (main wins a mixed pair,
  // same "the more important line dominates" call the auditor's own
  // higher-effective-total pick makes for hex/row/osm_id).
  const rowByKey = new Map(rows.map((r) => [r.key, r]))
  const bandOfPair = (aKey: string, bKey: string): 'main' | 'branch' | null => {
    const a = rowByKey.get(aKey), b = rowByKey.get(bKey)
    return railContinuityBandOf(Math.min(a?.usage ?? 0, b?.usage ?? 0))
  }
  for (const v of findRailFlowJumps(rows, stopsIndex)) {
    if (!inCountry(v.endpointLat, v.endpointLon)) continue
    const band = bandOfPair(v.aKey, v.bKey)
    if (band) bands[band].jumpEndpoints++
  }
  for (const v of findRailContinuityGaps(rows, stopsIndex)) {
    if (!inCountry(v.endpointLat, v.endpointLon)) continue
    const band = bandOfPair(v.aKey, v.bKey)
    if (band) bands[band].seamEndpoints++
  }

  return { bands, stopsAvailable: stopsIndex !== null, countryGateAvailable }
}

function printRailContinuity(result: RailContinuityResult): void {
  console.log(
    `\n  continuity (km${result.countryGateAvailable ? ', row-level CGAZ match' : ' — NO CGAZ polygon for this country, hex-admin only'})`,
  )
  if (!result.stopsAvailable) console.log(`    ${RAIL_STOPS_UNAVAILABLE_WARNING}`)
  for (const band of ['main', 'branch'] as const) {
    const b = result.bands[band]
    console.log(
      `    ${band.padEnd(7)} pax-measured ${kmOf(b.paxMeasuredM).padStart(6)} km · frt-measured ${kmOf(b.frtMeasuredM).padStart(6)} km · ` +
      `both ${kmOf(b.bothMeasuredM).padStart(6)} km · default-freight ${kmOf(b.effectiveDefaultFreightM).padStart(6)} km · ` +
      `silent ${kmOf(b.silentM).padStart(6)} km · default ${kmOf(b.defaultM).padStart(6)} km · ` +
      `seam_endpoints ${b.seamEndpoints} · jump_endpoints ${b.jumpEndpoints}`,
    )
  }
}

const railContinuityBandJson = (b: RailContinuityBand) => ({
  pax_measured_km: Math.round(b.paxMeasuredM / 1000),
  frt_measured_km: Math.round(b.frtMeasuredM / 1000),
  both_measured_km: Math.round(b.bothMeasuredM / 1000),
  effective_default_freight_km: Math.round(b.effectiveDefaultFreightM / 1000),
  silent_km: Math.round(b.silentM / 1000),
  default_km: Math.round(b.defaultM / 1000),
  seam_endpoints: b.seamEndpoints,
  jump_endpoints: b.jumpEndpoints,
})

function printGaps(roads: RoadsAgg | null, rail: RailAgg | null, undeclared: string[]): void {
  console.log('\nGAPS')
  if (roads) {
    const worst: Array<{ ci: number; rows: number; src0: number }> = []
    for (let ci = 0; ci < 16; ci++) if (roads.classSrc0[ci] > 0) worst.push({ ci, rows: roads.classRows[ci], src0: roads.classSrc0[ci] })
    worst.sort((a, b) => b.src0 - a.src0)
    printBlock('  ', 'roads src=0   ', worst.slice(0, 5).map((w) =>
      `${(ROAD_CLASS_NAMES[w.ci] ?? `class ${w.ci}`).padEnd(13)} ${pct(w.src0, w.rows).padStart(6)} of ${fmtInt(w.rows)} seg`))
    if (worst.length === 0) console.log('  roads src=0   none — every segment carries a source')
  }
  if (rail) {
    const parts: string[] = []
    for (let ui = 0; ui < 4; ui++) {
      if (rail.usageRows[ui] === 0) continue
      parts.push(`${RAIL_USAGE_NAMES[ui] ?? `usage ${ui}`} ${pct(rail.usageSrc0[ui], rail.usageRows[ui])} of ${fmtInt(rail.usageRows[ui])}`)
    }
    console.log(`  rail src=0    ${parts.join(' · ')} seg`)
  }
  printBlock('  ', 'undeclared    ', undeclared.length ? undeclared : ['none — every country source declares roadCoverage/railFamilies'])
}

/** Registry entries lacking the declaration the invariant scanner keys on:
 *  roads without `roadCoverage`, railways without `railFamilies` — over the
 *  union of country-prefixed entries and ids actually stamped in this
 *  country's rows (plus stamped ids missing from the registry entirely). */
function undeclaredSourceLines(roads: RoadsAgg | null, rail: RailAgg | null, ccPrefix: string): string[] {
  const out: string[] = []
  const check = (layer: 'roads' | 'railways', rowsOf: (id: number) => number): void => {
    const ids = new Set<number>()
    for (let id = 1; id < 0x10000; id++) if (rowsOf(id) > 0) ids.add(id)
    for (const d of DATASETS) if (d.layer === layer && d.key.startsWith(ccPrefix)) ids.add(d.id)
    for (const id of [...ids].sort((a, b) => a - b)) {
      const d = DATASETS_BY_ID.get(id)
      if (!d) out.push(`id ${id}: ${fmtInt(rowsOf(id))} ${layer} rows but NOT IN REGISTRY`)
      else if (layer === 'roads' && d.layer === 'roads' && !d.roadCoverage) out.push(`${d.key}: roadCoverage undeclared`)
      else if (layer === 'railways' && d.layer === 'railways' && !d.railFamilies) out.push(`${d.key}: railFamilies undeclared`)
    }
  }
  if (roads) check('roads', (id) => roads.majors.srcRows[id] + roads.locals.srcRows[id])
  if (rail) check('railways', (id) => rail.srcRows[id])
  return out
}

// ── JSON mirror ──

const tiersJson = (tiers: Float64Array, labels: readonly string[]): Record<string, number> =>
  Object.fromEntries(labels.map((l, t) => [l, tiers[t]]))
const sourcesJson = (srcRows: Float64Array): Array<{ id: number; key: string; rows: number }> =>
  topSourceRows(srcRows, 0x10000).map((t) => ({ id: t.id, key: SOURCES_BY_ID.get(t.id)?.key ?? 'NOT IN REGISTRY', rows: t.rows }))
const roadBandJson = (b: RoadBandAgg): object => ({
  segs: b.segs,
  km: Math.round(b.meters / 1000),
  tiers: tiersJson(b.tiers, ROAD_TIER_LABELS),
  speed: { tagged: b.speedTagged, taper: b.speedTaper, default: b.segs - b.speedTagged - b.speedTaper },
  sources: sourcesJson(b.srcRows),
})

// ── Main ──

function main(): void {
  const arg = (flag: string): string => {
    const i = process.argv.indexOf(flag)
    return i >= 0 ? (process.argv[i + 1] ?? '') : ''
  }
  const country = arg('--country').toUpperCase()
  const layer = arg('--layer') || 'all'
  const jsonPath = arg('--json')
  if (!/^[A-Z]{2}$/.test(country) || !['roads', 'railways', 'all'].includes(layer)) {
    console.error('Usage: DATA_YEAR=2026 npx tsx pipeline/enrichment-status.ts --country CZ [--layer roads|railways|all] [--json out.json]')
    process.exit(1)
  }
  const t0 = Date.now()
  const h3r4Dir = resolve(import.meta.dirname, `../data/prepared/${YEAR}/h3r4`)
  const adminIso = readAdminIso(resolve(import.meta.dirname, '../data/prepared/h3r4-admin.bin'))
  const hexes = [...adminIso].filter(([, iso]) => iso === country).map(([hex]) => hex).sort()
  if (hexes.length === 0) {
    console.error(`no hexes for ${country} in h3r4-admin.bin (${adminIso.size} total)`)
    process.exit(1)
  }

  const roads: RoadsAgg | null = layer !== 'railways'
    ? { files: 0, majors: newRoadBand(), locals: newRoadBand(), classRows: new Float64Array(16), classSrc0: new Float64Array(16) }
    : null
  const rail: RailAgg | null = layer !== 'roads'
    ? {
        files: 0, segs: 0, meters: 0, serviceSegs: 0, tiers: new Float64Array(5), srcRows: new Float64Array(0x10000),
        families: RAIL_FAMILY_NAMES.map(() => ({ segs: 0, meters: 0, timetable: 0, silent: 0, src0: 0 })),
        usageRows: new Float64Array(4), usageSrc0: new Float64Array(4), divisorHist: new Float64Array(256),
      }
    : null
  for (const hex of hexes) {
    const roadsPath = resolve(h3r4Dir, hex, 'roads.arrow')
    if (roads && existsSync(roadsPath)) scanRoadsHex(roadsPath, roads)
    const railPath = resolve(h3r4Dir, hex, 'railways.arrow')
    if (rail && existsSync(railPath)) scanRailwaysHex(railPath, rail)
  }

  const ccPrefix = country.toLowerCase() + '-'
  const railRegistry = rail
    ? DATASETS.filter((d) => d.layer === 'railways' && (d.key.startsWith(ccPrefix) || rail.srcRows[d.id] > 0))
        .sort((a, b) => a.id - b.id)
    : []
  const undeclared = undeclaredSourceLines(roads, rail, ccPrefix)
  const railContinuity = rail ? computeRailContinuity(h3r4Dir, hexes, country) : null

  console.log(`ENRICHMENT STATUS  ${country} · year ${YEAR} · ${hexes.length} hexes`)
  if (roads) printRoads(roads)
  if (rail) printRailways(rail, railRegistry)
  if (railContinuity) printRailContinuity(railContinuity)
  printGaps(roads, rail, undeclared)
  const elapsedS = (Date.now() - t0) / 1000
  console.log(`\nscan  roads ${roads ? roads.files : '—'} files · railways ${rail ? rail.files : '—'} files · ${elapsedS.toFixed(1)} s · ${h3r4Dir}`)

  if (jsonPath) {
    writeFileSync(jsonPath, JSON.stringify({
      country,
      dataYear: YEAR,
      hexes: hexes.length,
      elapsedS,
      roads: roads && {
        segs: roads.majors.segs + roads.locals.segs,
        km: Math.round((roads.majors.meters + roads.locals.meters) / 1000),
        majors: roadBandJson(roads.majors),
        locals: roadBandJson(roads.locals),
        src0ByClass: [...roads.classRows].map((rows, ci) => ({ class: ROAD_CLASS_NAMES[ci] ?? `class ${ci}`, rows, src0: roads.classSrc0[ci] }))
          .filter((e) => e.rows > 0),
      },
      railways: rail && {
        segs: rail.segs,
        km: Math.round(rail.meters / 1000),
        serviceSegs: rail.serviceSegs,
        tiers: tiersJson(rail.tiers, RAIL_TIER_LABELS),
        families: rail.families
          .map((f, fi) => ({ family: RAIL_FAMILY_NAMES[fi], segs: f.segs, km: Math.round(f.meters / 1000), timetable: f.timetable, silent: f.silent, src0: f.src0 }))
          .filter((f) => f.segs > 0),
        src0ByUsage: [...rail.usageRows].map((rows, ui) => ({ usage: RAIL_USAGE_NAMES[ui] ?? `usage ${ui}`, rows, src0: rail.usageSrc0[ui] }))
          .filter((e) => e.rows > 0),
        parallelDivisor: Object.fromEntries([...rail.divisorHist].map((n, d) => [d, n]).filter(([, n]) => n > 0)),
        sources: sourcesJson(rail.srcRows),
        registry: railRegistry.map((d) => ({
          id: d.id,
          key: d.key,
          provenance: SOURCES_BY_ID.get(d.id)?.provenance ?? null,
          measurement: d.measurement ?? null,
          railFamilies: d.railFamilies ?? null,
          timetableCoverage: (d as { timetableCoverage?: unknown }).timetableCoverage ?? null,
        })),
        continuity: railContinuity && {
          countryGateAvailable: railContinuity.countryGateAvailable,
          stopsSidecarAvailable: railContinuity.stopsAvailable,
          main: railContinuityBandJson(railContinuity.bands.main),
          branch: railContinuityBandJson(railContinuity.bands.branch),
        },
      },
      gaps: { undeclared },
    }, null, 1))
    console.log(`json  ${jsonPath}`)
  }
}

if (import.meta.url === `file://${process.argv[1]}`) main()
