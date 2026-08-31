/**
 * READ-ONLY enrichment-invariant scanner (A3/C2/R5 of the 2026-06 audit wave) —
 * the acceptance gate after every reset + re-enrich, and a CI tripwire:
 * exits non-zero when any invariant is violated.
 *
 * Capability-aware per the /gg W5 verdict: instead of hardcoding "class>=5 +
 * national source = violation", each rule reads the source's declared
 * capability (`roadCoverage` / `railFamilies`) from `lib/enrichment-datasets.ts`
 * and skips rows whose source declares none — no guessing about undeclared feeds.
 *
 * Rules (roads / railways / industrial layers under prepared/{year}/h3r4):
 *   R0  unknown-source  — source_id stamped on a row but absent from DATASETS
 *   R1  road-coverage   — row's road_class outside its source's declared roadCoverage
 *   R2  moto-scramble   — aadt_moto > max(500, 0.5*aadt_light): cars landed in the
 *                         moto column (the PL provincial XLS column-shift bug shape)
 *   R3  tram-overcount  — tram/light_rail row with trains_passenger > 400 from a
 *                         source whose railFamilies excludes 'tram' (family defaults
 *                         are <=250, so 400 clears them with margin)
 *   R4  wind-as-thermal — industrial row whose name matches the wind keyword set
 *                         AND nace_4digit in {3500,3511,3512} (wind is source_type=10,
 *                         never a power NACE — the pre-5f1b969f bug shape)
 *   R5  flow-jump       — same-ref same-source AADT jumps >3x at a shared endpoint
 *                         with no junction (physically impossible flow change)
 *   R6  value-range     — negative / non-finite AADT or train counts, or total AADT
 *                         above the physically-impossible ceiling (500k; the busiest
 *                         measured road, Toronto Hwy 401, peaks ~420k AADT)
 *   R7  zero-write      — row stamped by a national/city/continental source
 *                         (priority >= 70) with ALL AADT columns zero — the IT/SA
 *                         "wrote zeros" bug shape (writeRoadAadt now throws on it,
 *                         but pre-fix data can still carry it)
 *   R8  class-range     — road_class outside engine inputs.rs codes 0..12
 *   R9  country-bleed   — row stamped by a country-bound source but the WHOLE
 *                         segment lies outside that country (CGAZ polygon test;
 *                         midpoint pre-check, both endpoints confirm — a segment
 *                         genuinely straddling the border is NOT flagged).
 *                         The PL GPR → CZ bleed shape (see lib/country-polygon.ts).
 *   R10 layer-mismatch  — source's declared `layer` is neither the scanned layer
 *                         nor 'any' (wrong dataset-id constant in an enricher)
 *   R11 unknown-country — registry-level: a dataset key carries a country prefix
 *                         that CGAZ/ISO-3166 cannot resolve (checked once at start)
 *   R14 industrial-orphan — a shared-id (national-mix 330) industrial stamp
 *       whose centroid lies in NO ownership area (CGAZ country ∪ declared
 *       non-CGAZ bboxes): unreachable by every national pass since #31 —
 *       stale by construction. Healed by heal-industrial-orphans.ts.
 *   R12 buildings-contract — settlement v2 (Convention-B): a stamped
 *                         buildings.arrow must carry exactly `buildings_v2` +
 *                         the opening_hours_frac column; an UNstamped file must
 *                         not contain v2 POI-join type ids 10-13 (that shape =
 *                         an enricher rewrite dropped the schema metadata, and
 *                         the heatmap loader fail-loud rejects the file)
 *   R15 rail-flow-jump  — cross-source EFFECTIVE rail traffic (engine
 *                         zero-default + parallel_divisor mirror, see
 *                         lib/rail-graph.ts::effectiveRailTraffic) jumps >3x
 *                         at a degree-2 heavy-rail non-service node with no
 *                         junction/stop to explain it, once effective max
 *                         >=20 trains/day — the trať 200 shape (110->9863,
 *                         raw 16+0 vs 2+1 hides the floor, effective 36 vs 3
 *                         clears it)
 *   R16 rail-continuity-gap (rail) — same candidate node, one side measured
 *                         (source_id>0) and the other an unmeasured engine-
 *                         default row (source_id==0), EFFECTIVE totals >3x
 *                         apart — the rail twin of R13, but comparing what the
 *                         engine actually renders, not raw stamped ints
 *
 *   R15/R16 run on a SCOPE-WIDE endpoint index — never per-hex like R5/R13
 *   accept for roads — because the bug class they catch (banding at a wrong
 *   section boundary) sits exactly on H3 borders: railways.arrow rows are
 *   assigned to hexes by midpoint, so a per-hex adjacency scan would miss a
 *   seam that straddles two hexes. See collectRailEndpointRows
 *   (lib/rail-endpoint-rows.ts) and findRailFlowJumps/findRailContinuityGaps
 *   (lib/rail-graph-metrics.ts) — both fail-closed on missing end_lat/
 *   end_lon/usage/service columns (EXTRACT-core for this rule) and degrade to
 *   junction-only exemption (no rail-stops sidecar yet) with a loud warning,
 *   never a silent narrower scan.
 *
 * Usage:
 *   DATA_YEAR=2025 npx tsx pipeline/audit-enrichment-invariants.ts \
 *     --bbox S,W,N,E [--bbox S,W,N,E …] [--sample N] [--fix-hint] \
 *     [--ndjson <path>] [--summary-json <path>] [--lenient-io]
 *
 * `--bbox` is REPEATABLE: country scopes pass their per-polygon padded bbox
 * list (never the union envelope); hexes are deduped across the boxes so an
 * overlap never double-counts a violation. Single-bbox callers are unchanged.
 * `--sample N` checks at most ~N evenly-strided rows per hex per layer
 * (R5 always scans full hexes — adjacency needs every segment).
 * `--fix-hint` appends a per-rule remediation hint for every rule that fired.
 * `--ndjson <path>` writes one JSON line per violation with the stable
 * fingerprint fields {rule, source, hex, osm_id, row, lat5, lon5} (+detail) —
 * the chain gate diffs these as a multiset against its baseline.
 * `--summary-json <path>` writes machine totals {total, counts: rule|source→n,
 * ioErrors} — the gate cross-checks NDJSON line count against `total`.
 * Operational I/O damage (unreadable arrow file, missing EXTRACT-core column)
 * is FATAL BY DEFAULT — exit 3, distinct from violations so a crash/corrupt
 * hex can never be baselined as "resolved" (#31.3: fail-open let a damaged hex
 * scan as CLEAN). `--lenient-io` downgrades it to the stderr warnings only —
 * for exploratory scans over known-damaged trees, NEVER for a gate. Chain-only
 * columns (aadt_*, trains_*, nace_4digit) legitimately don't exist on
 * never-enriched hexes and stay a skip, not an IO error.
 * Exits 0 clean, 1 on violations (prints a table, first 50 rows), 2 on usage,
 * 3 on operational I/O errors (unless --lenient-io).
 */

import { readFileSync, writeFileSync } from 'node:fs'
import { resolve, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { tableFromIPC, type Table, type Vector } from 'apache-arrow'
import { DATASETS } from './lib/enrichment-datasets.js'
import { isMeasured } from './lib/sources.js'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import { makeOwnershipGate, segmentWhollyOutside, makeAnyCountryGate } from './lib/country-polygon.js'
import { SOURCE_ID_GLOBAL_INDUSTRIAL_NATIONAL_MIX } from './lib/source-ids.generated.js'
import { MAX_BUILDING_TYPE, V2_SPECIFIC_TYPE_MIN } from './lib/buildings-arrow.js'
import { DATA_YEAR as YEAR } from './lib/data-year.js'
import { collectRailEndpointRows, loadRailStopsIndex, RAIL_STOPS_UNAVAILABLE_WARNING } from './lib/rail-endpoint-rows.js'
import { findRailFlowJumps, findRailContinuityGaps } from './lib/rail-graph-metrics.js'
import type { RailContinuityViolation } from './lib/rail-graph.js'

const __dirname = dirname(fileURLToPath(import.meta.url))
const H3R4_DIR = resolve(__dirname, '..', 'data', 'prepared', YEAR, 'h3r4')

// Wind keyword set — copied from NAME_RULES 'wind (skip)' in
// enrich-industrial-name-heuristic.ts (not importable: that file runs main() on
// import). Substring match on the lowercased name, same as its matchName().
const WIND_NAME_KEYWORDS = [
  'wind farm', 'wind park', 'windpark', 'éolien', 'vindpark', 'vindkraft',
  'parque eólico', 'větrná', 'wiatrowy',
]
const POWER_NACE = new Set([3500, 3511, 3512])

// R6 ceiling: the busiest measured road on Earth (Toronto Hwy 401) peaks ~420k
// AADT — anything above 500k is a loader unit/parse error, not traffic.
const AADT_IMPOSSIBLE = 500_000

// ── args ─────────────────────────────────────────────────────────────────────

function arg(name: string): string | null {
  const i = process.argv.indexOf(name)
  return i >= 0 ? process.argv[i + 1] ?? null : null
}

function argAll(name: string): string[] {
  const out: string[] = []
  for (let i = 0; i < process.argv.length - 1; i++) {
    if (process.argv[i] === name) out.push(process.argv[i + 1])
  }
  return out
}

const bboxArgsRaw = argAll('--bbox')
if (bboxArgsRaw.length === 0) {
  console.error(
    'Usage: npx tsx pipeline/audit-enrichment-invariants.ts --bbox S,W,N,E [--bbox …] [--sample N] [--fix-hint] [--ndjson <path>] [--summary-json <path>] [--lenient-io]',
  )
  process.exit(2)
}
const BBOXES: Array<[number, number, number, number]> = [] // each [minLat, minLon, maxLat, maxLon]
for (const bboxArg of bboxArgsRaw) {
  const [s, w, n, e] = bboxArg.split(',').map(Number)
  if (![s, w, n, e].every(Number.isFinite) || s >= n || w >= e) {
    console.error(`Invalid --bbox '${bboxArg}' — expected S,W,N,E with S<N and W<E`)
    process.exit(2)
  }
  BBOXES.push([s, w, n, e])
}
const SAMPLE = arg('--sample') ? Math.max(1, parseInt(arg('--sample')!, 10)) : Infinity
const FIX_HINT = process.argv.includes('--fix-hint')
const NDJSON_PATH = arg('--ndjson')
const SUMMARY_PATH = arg('--summary-json')
const LENIENT_IO = process.argv.includes('--lenient-io')

// ── operational I/O errors (distinct from invariant violations) ──────────────

const ioErrors: Array<{ hex: string; layer: string; error: string }> = []

/** Unreadable file / broken extract-core schema. Fatal exit 3 by DEFAULT (a
 *  silently skipped hex is how corruption used to pass as CLEAN); --lenient-io
 *  keeps only the stderr warning for exploratory scans over damaged trees. */
function reportIoError(hex: string, layer: string, error: string): void {
  ioErrors.push({ hex, layer, error })
  console.error(`IO ERROR ${layer} ${hex}: ${error}`)
}

/** EXTRACT-core columns exist on every hex the extractor wrote — absence is
 *  file damage, not "not yet enriched". Chain-only columns are NOT listed here
 *  on purpose (see module doc). */
function requireCoreColumns(t: Table, hex: string, layerFile: string, names: string[]): boolean {
  const missing = names.filter((c) => !t.getChild(c))
  if (missing.length === 0) return true
  reportIoError(hex, layerFile, `missing extract-core column(s): ${missing.join(', ')}`)
  return false
}

// ── capability registry ──────────────────────────────────────────────────────

const ROAD_COVERAGE = new Map<number, ReadonlySet<number>>()
const RAIL_FAMILIES = new Map<number, ReadonlySet<string>>()
const KEY_BY_ID = new Map<number, string>()
const LAYER_BY_ID = new Map<number, string>()
const PRIORITY_BY_ID = new Map<number, number>()
const HIGH_MOTO = new Set<number>()
for (const d of DATASETS) {
  KEY_BY_ID.set(d.id, d.key)
  LAYER_BY_ID.set(d.id, d.layer)
  PRIORITY_BY_ID.set(d.id, d.priority)
  if (d.roadCoverage) ROAD_COVERAGE.set(d.id, new Set(d.roadCoverage))
  if (d.railFamilies) RAIL_FAMILIES.set(d.id, new Set(d.railFamilies))
  if (d.highMoto) HIGH_MOTO.add(d.id)
}

// ── source → country (for R9/R11) ────────────────────────────────────────────

// City sources don't carry an ISO prefix in the key; map them explicitly.
const CITY_COUNTRY: Record<string, string> = {
  'city-praha-tsk': 'CZ',
  'city-brno-detectors': 'CZ',
  'city-wien-dauerzaehlstellen': 'AT',
}
// Two-letter key prefixes that are NOT ISO country codes (continental scope).
const NON_COUNTRY_PREFIXES = new Set(['eu'])

/** ISO alpha-2 of the country a source is bound to, or null for
 *  global/continental/heuristic sources (which may legitimately stamp anywhere). */
function countryOfKey(key: string): string | null {
  if (CITY_COUNTRY[key]) return CITY_COUNTRY[key]
  const m = /^([a-z]{2})-/.exec(key)
  if (!m || NON_COUNTRY_PREFIXES.has(m[1])) return null
  return m[1].toUpperCase()
}

const COUNTRY_BY_ID = new Map<number, string>()
for (const d of DATASETS) {
  const cc = countryOfKey(d.key)
  if (cc) COUNTRY_BY_ID.set(d.id, cc)
}

// ── violation collection ─────────────────────────────────────────────────────

interface Violation {
  rule: string
  hex: string
  row: number
  osmId: number | null
  source: string
  lat: number
  lon: number
  detail: string
}
const violations: Violation[] = []
const byRule = new Map<string, number>()
const byRuleSource = new Map<string, Map<string, number>>()

function report(
  rule: string, hex: string, row: number, osmId: number | null,
  sourceId: number, lat: number, lon: number, detail: string,
  // R15/R16 override the per-source label with a canonical PAIR + severity
  // bucket (/gg item 13): the pair must be IN the fingerprint fields — this
  // `source` value feeds --ndjson's `source` field, part of fingerprintOf in
  // chain/gate.ts — not only in `detail`, since the gate diffs fingerprints
  // (rule|source|hex|osm_id|row|lat5|lon5) and never looks at detail.
  sourceLabelOverride?: string,
): void {
  byRule.set(rule, (byRule.get(rule) ?? 0) + 1)
  const source = sourceLabelOverride ?? (KEY_BY_ID.get(sourceId) ?? `id ${sourceId}`)
  const perSource = byRuleSource.get(rule) ?? new Map<string, number>()
  perSource.set(source, (perSource.get(source) ?? 0) + 1)
  byRuleSource.set(rule, perSource)
  violations.push({ rule, hex, row, osmId, source, lat, lon, detail })
}

// ── R11 + country gates (built once; reused per row for R9) ──────────────────

// One CGAZ polygon gate per country that any registered source is bound to.
// Building all of them up front IS the R11 registry check — an unresolvable
// country prefix fails here even when no row in the bbox carries that source.
const GATES = new Map<string, ((lat: number, lon: number) => boolean) | null>()
for (const cc of new Set(COUNTRY_BY_ID.values())) {
  try {
    GATES.set(cc, makeOwnershipGate(cc)) // country + territory extensions (MA∪EH) — mirrors the writer/heal union
  } catch (err) {
    GATES.set(cc, null)
    const keys = DATASETS.filter(d => COUNTRY_BY_ID.get(d.id) === cc).map(d => d.key).join(', ')
    report('R11 unknown-country', '—', -1, null, -1, 0, 0,
      `country '${cc}' unresolvable (${err instanceof Error ? err.message : err}) — sources: ${keys}`)
  }
}

/** R9: whole feature outside the source's country? Midpoint first (cheap
 *  majority case), both endpoints confirm — border-straddling segments pass. */
function checkCountryBleed(
  hex: string, i: number, osmId: number | null, id: number,
  midLat: number, midLon: number, aLat: number, aLon: number, bLat: number, bLon: number,
): void {
  const cc = COUNTRY_BY_ID.get(id)
  if (!cc) return
  const gate = GATES.get(cc)
  if (!gate || !segmentWhollyOutside(gate, midLat, midLon, aLat, aLon, bLat, bLon)) return
  report('R9 country-bleed', hex, i, osmId, id, midLat, midLon,
    `source is ${cc}-bound but segment lies outside ${cc} (start+mid+end all foreign)`)
}

// ── scan plumbing ────────────────────────────────────────────────────────────

/** Evenly-strided row indices: every row when n<=SAMPLE, else ~SAMPLE of them. */
function* sampleRows(numRows: number): Generator<number> {
  const stride = numRows > SAMPLE ? Math.ceil(numRows / SAMPLE) : 1
  for (let i = 0; i < numRows; i += stride) yield i
}

/** Hexes covered by ANY of the --bbox instances, deduped: country scopes pass
 *  a per-polygon padded bbox list and overlapping padded boxes must not
 *  double-count a hex's violations in the gate's fingerprint multiset. */
function hexesInScope(layerFile: string): string[] {
  const seen = new Set<string>()
  for (const b of BBOXES) for (const h of iterateCountryHexes(H3R4_DIR, b, layerFile)) seen.add(h)
  return [...seen].sort()
}

function scanLayer(
  layerFile: string,
  checkTable: (t: Table, hex: string) => number,
): { hexes: number; rows: number; ms: number } {
  const t0 = Date.now()
  const hexes = hexesInScope(layerFile)
  let rows = 0
  for (const hex of hexes) {
    let table: Table
    try {
      table = tableFromIPC(readFileSync(resolve(H3R4_DIR, hex, layerFile)))
    } catch (err) {
      reportIoError(hex, layerFile, `unreadable arrow: ${err instanceof Error ? err.message : err}`)
      continue
    }
    rows += checkTable(table, hex)
  }
  return { hexes: hexes.length, rows, ms: Date.now() - t0 }
}

/** Segment midpoint (lat, lon) for row `i`, falling back to the start vertex
 *  when the end column is absent — used only to label a violation's location. */
function segMid(
  sLat: Vector, sLon: Vector, eLat: Vector | null, eLon: Vector | null, i: number,
): [number, number] {
  const s0 = sLat.get(i) as number, s1 = sLon.get(i) as number
  const e0 = (eLat?.get(i) as number) ?? s0, e1 = (eLon?.get(i) as number) ?? s1
  return [(s0 + e0) / 2, (s1 + e1) / 2]
}

/** Shared per-row registry checks (R0 + R10) for any layer. */
function checkSourceRegistry(
  layer: string, hex: string, i: number, osmId: number | null, id: number, lat: number, lon: number,
): void {
  if (id === 0) return
  if (!KEY_BY_ID.has(id)) {
    report('R0 unknown-source', hex, i, osmId, id, lat, lon, `source_id=${id} not in DATASETS registry`)
    return
  }
  const declared = LAYER_BY_ID.get(id)
  if (declared !== layer && declared !== 'any') {
    report('R10 layer-mismatch', hex, i, osmId, id, lat, lon,
      `source declares layer '${declared}' but stamped a ${layer} row`)
  }
}

// ── R0/R1/R2/R5/R6/R7/R8/R9/R10: roads ───────────────────────────────────────

const roads = scanLayer('roads.arrow', (t, hex) => {
  const cls = t.getChild('road_class')
  const light = t.getChild('aadt_light')
  const moto = t.getChild('aadt_moto')
  const src = t.getChild('source_id')
  const osm = t.getChild('osm_id')
  const sLat = t.getChild('start_lat'), sLon = t.getChild('start_lon')
  const eLat = t.getChild('end_lat'), eLon = t.getChild('end_lon')
  const med = t.getChild('aadt_medium'), hev = t.getChild('aadt_heavy')
  if (!requireCoreColumns(t, hex, 'roads.arrow', ['road_class', 'source_id', 'start_lat', 'start_lon'])) return 0
  // aadt_* are chain-only: absent = hex never road-enriched → skip (not an IO error).
  if (!cls || !light || !moto || !src || !sLat || !sLon) return 0
  let checked = 0
  for (const i of sampleRows(t.numRows)) {
    checked++
    const id = (src.get(i) as number) ?? 0
    const c = (cls.get(i) as number) ?? 0
    const li = (light.get(i) as number) ?? 0
    const mo = (moto.get(i) as number) ?? 0
    const me = (med?.get(i) as number) ?? 0
    const he = (hev?.get(i) as number) ?? 0
    const oid = osm ? Number(osm.get(i)) : null
    const [la, lo] = segMid(sLat, sLon, eLat, eLon, i)
    checkSourceRegistry('roads', hex, i, oid, id, la, lo)
    const coverage = ROAD_COVERAGE.get(id)
    if (coverage && !coverage.has(c)) {
      report('R1 road-coverage', hex, i, oid, id, la, lo, `road_class=${c} outside declared coverage`)
    }
    if (li > 0 && mo > Math.max(500, 0.5 * li) && !HIGH_MOTO.has(id)) {
      report('R2 moto-scramble', hex, i, oid, id, la, lo, `aadt_moto=${mo} vs aadt_light=${li}`)
    }
    const badCol =
      !Number.isFinite(li) || li < 0 ? `aadt_light=${li}` :
      !Number.isFinite(me) || me < 0 ? `aadt_medium=${me}` :
      !Number.isFinite(he) || he < 0 ? `aadt_heavy=${he}` :
      !Number.isFinite(mo) || mo < 0 ? `aadt_moto=${mo}` : null
    const total = li + me + he + mo
    if (badCol) {
      report('R6 value-range', hex, i, oid, id, la, lo, `${badCol} (negative or non-finite)`)
    } else if (total > AADT_IMPOSSIBLE) {
      report('R6 value-range', hex, i, oid, id, la, lo, `total AADT ${total} > ${AADT_IMPOSSIBLE} (physically impossible)`)
    }
    if (id > 0 && total === 0 && isMeasured(id)) {
      // isMeasured (not priority>=70) so the rule matches the writeRoadAadt
      // guard + heal-road-zero-write EXACTLY (/gg #33 Codex): priority>=70
      // wrongly flagged national-PROXY zeros (which the writer allows) and
      // missed global-measured zeros (which the writer rejects).
      report('R7 zero-write', hex, i, oid, id, la, lo, `stamped by measured source but all AADT columns are 0`)
    }
    if (c > 12) {
      report('R8 class-range', hex, i, oid, id, la, lo, `road_class=${c} outside engine codes 0..12`)
    }
    checkCountryBleed(hex, i, oid, id, la, lo,
      sLat.get(i) as number, sLon.get(i) as number,
      (eLat?.get(i) as number) ?? (sLat.get(i) as number), (eLon?.get(i) as number) ?? (sLon.get(i) as number))
  }

  // R5 flow conservation (Ondra 2026-06-12, C3.2 closure): along ONE road
  // (same ref) stamped by ONE source, AADT must not jump >3x between
  // segments that share an endpoint UNLESS a same-or-higher-class road also
  // touches that endpoint (a junction where traffic can really enter/leave).
  // Catches source data errors (PL provincial accept-anywhere), matching
  // mistakes and OSM ref typos. Per-hex only (cross-hex adjacency skipped);
  // full-row pass, not sampled — adjacency needs every segment.
  {
    const ref = t.getChild('ref')
    if (ref && med && hev && eLat && eLon) {
      const key = (lat: number, lon: number) => `${lat.toFixed(5)},${lon.toFixed(5)}`
      // endpoint → classes of ALL roads touching it (junction detection)
      const endpointClasses = new Map<string, number[]>()
      type Seg = { i: number; id: number; total: number; a: string; b: string; c: number; r: string }
      const segs: Seg[] = []
      for (let i = 0; i < t.numRows; i++) {
        const c = (cls.get(i) as number) ?? 0
        const a = key(sLat.get(i) as number, sLon.get(i) as number)
        const b = key((eLat.get(i) as number) ?? (sLat.get(i) as number), (eLon.get(i) as number) ?? (sLon.get(i) as number))
        for (const k of [a, b]) {
          const arr = endpointClasses.get(k) ?? []
          arr.push(c)
          endpointClasses.set(k, arr)
        }
        const id = (src.get(i) as number) ?? 0
        const r = (ref.get(i) as string | null) ?? ''
        if (!r || id === 0 || c > 4) continue
        const total = ((light.get(i) as number) ?? 0) + ((med.get(i) as number) ?? 0) + ((hev.get(i) as number) ?? 0)
        if (total > 0) segs.push({ i, id, total, a, b, c, r })
      }
      const byEndpoint = new Map<string, Seg[]>()
      for (const sg of segs) {
        for (const k of [sg.a, sg.b]) {
          const arr = byEndpoint.get(k) ?? []
          arr.push(sg)
          byEndpoint.set(k, arr)
        }
      }
      const flagged = new Set<string>()
      for (const [k, list] of byEndpoint) {
        if (list.length < 2) continue
        for (let x = 0; x < list.length; x++) {
          for (let y = x + 1; y < list.length; y++) {
            const A = list[x], B = list[y]
            if (A.r !== B.r || A.id !== B.id) continue
            const ratio = Math.max(A.total, B.total) / Math.max(1, Math.min(A.total, B.total))
            if (ratio <= 3) continue
            // Junction = ANY other road touches the endpoint (class-blind:
            // even a collector legitimately adds/removes traffic). The flag
            // therefore fires only on mid-segment jumps where NOTHING
            // connects — physically impossible flow changes.
            const touching = endpointClasses.get(k) ?? []
            const refSegsHere = list.filter((s2) => s2.r === A.r).length
            const hasJunction = touching.length > refSegsHere
            const fkey = `${A.r}:${k}`
            if (!hasJunction && !flagged.has(fkey)) {
              flagged.add(fkey)
              const [la2, lo2] = k.split(',').map(Number)
              report('R5 flow-jump', hex, A.i, osm ? Number(osm.get(A.i)) : null, A.id, la2, lo2,
                `ref=${A.r} ${A.total}→${B.total} (${ratio.toFixed(1)}x) at shared endpoint, no junction`)
            }
          }
        }
      }
    }
  }

  // R13 continuity-gap (complement of the continuity-fill, NOT a flow-jump): a
  // MEASURED major segment sharing a degree-2 endpoint with an UNMEASURED
  // (source_id==0) segment of the SAME canonical ref — a coverage gap where the
  // measured flow should continue but the row falls to the engine class default
  // (the physically-impossible jump continuity-fill repairs). Boolean, no ratio:
  // source_id==0 rows carry 0 in Arrow, the class default is applied in Rust
  // (normalize/road.rs). After the fill runs this flags only what it couldn't
  // reach — cross-hex, ref-less or conflicting-anchor gaps worth human triage.
  {
    const ref = t.getChild('ref')
    if (ref && eLat && eLon) {
      const canon = (r: string | null) => (r ? r.trim().toUpperCase().replace(/\s+/g, '') : '')
      const major = (c: number) => c <= 4 || (c >= 10 && c <= 12)
      const byEp = new Map<string, Array<{ i: number; src: number; r: string }>>()
      for (let i = 0; i < t.numRows; i++) {
        const c = (cls.get(i) as number) ?? 0
        if (!major(c)) continue
        const r = canon((ref.get(i) as string | null) ?? null)
        if (!r) continue
        const a = `${(sLat.get(i) as number).toFixed(5)},${(sLon.get(i) as number).toFixed(5)}`
        const b = `${(eLat.get(i) as number).toFixed(5)},${(eLon.get(i) as number).toFixed(5)}`
        const id = (src.get(i) as number) ?? 0
        for (const k of [a, b]) {
          const arr = byEp.get(k) ?? []
          arr.push({ i, src: id, r })
          byEp.set(k, arr)
        }
      }
      const flagged = new Set<string>()
      for (const [k, list] of byEp) {
        if (list.length !== 2) continue // degree-2 major only (no junction / same-ref fork)
        const [A, B] = list
        if (A.r !== B.r) continue // ref changes here → not the same road
        const meas = isMeasured(A.src) ? A : isMeasured(B.src) ? B : null
        const gap = A.src === 0 ? A : B.src === 0 ? B : null
        if (!meas || !gap || meas === gap) continue
        const fkey = `${meas.r}:${k}`
        if (flagged.has(fkey)) continue
        flagged.add(fkey)
        const [la2, lo2] = k.split(',').map(Number)
        report('R13 continuity-gap', hex, gap.i, osm ? Number(osm.get(gap.i)) : null, gap.src, la2, lo2,
          `ref=${meas.r} measured neighbour but this major segment is default (coverage gap)`)
      }
    }
  }
  return checked
})

// ── R0/R3/R6/R9/R10: railways ────────────────────────────────────────────────

const rails = scanLayer('railways.arrow', (t, hex) => {
  const rt = t.getChild('rail_type')
  const pax = t.getChild('trains_passenger')
  const frt = t.getChild('trains_freight')
  const src = t.getChild('source_id')
  const osm = t.getChild('osm_id')
  const sLat = t.getChild('start_lat'), sLon = t.getChild('start_lon')
  const eLat = t.getChild('end_lat'), eLon = t.getChild('end_lon')
  if (!requireCoreColumns(t, hex, 'railways.arrow', ['rail_type', 'source_id', 'start_lat', 'start_lon'])) return 0
  // trains_passenger is chain-only: absent = hex never rail-enriched → skip.
  if (!rt || !pax || !src || !sLat || !sLon) return 0
  let checked = 0
  for (const i of sampleRows(t.numRows)) {
    checked++
    const id = (src.get(i) as number) ?? 0
    const type = (rt.get(i) as number) ?? 0
    const p = (pax.get(i) as number) ?? 0
    const f = (frt?.get(i) as number) ?? 0
    const oid = osm ? Number(osm.get(i)) : null
    const [la, lo] = segMid(sLat, sLon, eLat, eLon, i)
    checkSourceRegistry('railways', hex, i, oid, id, la, lo)
    if (!Number.isFinite(p) || p < 0 || !Number.isFinite(f) || f < 0) {
      report('R6 value-range', hex, i, oid, id, la, lo,
        `trains_passenger=${p} trains_freight=${f} (negative or non-finite)`)
    }
    checkCountryBleed(hex, i, oid, id, la, lo,
      sLat.get(i) as number, sLon.get(i) as number,
      (eLat?.get(i) as number) ?? (sLat.get(i) as number), (eLon?.get(i) as number) ?? (sLon.get(i) as number))
    if (type !== 1 && type !== 2) continue
    if (p <= 400) continue
    const families = RAIL_FAMILIES.get(id)
    if (families && !families.has('tram')) {
      report('R3 tram-overcount', hex, i, oid, id, la, lo, `rail_type=${type} trains_passenger=${p} from non-tram source`)
    }
  }
  return checked
})

// ── R0/R4/R9/R10/R14: industrial ─────────────────────────────────────────────

// World membership for R14 — built on the first shared-id row seen, never for scopes
// with no shared-id stamps.
const inAnyCountry = makeAnyCountryGate()

const industrial = scanLayer('industrial.arrow', (t, hex) => {
  const name = t.getChild('name')
  const nace = t.getChild('nace_4digit')
  const src = t.getChild('source_id')
  const osm = t.getChild('osm_id')
  const lat = t.getChild('centroid_lat'), lon = t.getChild('centroid_lon')
  if (!requireCoreColumns(t, hex, 'industrial.arrow', ['name', 'source_id', 'centroid_lat', 'centroid_lon'])) return 0
  // nace_4digit is chain-only: absent = hex never industrial-enriched → skip.
  if (!name || !nace || !src || !lat || !lon) return 0
  let checked = 0
  for (const i of sampleRows(t.numRows)) {
    checked++
    const id = (src.get(i) as number) ?? 0
    const la = lat.get(i) as number, lo = lon.get(i) as number
    const oid = osm ? Number(osm.get(i)) : null
    checkSourceRegistry('industrial', hex, i, oid, id, la, lo)
    checkCountryBleed(hex, i, oid, id, la, lo, la, lo, la, lo)
    // R14: the shared national-mix id has no country identity, so R9 cannot see
    // it — but a stamp in NO ownership area is provably orphaned (see header).
    if (id === SOURCE_ID_GLOBAL_INDUSTRIAL_NATIONAL_MIX && Number.isFinite(la) && Number.isFinite(lo) && !inAnyCountry(la, lo)) {
      report('R14 industrial-orphan', hex, i, oid, id, la, lo,
        'shared-id stamp outside every ownership area — unreachable by any national pass since #31')
    }
    const nc = (nace.get(i) as number) ?? 0
    if (!POWER_NACE.has(nc)) continue
    const nm = (name.get(i) as string | null) ?? ''
    if (!nm) continue
    const lower = nm.toLowerCase()
    if (WIND_NAME_KEYWORDS.some(kw => lower.includes(kw))) {
      report('R4 wind-as-thermal', hex, i, oid, id, la, lo, `nace=${nc} name="${nm.slice(0, 50)}"`)
    }
  }
  return checked
})

// ── R0/R8/R9/R10/R12: buildings ──────────────────────────────────────────────

// Per-file contract stamp written by `osm-extract::finalize` and fail-loud
// asserted by `tile-painter::schema_check::BUILDINGS_CONTRACT_V2`
// (branch settlement-phase2). Value is the contract — never edit casually.
const BUILDINGS_CONTRACT_V2 = 'buildings_v2'

const buildings = scanLayer('buildings.arrow', (t, hex) => {
  const btype = t.getChild('building_type')
  const src = t.getChild('source_id')
  const osm = t.getChild('osm_id')
  const lat = t.getChild('centroid_lat'), lon = t.getChild('centroid_lon')
  if (!requireCoreColumns(t, hex, 'buildings.arrow', ['building_type', 'centroid_lat', 'centroid_lon'])) return 0
  if (!btype || !lat || !lon) return 0
  const hexLat = (lat.get(0) as number) ?? 0, hexLon = (lon.get(0) as number) ?? 0

  const contract = t.schema.metadata.get('buildings_contract')
  if (contract !== undefined && contract !== BUILDINGS_CONTRACT_V2) {
    report('R12 buildings-contract', hex, -1, null, 0, hexLat, hexLon,
      `buildings_contract='${contract}' (expected '${BUILDINGS_CONTRACT_V2}')`)
  }
  if (contract === BUILDINGS_CONTRACT_V2 && !t.getChild('opening_hours_frac')) {
    report('R12 buildings-contract', hex, -1, null, 0, hexLat, hexLon,
      `stamped ${BUILDINGS_CONTRACT_V2} but opening_hours_frac column is missing`)
  }

  let checked = 0
  let v2TypesInUnstamped = 0
  for (const i of sampleRows(t.numRows)) {
    checked++
    const id = (src?.get(i) as number) ?? 0
    const c = (btype.get(i) as number) ?? 0
    const la = lat.get(i) as number, lo = lon.get(i) as number
    const oid = osm ? Number(osm.get(i)) : null
    checkSourceRegistry('buildings', hex, i, oid, id, la, lo)
    checkCountryBleed(hex, i, oid, id, la, lo, la, lo, la, lo)
    if (c > MAX_BUILDING_TYPE) {
      report('R8 class-range', hex, i, oid, id, la, lo, `building_type=${c} outside engine codes 0..${MAX_BUILDING_TYPE}`)
    } else if (contract === undefined && c >= V2_SPECIFIC_TYPE_MIN) {
      v2TypesInUnstamped++
    }
  }
  if (v2TypesInUnstamped > 0) {
    report('R12 buildings-contract', hex, -1, null, 0, hexLat, hexLon,
      `${v2TypesInUnstamped}/${checked} sampled rows carry v2 type ids (>=${V2_SPECIFIC_TYPE_MIN}) but the file has no buildings_contract stamp — a rewrite dropped the schema metadata`)
  }
  return checked
})

// ── R15/R16: rail continuity (2026-07-15 railway continuity plan, Phase 2) ──
// A SEPARATE, full-row pass over the SAME hex set the railways scan above
// used (hexesInScope('railways.arrow')) — deliberately outside scanLayer's
// sampled loop, AFTER every per-layer scan (never inside the railways scan's
// sampleRows loop: R5 already establishes that adjacency needs every
// segment, and R15/R16 need the same full-row guarantee scope-wide, not just
// per-hex — see the module doc and /gg item 10).
const railContinuity = (() => {
  const t0 = Date.now()
  const hexes = hexesInScope('railways.arrow')
  const { rows, ioErrors: rowIoErrors } = collectRailEndpointRows(H3R4_DIR, hexes)
  // Dedup against ioErrors the sampled railways scan above already recorded
  // for the SAME hex (an unreadable file is unreadable either pass); a
  // genuinely NEW error — this rule's stricter end_lat/end_lon/usage/service
  // requirement catching a file the sampled scan's looser column set let
  // through — still reports.
  const alreadyFlagged = new Set(ioErrors.filter((e) => e.layer === 'railways.arrow').map((e) => e.hex))
  for (const e of rowIoErrors) {
    if (alreadyFlagged.has(e.hex)) continue
    reportIoError(e.hex, 'railways.arrow', e.error)
  }

  // Missing/empty sidecar is NOT a violation and NOT an IO error — it means
  // walk-based sources have not run in this tree yet. R15/R16
  // still run, just with the junction-only exemption (a real stop boundary
  // without a sidecar entry will over-fire until the sidecar lands).
  const stopsIndex = loadRailStopsIndex(H3R4_DIR)
  if (!stopsIndex) console.error(`R15/R16: ${RAIL_STOPS_UNAVAILABLE_WARNING}`)

  const rowByKey = new Map(rows.map((r) => [r.key, r]))
  const keyOfSource = (id: number): string => KEY_BY_ID.get(id) ?? `id ${id}`
  const severityBucket = (ratio: number): 'x3-10' | 'x10-30' | 'x30+' =>
    ratio >= 30 ? 'x30+' : ratio >= 10 ? 'x10-30' : 'x3-10'

  const reportViolation = (rule: string, v: RailContinuityViolation): void => {
    // hex/row/osm_id = the higher-effective-total side's — the side actually
    // carrying the (wrong) audible traffic is the one worth triaging first.
    const higherIsA = v.effA.total >= v.effB.total
    const higherKey = higherIsA ? v.aKey : v.bKey
    const higherSourceId = higherIsA ? v.aSourceId : v.bSourceId
    const [hex, rowIdxStr] = higherKey.split(':')
    const row = rowByKey.get(higherKey)
    const osmId = row && row.osmId ? Number(row.osmId) : null
    const pairLabel = `${keyOfSource(v.aSourceId)}→${keyOfSource(v.bSourceId)}|${severityBucket(v.ratio)}`
    const detail =
      `${keyOfSource(v.aSourceId)}→${keyOfSource(v.bSourceId)} eff pax ${v.effA.pax.toFixed(1)}/${v.effB.pax.toFixed(1)} ` +
      `frt ${v.effA.frt.toFixed(1)}/${v.effB.frt.toFixed(1)} total ${v.effA.total.toFixed(1)}/${v.effB.total.toFixed(1)} ` +
      `(col=${v.column}, ${v.ratio.toFixed(1)}x)`
    report(rule, hex, Number(rowIdxStr), osmId, higherSourceId, v.endpointLat, v.endpointLon, detail, pairLabel)
  }

  const jumps = findRailFlowJumps(rows, stopsIndex)
  for (const v of jumps) reportViolation('R15 rail-flow-jump', v)
  const gaps = findRailContinuityGaps(rows, stopsIndex)
  for (const v of gaps) reportViolation('R16 rail-continuity-gap', v)

  return { hexes: hexes.length, rows: rows.length, jumps: jumps.length, gaps: gaps.length, ms: Date.now() - t0 }
})()

// ── summary ──────────────────────────────────────────────────────────────────

const FIX_HINTS: Record<string, string> = {
  'R14 industrial-orphan': 'run heal-industrial-orphans.ts over the scope (chain step industrial-heal-orphans does this) — legacy shared-id stamps outside every ownership area',
  'R0 unknown-source': 'Register the id in lib/enrichment-datasets.ts (allocate-dataset-id.ts) or fix the enricher stamping a raw id, then reset + re-enrich the bbox.',
  'R1 road-coverage': 'The enricher bypassed its class gate — pass the declared coverage set to writeRoadAadt, then reset + re-enrich the affected hexes.',
  'R2 moto-scramble': 'Loader column mapping scrambled (cars in the moto column, the PL provincial XLS shape) — fix the source loader, reset + re-enrich. If the country genuinely rides motos, declare highMoto on the dataset.',
  'R3 tram-overcount': 'Heavy-rail counts landed on tram rows — filter by rail_type in the enricher or declare railFamilies correctly, then re-enrich.',
  'R4 wind-as-thermal': 'Wind site classified as thermal power — wind is source_type=10, never NACE 35xx; fix the NACE assignment and re-run industrial enrichment.',
  'R5 flow-jump': 'Check the national census values at the printed endpoint — source data error, wrong section match, or OSM ref typo; fix the match and re-enrich that road.',
  'R6 value-range': 'Physically impossible value — loader unit/parse error (vehicles/hour? thousands? sentinel -1?); fix the loader, reset + re-enrich.',
  'R7 zero-write': 'Enricher stamped its id but wrote zeros (the IT/SA bug shape, predates the writeRoadAadt guard) — re-run the enricher on current code.',
  'R8 class-range': 'road_class outside engine inputs.rs 0..12 — extraction bug; re-extract the hex (osm-to-h3r4.sh).',
  'R9 country-bleed': 'Cross-border stamp — gate the enricher match by makeCountryGate (lib/country-polygon.ts) and reset + re-enrich BOTH countries. The PL GPR → CZ bleed is the known deferred case (gate fixed in enrich-roads-pl.ts; data awaits re-enrich).',
  'R10 layer-mismatch': 'Wrong dataset-id constant in the enricher — stamp the id registered for this layer, then reset + re-enrich.',
  'R11 unknown-country': "Dataset key's country prefix has no CGAZ/ISO-3166 identity — fix the key, or add the prefix to NON_COUNTRY_PREFIXES / CITY_COUNTRY in this scanner.",
  'R12 buildings-contract': 'Settlement v2 contract broken — a rewrite dropped/garbled schema metadata. Rewrite buildings.arrow ONLY via lib/buildings-arrow.ts::writeBuildingEnrichment; re-extract the hex (osm-to-h3r4.sh) to restore the stamp.',
  'R13 continuity-gap': 'A measured major road continues into a same-ref default segment with no junction between them — run enrich-roads-continuity-fill.ts (it copies the measured value across). Residue after the fill = cross-hex gap, ref-less road, or conflicting anchors; triage by hand.',
  'R15 rail-flow-jump': "banding/matcher artifact or a genuinely wrong section value — re-run the graph-walk matcher for this line; a legit service boundary means a missing stop in the country's rail-stops sidecar",
  'R16 rail-continuity-gap': 'measured line continues into an engine-default row — between-stop gap the walk should cover; residue = genuine end-of-data, document in provenance.md',
}

console.log(`=== Enrichment invariant scan — ${BBOXES.length} bbox(es) ${BBOXES.map((b) => b.join(',')).join(' | ')} (${YEAR}) ===`)
if (SAMPLE !== Infinity) console.log(`  sampling: ~${SAMPLE} rows per hex per layer`)
console.log(`  roads      ${roads.hexes} hexes  ${roads.rows.toLocaleString()} rows checked  ${(roads.ms / 1000).toFixed(1)}s`)
console.log(`  railways   ${rails.hexes} hexes  ${rails.rows.toLocaleString()} rows checked  ${(rails.ms / 1000).toFixed(1)}s`)
console.log(`  industrial ${industrial.hexes} hexes  ${industrial.rows.toLocaleString()} rows checked  ${(industrial.ms / 1000).toFixed(1)}s`)
console.log(`  buildings  ${buildings.hexes} hexes  ${buildings.rows.toLocaleString()} rows checked  ${(buildings.ms / 1000).toFixed(1)}s`)
console.log(
  `  rail-continuity ${railContinuity.hexes} hexes  ${railContinuity.rows.toLocaleString()} rows checked  ` +
  `${railContinuity.jumps} R15  ${railContinuity.gaps} R16  ${(railContinuity.ms / 1000).toFixed(1)}s`,
)

// ── machine outputs — written on EVERY exit path so the chain gate can never
// mistake a missing file for "all resolved" (it fails on absent/unparsable). ──
if (NDJSON_PATH) {
  const lines = violations.map((v) =>
    JSON.stringify({
      rule: v.rule, source: v.source, hex: v.hex, osm_id: v.osmId, row: v.row,
      // toFixed(5) STRINGS: a stable textual fingerprint immune to float re-formatting.
      lat5: v.lat.toFixed(5), lon5: v.lon.toFixed(5),
      detail: v.detail,
    }),
  )
  writeFileSync(NDJSON_PATH, lines.length > 0 ? lines.join('\n') + '\n' : '')
}
if (SUMMARY_PATH) {
  const counts: Record<string, number> = {}
  for (const [rule, perSource] of byRuleSource) {
    for (const [srcKey, c] of perSource) counts[`${rule}|${srcKey}`] = c
  }
  writeFileSync(
    SUMMARY_PATH,
    JSON.stringify(
      { dataYear: YEAR, bboxes: BBOXES.map((b) => b.join(',')), total: violations.length, counts, ioErrors: ioErrors.length },
      null, 2,
    ) + '\n',
  )
}

// Operational damage outranks the violation verdict: a corrupt hex must exit 3
// (chain FAIL, not baselineable), never fold into exit 0/1 semantics. Fail-
// closed is the DEFAULT (#31.3); --lenient-io is the explicit exploratory
// escape hatch and still prints every warning above.
if (!LENIENT_IO && ioErrors.length > 0) {
  console.error(`\n${ioErrors.length} OPERATIONAL I/O ERROR(S) — unreadable arrow or broken extract-core schema (exit 3):`)
  for (const e of ioErrors.slice(0, 20)) console.error(`  ${e.layer} ${e.hex}: ${e.error}`)
  if (ioErrors.length > 20) console.error(`  … and ${ioErrors.length - 20} more`)
  process.exit(3)
}

if (violations.length === 0) {
  console.log(`\nCLEAN — no invariant violations.`)
  process.exit(0)
}

console.log(`\n${violations.length} VIOLATION(S):`)
for (const [rule, count] of [...byRule].sort((a, b) => b[1] - a[1])) {
  const perSource = [...(byRuleSource.get(rule) ?? [])].sort((a, b) => b[1] - a[1])
    .map(([src, c]) => `${src}: ${c}`).join(', ')
  console.log(`  ${rule}: ${count}  (${perSource})`)
}
// Rarest rules first — a 40k-row flood from one rule must not push the
// single R4/R8 finding out of the 50-row window.
violations.sort((a, b) => (byRule.get(a.rule)! - byRule.get(b.rule)!) || a.rule.localeCompare(b.rule))
console.log(`\n  ${'rule'.padEnd(18)} ${'source'.padEnd(22)} ${'hex'.padEnd(16)} ${'row'.padStart(7)} ${'osm_id'.padStart(12)}  ${'lat'.padStart(8)} ${'lon'.padStart(9)}  detail`)
for (const v of violations.slice(0, 50)) {
  console.log(`  ${v.rule.padEnd(18)} ${v.source.padEnd(22)} ${v.hex.padEnd(16)} ${String(v.row).padStart(7)} ${String(v.osmId ?? '—').padStart(12)}  ${v.lat.toFixed(4).padStart(8)} ${v.lon.toFixed(4).padStart(9)}  ${v.detail}`)
}
if (violations.length > 50) console.log(`  … and ${violations.length - 50} more`)

if (FIX_HINT) {
  console.log(`\nFIX HINTS:`)
  for (const [rule] of [...byRule].sort((a, b) => b[1] - a[1])) {
    console.log(`  ${rule}: ${FIX_HINTS[rule] ?? '(no hint registered)'}`)
  }
}
process.exit(1)
