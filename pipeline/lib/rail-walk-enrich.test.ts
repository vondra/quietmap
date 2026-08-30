/**
 * Driver-level tests for `enrichRailwaysByGraphWalk` (rail-walk-enrich.ts).
 * `rail-graph.ts`/`rail-graph-metrics.ts` already lock the pure topology and
 * routing invariants (T-junction healing, Dijkstra, ambiguity, parallel
 * spread, the 2026-07-16 Step-B quarantine ellipse) — these tests exercise
 * the I/O DRIVER built on top: cross-hex stitching, the enableDestructive
 * gate, quarantine-gated retract/silent withholding, the bleed-gate
 * retract arm (item 1: explicit `bleedGate`, never `countryGate`), and the
 * rail-stops sidecar.
 *
 * Two real H3 res-4 cells (SPEC reference hexes) are used as directory names
 * so `iterateCountryHexes`'s `cellToLatLng` validity check passes; the
 * segment coordinates inside each fixture are independent of the hex's own
 * geometry (as in production, a hex's `railways.arrow` may hold any segment
 * assigned to it).
 *
 * Run: `cd pipeline && npx tsx --test lib/rail-walk-enrich.test.ts`
 */

import { test, after } from 'node:test'
import assert from 'node:assert/strict'
import { mkdtempSync, mkdirSync, rmSync, writeFileSync, readFileSync, existsSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { tmpdir } from 'node:os'
import {
  Table, vectorFromArray, tableFromIPC, tableToIPC,
  Float64, Float32, Int32, Uint8, Uint16, Utf8,
} from 'apache-arrow'
import { enrichRailwaysByGraphWalk } from './rail-walk-enrich.js'

// cz-szcd-gtfs (national-measured) and global-gtfs-transit (continental-measured) —
// both registered `layer: 'railways'` sources (see enrichment-datasets.ts), so
// writeRailTrains's registry-membership check accepts them.
const STAMP_ID = 110
const GLOBAL_GTFS_ID = 100
// The foreign-national guard fixture trio (test e3): de-national-railway walks,
// cz-szcd-gtfs (national-measured) + cz-timetable-silent (baseline tier,
// `nationallyOwned` registry flag) must both survive it.
const DE_NATIONAL_ID = 9864
const CZ_NATIONAL_ID = STAMP_ID
const CZ_SILENT_ID = 9863

// Reference hexes (CLAUDE.md "Reference hexes"): Dobříš ~49.865,13.960; Ruzyně
// ~50.141,14.424. Only their VALIDITY as H3 res-4 cells + centroid-in-bbox
// matters here — fixture segment coordinates are chosen independently below.
const HEX_A = '841e309ffffffff'
const HEX_B = '841e355ffffffff'
const BBOX: readonly [number, number, number, number] = [49.0, 13.0, 51.0, 15.0]

const TMP = mkdtempSync(join(tmpdir(), 'rail-walk-enrich-test-'))
after(() => rmSync(TMP, { recursive: true, force: true }))

/** `${TMP}/<name>/prepared/<year>/h3r4` — mirrors the real
 *  `data/prepared/{year}/h3r4` shape so the sidecar's year-from-parent-dir
 *  derivation resolves to a distinctive, assertable value. */
function freshScopeDir(name: string, year = '2099'): string {
  const dir = join(TMP, name, 'prepared', year, 'h3r4')
  mkdirSync(dir, { recursive: true })
  return dir
}

interface FixtureRow {
  startLat: number
  startLon: number
  endLat: number
  endLon: number
  railType?: number
  usage?: number
  service?: number
  name?: string
  ref?: string
  sourceId?: number
  pax?: number
  frt?: number
  divisor?: number
}

/** Flat-earth length (metres) — same formula family as spatial.ts's
 *  flatDist, used only to populate/cross-check the fixture's `length_m`. */
function segLengthM(r: Pick<FixtureRow, 'startLat' | 'startLon' | 'endLat' | 'endLon'>): number {
  const dLat = (r.endLat - r.startLat) * 110_540
  const dLon = (r.endLon - r.startLon) * 111_320 * Math.cos(((r.startLat + r.endLat) / 2) * Math.PI / 180)
  return Math.sqrt(dLat * dLat + dLon * dLon)
}

/** A minimal railways.arrow: only the columns rail-walk-enrich.ts and
 *  writeRailTrains actually read (see write_railways.rs for the FULL engine
 *  schema — segment_idx/maxspeed/electrified/gauge/bridge/tunnel/highspeed
 *  are irrelevant to either reader and omitted). */
function writeRailwaysArrowFixture(path: string, rows: FixtureRow[]): void {
  const idx = [...rows.keys()]
  // eslint-disable-next-line @typescript-eslint/no-explicit-any -- heterogeneous Vector record
  const cols: Record<string, any> = {
    osm_id: vectorFromArray(idx.map(i => 800_000 + i), new Float64()),
    start_lat: vectorFromArray(rows.map(r => r.startLat), new Float64()),
    start_lon: vectorFromArray(rows.map(r => r.startLon), new Float64()),
    end_lat: vectorFromArray(rows.map(r => r.endLat), new Float64()),
    end_lon: vectorFromArray(rows.map(r => r.endLon), new Float64()),
    length_m: vectorFromArray(rows.map(r => segLengthM(r)), new Float32()),
    rail_type: vectorFromArray(rows.map(r => r.railType ?? 0), new Uint8()),
    usage: vectorFromArray(rows.map(r => r.usage ?? 0), new Uint8()),
    name: vectorFromArray(rows.map(r => r.name ?? ''), new Utf8()),
    ref: vectorFromArray(rows.map(r => r.ref ?? ''), new Utf8()),
    service: vectorFromArray(rows.map(r => r.service ?? 0), new Uint8()),
    source_id: vectorFromArray(rows.map(r => r.sourceId ?? 0), new Uint16()),
    trains_passenger: vectorFromArray(rows.map(r => r.pax ?? 0), new Int32()),
    trains_freight: vectorFromArray(rows.map(r => r.frt ?? 0), new Int32()),
    parallel_divisor: vectorFromArray(rows.map(r => r.divisor ?? 1), new Uint8()),
  }
  const table = new Table(cols)
  writeFileSync(path, Buffer.from(tableToIPC(table, 'file')))
}

function putHex(h3r4Dir: string, hexId: string, rows: FixtureRow[]): string {
  const dir = resolve(h3r4Dir, hexId)
  mkdirSync(dir, { recursive: true })
  const path = resolve(dir, 'railways.arrow')
  writeRailwaysArrowFixture(path, rows)
  return path
}

function readCols(path: string) {
  const t = tableFromIPC(readFileSync(path))
  return {
    pax: (i: number) => t.getChild('trains_passenger')!.get(i) as number,
    frt: (i: number) => t.getChild('trains_freight')!.get(i) as number,
    src: (i: number) => t.getChild('source_id')!.get(i) as number,
    div: (i: number) => t.getChild('parallel_divisor')!.get(i) as number,
  }
}

// ── (a) two-hex scope: a line crossing the hex boundary, both directions sum ─

test('two-hex scope: line crosses the hex boundary, walk stitches across it, both directions sum, stamps land in both files', async () => {
  const h3r4Dir = freshScopeDir('two-hex')
  putHex(h3r4Dir, HEX_A, [
    { startLat: 50.000, startLon: 14.000, endLat: 50.001, endLon: 14.000 }, // row0: A -> mid1
    { startLat: 50.001, startLon: 14.000, endLat: 50.002, endLon: 14.000 }, // row1: mid1 -> border (shared coord with hex B)
  ])
  putHex(h3r4Dir, HEX_B, [
    { startLat: 50.002, startLon: 14.000, endLat: 50.003, endLon: 14.000 }, // row0: border -> mid2
    { startLat: 50.003, startLon: 14.000, endLat: 50.004, endLon: 14.000 }, // row1: mid2 -> C
  ])

  const stats = await enrichRailwaysByGraphWalk({
    h3r4Dir,
    bbox: BBOX,
    pairs: [
      { fromLat: 50.000, fromLon: 14.000, toLat: 50.004, toLon: 14.000, pax: 16, frt: 0 },
      { fromLat: 50.004, fromLon: 14.000, toLat: 50.000, toLon: 14.000, pax: 16, frt: 0 }, // reverse direction — must SUM
    ],
    sourceId: STAMP_ID,
    retractSafe: true,
    enableDestructive: false,
    sidecar: { scope: 'two-hex-scope', extractFingerprint: 'fp-a', feeds: ['feedA'] },
  })

  assert.equal(stats.hexes, 2)
  assert.equal(stats.rows, 4)
  assert.equal(stats.stamped, 4, 'all four segments sit on the one shortest path')
  assert.equal(stats.walk.pairsTotal, 1, 'both directions collapse into ONE canonical pair')
  assert.equal(stats.walk.pairsWalked, 1)

  for (const hexId of [HEX_A, HEX_B]) {
    const c = readCols(resolve(h3r4Dir, hexId, 'railways.arrow'))
    for (let i = 0; i < 2; i++) {
      assert.equal(c.pax(i), 32, `${hexId} row ${i} pax summed both directions`)
      assert.equal(c.frt(i), 0)
      assert.equal(c.src(i), STAMP_ID)
    }
  }

  assert.notEqual(stats.sidecarPath, '')
  assert.ok(existsSync(stats.sidecarPath))
  const sidecar = JSON.parse(readFileSync(stats.sidecarPath, 'utf8'))
  assert.equal(sidecar.version, 1)
  assert.equal(sidecar.year, '2099')
  assert.equal(sidecar.scope, 'two-hex-scope')
  assert.deepEqual(sidecar.feeds, ['feedA'])
  assert.equal(sidecar.stops.length, 2, 'both station endpoints, deduped at 4dp')
  // DE Step A v2 (2026-07-16 failure analysis, fix 3): the sidecar always
  // carries failedPairChords now, even when — as here — nothing failed.
  assert.deepEqual(sidecar.failedPairChords, [])
})

// ── (b) enableDestructive=false: walk stamps (incl. their divisor) land, ────
// retract/silent stay suppressed (2026-07-16 /gg review item 3: divisor
// rides with the atomic write in EVERY mode — "stamp-only" means no retract,
// no silent, never "no divisor").

test('enableDestructive=false: walk stamps land WITH their own divisor; retract/silent stay suppressed', async () => {
  const h3r4Dir = freshScopeDir('no-destructive')
  putHex(h3r4Dir, HEX_A, [
    { startLat: 50.100, startLon: 14.100, endLat: 50.101, endLon: 14.100, divisor: 9 }, // row0 A-B — walked
    { startLat: 50.101, startLon: 14.100, endLat: 50.101, endLon: 14.101, sourceId: 999, pax: 50, frt: 5, divisor: 3 }, // row1 B-D spur, legacy stamp under a retract id
    { startLat: 50.101, startLon: 14.100, endLat: 50.102, endLon: 14.100, divisor: 9 }, // row2 B-C — walked
  ])

  const stats = await enrichRailwaysByGraphWalk({
    h3r4Dir,
    bbox: BBOX,
    pairs: [{ fromLat: 50.100, fromLon: 14.100, toLat: 50.102, toLon: 14.100, pax: 10, frt: 2 }],
    sourceId: STAMP_ID,
    silentResidual: { sourceId: GLOBAL_GTFS_ID, pax: 2, frt: 1 },
    retract: { sourceIds: [999] },
    retractSafe: true,
    enableDestructive: false, // <- under test
    sidecar: { scope: 'no-destructive-scope', extractFingerprint: 'fp-b', feeds: [] },
  })

  assert.equal(stats.stamped, 2, 'row0 + row2 walked (A-B-C is the only path)')
  assert.equal(stats.silentStamped, 0, 'silent suppressed by enableDestructive=false')
  assert.equal(stats.retracted, 0, 'retract suppressed by enableDestructive=false — no RailRetract object is even built (no countryGate bleed either)')

  const c = readCols(resolve(h3r4Dir, HEX_A, 'railways.arrow'))
  assert.equal(c.src(1), 999, 'legacy stamp untouched')
  assert.equal(c.pax(1), 50)
  assert.equal(c.frt(1), 5)
  assert.equal(c.div(1), 3, 'untouched row keeps its pre-existing divisor byte-for-byte')
  // Divisor rides with the walk stamp in the SAME atomic write regardless of
  // enableDestructive — row0/row2 move from their pre-seeded 9 down to the
  // walk's true divisor (1, no parallel siblings on this simple line) as
  // PART OF the match's own return value, not a separate opt-in pass.
  assert.equal(c.div(0), 1)
  assert.equal(c.div(2), 1)
})

// ── (c) enableDestructive=true, failure-free component ──────────────────────

test('enableDestructive=true + failure-free component: silent residual on the unwalked spur, retract disowns an unreachable legacy stamp, divisor rides with EVERY accepted match (walk AND silent)', async () => {
  const h3r4Dir = freshScopeDir('destructive-clean')
  putHex(h3r4Dir, HEX_A, [
    { startLat: 52.000, startLon: 16.000, endLat: 52.001, endLon: 16.000, divisor: 9 }, // row0 E-F — walked
    { startLat: 52.001, startLon: 16.000, endLat: 52.002, endLon: 16.000, divisor: 9 }, // row1 F-G — walked
    { startLat: 52.001, startLon: 16.000, endLat: 52.001, endLon: 16.001, divisor: 7 }, // row2 F-H spur — unwalked, same failure-free component, NO parallel sibling
    { startLat: 52.010, startLon: 16.010, endLat: 52.011, endLon: 16.010, railType: 1, sourceId: 999, pax: 77, frt: 3, divisor: 4 }, // row3 tram, legacy stamp, outside the graph entirely
  ])

  const stats = await enrichRailwaysByGraphWalk({
    h3r4Dir,
    bbox: BBOX,
    pairs: [{ fromLat: 52.000, fromLon: 16.000, toLat: 52.002, toLon: 16.000, pax: 10, frt: 2 }],
    sourceId: STAMP_ID,
    silentResidual: { sourceId: GLOBAL_GTFS_ID, pax: 2, frt: 1 },
    retract: { sourceIds: [999] },
    retractSafe: true,
    enableDestructive: true,
    sidecar: { scope: 'destructive-clean-scope', extractFingerprint: 'fp-c', feeds: ['feedC'] },
  })

  assert.equal(stats.stamped, 2, 'row0 + row1 walked')
  assert.equal(stats.silentStamped, 1, 'row2 (unwalked mainline spur, failure-free component)')
  assert.equal(stats.retracted, 1, 'row3 (tram, legacy id, no graph component to withhold on)')

  const c = readCols(resolve(h3r4Dir, HEX_A, 'railways.arrow'))
  assert.equal(c.pax(0), 10); assert.equal(c.frt(0), 2); assert.equal(c.src(0), STAMP_ID)
  assert.equal(c.pax(1), 10); assert.equal(c.frt(1), 2); assert.equal(c.src(1), STAMP_ID)
  assert.equal(c.pax(2), 2); assert.equal(c.frt(2), 1); assert.equal(c.src(2), GLOBAL_GTFS_ID)
  assert.equal(c.pax(3), 0); assert.equal(c.frt(3), 0); assert.equal(c.src(3), 0, 'tram disowned, nothing reclaims it')

  // Divisor: row0/row1 (walked) move from the pre-seeded 9 down to the walk's
  // true divisor (1, no parallel siblings here) as part of the atomic write.
  // row2 (SILENT-stamped, previously "untouched" under the old separate-pass
  // design — the very bug this fix closes) ALSO moves to its true divisor —
  // here 1, since it's a lone spur with no sibling in divisorBySegmentKey
  // either, so its stale pre-seeded 7 is corrected rather than left stale.
  // row3 (retracted) is reset to 1 by writeRailTrains's own divisor-void-on-
  // retract invariant.
  assert.equal(c.div(0), 1)
  assert.equal(c.div(1), 1)
  assert.equal(c.div(2), 1, 'silent-stamped row is corrected too — no sibling recorded for this lone spur, so it rides at divisor 1, not its stale pre-seeded 7')
  assert.equal(c.div(3), 1)
})

// ── (d) a DISCONNECTED pair takes endpoint-radius BALLS around both snapped
// ends (item 4 review round: its trains exist, the graph is broken between
// the stations — an empty ellipse would hand live lines to the silent
// residual). Both tiny components here sit entirely inside the pair's own
// bound (~292 km, from the ~116 km chord), so silent and retract are fully
// withheld — see rail-graph-metrics.test.ts for the ball's outer limit and
// the ambiguous-only ellipse. ───────────────────────────────────────────────

test('a disconnected pair\'s graphless evidence shapes withhold silent+retract near both stations — stamps only, and the taxonomy/km numbers are exact', async () => {
  const h3r4Dir = freshScopeDir('failed-component')
  const rowEF = { startLat: 52.100, startLon: 16.100, endLat: 52.101, endLon: 16.100 } // row0
  const rowFG = { startLat: 52.101, startLon: 16.100, endLat: 52.102, endLon: 16.100 } // row1
  const rowFH = { startLat: 52.101, startLon: 16.100, endLat: 52.101, endLon: 16.101, sourceId: 999, pax: 15, frt: 3 } // row2 spur, legacy stamp
  const rowPQ = { startLat: 53.000, startLon: 17.000, endLat: 53.001, endLon: 17.000 } // row3 — a SEPARATE, disconnected component
  putHex(h3r4Dir, HEX_A, [rowEF, rowFG, rowFH, rowPQ])

  const stats = await enrichRailwaysByGraphWalk({
    h3r4Dir,
    bbox: BBOX,
    // E (component 1) to Q (component 2): both snap, but no path connects them — 'disconnected'.
    pairs: [{ fromLat: 52.100, fromLon: 16.100, toLat: 53.001, toLon: 17.000, pax: 5, frt: 1 }],
    sourceId: STAMP_ID,
    silentResidual: { sourceId: GLOBAL_GTFS_ID, pax: 2, frt: 1 },
    retract: { sourceIds: [999] },
    retractSafe: true,
    enableDestructive: true,
    sidecar: { scope: 'failed-component-scope', extractFingerprint: 'fp-d', feeds: [] },
  })

  assert.equal(stats.stamped, 0, 'the only pair failed — nothing walked')
  assert.equal(stats.silentStamped, 0, 'both tiny components sit entirely inside the graphless evidence shapes — no silent even though otherwise eligible')
  assert.equal(stats.retracted, 0, 'row2 legacy stamp survives — it sits inside E\'s ball')
  assert.equal(stats.walk.failures.disconnected, 1)
  const expectedKm = (segLengthM(rowEF) + segLengthM(rowFG) + segLengthM(rowFH) + segLengthM(rowPQ)) / 1000
  assert.ok(
    Math.abs(stats.quarantinedKm - expectedKm) < 1e-6,
    `quarantinedKm ${stats.quarantinedKm} ~= ${expectedKm} — the graphless evidence shapes sweep both tiny components in full here`,
  )

  const c = readCols(resolve(h3r4Dir, HEX_A, 'railways.arrow'))
  assert.equal(c.src(0), 0, 'row0 (E-F) unstamped baseline, untouched')
  assert.equal(c.src(1), 0, 'row1 (F-G) unstamped baseline, untouched')
  assert.equal(c.src(2), 999, 'row2 (F-H) legacy stamp survives untouched — a live-but-unroutable line must not go silent at 2+1/day')
  assert.equal(c.pax(2), 15)
  assert.equal(c.frt(2), 3)
})

// ── (d2) an AMBIGUOUS pair (the ONE failure kind that takes the
// admissible-path ellipse — item 4 + review round) withholds silent inside
// that region but not on a dead-end tail past the distA+distB bound —
// exactly the property whole-component gating could never express, since the
// whole component would have withheld silent everywhere. ────────────────────

test('an ambiguous pair\'s candidate-path union withholds silent on its corridors — dead-end tails at the station stay silent-eligible', async () => {
  const h3r4Dir = freshScopeDir('quarantine-ellipse-vs-far-branch')
  // Same geometry as rail-graph-metrics.test.ts's path-union test: two
  // disjoint similar-length corridors A-N-B / A-S-B (~7.49 km each) fail
  // 'ambiguous'; A also anchors a two-hop dead-end tail running WEST,
  // opposite B. The union of candidate paths is exactly both corridors —
  // neither tail hop carries an admissible A-B path, so silent applies to
  // BOTH (the earlier graph-distance ellipse still leaked onto tail1
  // through its A endpoint; scaled up, one long ambiguous leg blanketed a
  // country).
  const rowAN = { startLat: 50.000, startLon: 14.000, endLat: 50.010, endLon: 14.050 }
  const rowNB = { startLat: 50.010, startLon: 14.050, endLat: 50.000, endLon: 14.100 }
  const rowAS = { startLat: 50.000, startLon: 14.000, endLat: 49.990, endLon: 14.050 }
  const rowSB = { startLat: 49.990, startLon: 14.050, endLat: 50.000, endLon: 14.100 }
  const rowTail1 = { startLat: 50.000, startLon: 14.000, endLat: 50.000, endLon: 13.902 }
  const rowTail2 = { startLat: 50.000, startLon: 13.902, endLat: 50.000, endLon: 13.804 }
  putHex(h3r4Dir, HEX_A, [rowAN, rowNB, rowAS, rowSB, rowTail1, rowTail2])

  const stats = await enrichRailwaysByGraphWalk({
    h3r4Dir,
    bbox: BBOX,
    pairs: [{ fromLat: 50.000, fromLon: 14.000, toLat: 50.000, toLon: 14.100, pax: 5, frt: 0 }],
    sourceId: STAMP_ID,
    silentResidual: { sourceId: GLOBAL_GTFS_ID, pax: 2, frt: 1 },
    retractSafe: true,
    enableDestructive: true,
    sidecar: { scope: 'quarantine-ellipse-vs-far-branch-scope', extractFingerprint: 'fp-far', feeds: [] },
  })

  assert.equal(stats.walk.failures.ambiguous, 1)
  assert.equal(stats.silentStamped, 2, 'both tail hops sit outside the candidate-path union — silent-eligible (the ellipse used to withhold tail1; the component-wide gate withheld both)')

  const c = readCols(resolve(h3r4Dir, HEX_A, 'railways.arrow'))
  assert.equal(c.src(0), 0, 'row0 (A-N) — a candidate corridor, withheld')
  assert.equal(c.src(1), 0, 'row1 (N-B) — withheld')
  assert.equal(c.src(2), 0, 'row2 (A-S) — the competing candidate corridor, withheld')
  assert.equal(c.src(3), 0, 'row3 (S-B) — withheld')
  assert.equal(c.src(4), GLOBAL_GTFS_ID, 'row4 (tail1) — on no candidate path: silent applies (the ellipse used to leak here through the A endpoint)')
  assert.equal(c.src(5), GLOBAL_GTFS_ID, 'row5 (tail2) — on no candidate path: silent applies even though it shares a component with the failed pair')

  // DE Step A v2 (2026-07-16 failure analysis, fix 3): the driver persists
  // the failed pair's diagnostics into the SAME sidecar A/B's snapped stops
  // already write — a v3 tuning pass reads this file instead of re-running
  // the whole enrichment.
  assert.notEqual(stats.sidecarPath, '')
  const sidecar = JSON.parse(readFileSync(stats.sidecarPath, 'utf8'))
  assert.equal(sidecar.failedPairChords.length, 1)
  assert.equal(sidecar.failedPairChords[0].reason, 'ambiguous')
  assert.ok(sidecar.failedPairChords[0].ambiguousGeometry, 'persisted record carries the v3 tuning geometry summary')
})

// ── (e) bleed-gate retract arm (item 1): requires enableDestructive=true, ───
// but bypasses retractSafe/quarantine health WITHIN a destructive run
// (2026-07-16 /gg review item 3 — `enableDestructive` gates the retract
// object as a WHOLE). The arm runs ONLY when an explicit `bleedGate` is
// provided — a nationally-owned id (CZ) passes its own country gate here.

test('bleed retract (explicit bleedGate): fires when enableDestructive=true even with retractSafe=false; a straddler survives', async () => {
  const h3r4Dir = freshScopeDir('country-bleed')
  const gate = (_lat: number, lon: number) => lon < 14.05 // "domestic" = west of 14.05
  putHex(h3r4Dir, HEX_A, [
    { startLat: 50.200, startLon: 15.000, endLat: 50.200, endLon: 15.001, sourceId: 999, pax: 20, frt: 4 }, // row0: wholly foreign
    { startLat: 50.201, startLon: 14.040, endLat: 50.201, endLon: 15.000, sourceId: 999, pax: 25, frt: 5 }, // row1: straddler (start domestic)
  ])

  const stats = await enrichRailwaysByGraphWalk({
    h3r4Dir,
    bbox: BBOX,
    pairs: [],
    sourceId: STAMP_ID,
    countryGate: gate,
    bleedGate: gate, // nationally-owned id: its own country gate IS the union of its legitimate territory
    retract: { sourceIds: [999] },
    retractSafe: false, // deliberately false — the bleed arm must fire anyway (WITHIN a destructive run)
    enableDestructive: true,
    sidecar: { scope: 'country-bleed-scope', extractFingerprint: 'fp-e', feeds: [] },
  })

  assert.equal(stats.retracted, 1, 'only the wholly-foreign row is disowned')
  assert.equal(stats.skippedForeign, 1, 'row0 never offered to match; row1 straddler is claimable (R9 semantics)')

  const c = readCols(resolve(h3r4Dir, HEX_A, 'railways.arrow'))
  assert.equal(c.src(0), 0, 'wholly-foreign row disowned')
  assert.equal(c.pax(0), 0)
  assert.equal(c.frt(0), 0)
  assert.equal(c.src(1), 999, 'straddler survives — geometry does not condemn it')
  assert.equal(c.pax(1), 25)
})

test('shared-id contract (item 1): countryGate WITHOUT bleedGate never disowns an out-of-gate row — the bleed arm is OFF and the ordinary arm is testimony-bound', async () => {
  // The order-destroying bug this pins: a per-country europe run (CH) whose
  // bbox overlaps a neighbour (southern DE) must NOT disown the SHARED
  // GLOBAL_GTFS_TRANSIT rows the neighbour's own run legitimately stamped —
  // neither via a bleed arm keyed off its own countryGate (bleed is off
  // without an explicit bleedGate) nor via the ordinary retractSafe arm (a
  // run's feeds testify only about the territory inside its countryGate).
  const h3r4Dir = freshScopeDir('shared-id-no-bleed')
  const gate = (_lat: number, lon: number) => lon < 14.05 // "domestic" = west of 14.05
  putHex(h3r4Dir, HEX_A, [
    { startLat: 50.200, startLon: 15.000, endLat: 50.200, endLon: 15.001, sourceId: 999, pax: 20, frt: 4 }, // row0: wholly foreign — a NEIGHBOUR's legitimate stamp under the shared id
    { startLat: 50.201, startLon: 14.000, endLat: 50.201, endLon: 14.001, sourceId: 999, pax: 30, frt: 6 }, // row1: domestic stale stamp — ordinary retract still applies
  ])

  const stats = await enrichRailwaysByGraphWalk({
    h3r4Dir,
    bbox: BBOX,
    pairs: [], // this run's walk claims nothing — both rows are retract candidates on paper
    sourceId: STAMP_ID,
    countryGate: gate,
    // NO bleedGate — the per-country europe mode contract.
    retract: { sourceIds: [999] },
    retractSafe: true, // even a provably complete snapshot must not reach across the border
    enableDestructive: true,
    sidecar: { scope: 'shared-id-no-bleed-scope', extractFingerprint: 'fp-shared', feeds: [] },
  })

  assert.equal(stats.retracted, 1, 'ONLY the domestic stale row is disowned')
  assert.equal(stats.skippedForeign, 1, 'the foreign row is never offered to match either')

  const c = readCols(resolve(h3r4Dir, HEX_A, 'railways.arrow'))
  assert.equal(c.src(0), 999, 'out-of-gate row of the shared id SURVIVES — foreign healing belongs to the union-gated world mode / heal-rail-country-bleed.ts')
  assert.equal(c.pax(0), 20)
  assert.equal(c.frt(0), 4)
  assert.equal(c.src(1), 0, 'domestic stale row is disowned on ordinary quarantine-free terms — the testimony guard does not blunt in-gate healing')
  assert.equal(c.pax(1), 0)
})

test('stamp-only (enableDestructive=false): retract is entirely suppressed — foreign-owned and service rows both survive untouched', async () => {
  // The old design let TWO arms inside writeRailTrains mutate data regardless
  // of enableDestructive: the country-bleed heal (row0 here) and the
  // always-on service-arm auto-heal (row1, now reclassified as a siding while
  // still owned by the retract id — writeRailTrains disowns such rows
  // UNCONDITIONALLY whenever a RailRetract object exists, `when` notwithstanding).
  // Both must now survive in stamp-only, since no RailRetract object is even
  // constructed — country-bleed healing there is deferred to
  // heal-rail-country-bleed.ts or the next destructive run.
  const h3r4Dir = freshScopeDir('stamp-only-no-mutate')
  const gate = (_lat: number, lon: number) => lon < 14.05 // "domestic" = west of 14.05
  putHex(h3r4Dir, HEX_A, [
    { startLat: 50.200, startLon: 15.000, endLat: 50.200, endLon: 15.001, sourceId: 999, pax: 20, frt: 4 }, // row0: wholly foreign, owned by a retract id
    { startLat: 50.201, startLon: 14.040, endLat: 50.201, endLon: 14.041, service: 2, sourceId: 999, pax: 30, frt: 6 }, // row1: domestic but now a siding, owned by the SAME retract id
  ])

  const stats = await enrichRailwaysByGraphWalk({
    h3r4Dir,
    bbox: BBOX,
    pairs: [],
    sourceId: STAMP_ID,
    countryGate: gate,
    bleedGate: gate, // even an explicit bleed gate must stay inert in stamp-only
    retract: { sourceIds: [999] },
    retractSafe: true,
    enableDestructive: false, // stamp-only: retract must be entirely suppressed, incl. the bleed + service-arm heals
    sidecar: { scope: 'stamp-only-scope', extractFingerprint: 'fp-f', feeds: [] },
  })

  assert.equal(stats.retracted, 0, 'no RailRetract object is even passed to the writer in stamp-only mode')

  const c = readCols(resolve(h3r4Dir, HEX_A, 'railways.arrow'))
  assert.equal(c.src(0), 999, 'wholly-foreign row survives — country-bleed healing is deferred to heal-rail-country-bleed.ts in stamp-only')
  assert.equal(c.pax(0), 20)
  assert.equal(c.src(1), 999, 'now-service row survives — the always-on service-arm heal is also suppressed in stamp-only')
  assert.equal(c.pax(1), 30)
})

// ── (e3) foreign-national stamp guard (DE Step A v2 Codex review item 4,
// 2026-07-16): a walk must never overwrite ANOTHER country's nationally-owned
// rows, even where shouldOverwrite's rank/year ladder alone would allow it —
// verified live risk on the widened DE scope: DE 9864 vs CZ 110 is a
// same-rank newer-year win, and CZ 9863 (timetable-silent) is baseline-tier;
// both would fall to the priority gate alone, yet CZ prepared data is FINAL.
// The run's OWN national id keeps updating normally. ─────────────────────────

test('foreign-national guard: rows owned by another country\'s national ids (110 CZPTT, 9863 CZ silent) survive a DE walk crossing them; DE\'s own rows update', async () => {
  const h3r4Dir = freshScopeDir('foreign-national-guard')
  // Four collinear on-path segments + one off-path foreign row (row4) that
  // never gets a stamp candidate and therefore must NOT count as "skipped".
  putHex(h3r4Dir, HEX_A, [
    { startLat: 50.000, startLon: 14.000, endLat: 50.001, endLon: 14.000, sourceId: CZ_NATIONAL_ID, pax: 30, frt: 6 }, // row0: CZ national-measured
    { startLat: 50.001, startLon: 14.000, endLat: 50.002, endLon: 14.000, sourceId: CZ_SILENT_ID, pax: 2, frt: 1 },   // row1: CZ silent residual (baseline tier, nationallyOwned flag)
    { startLat: 50.002, startLon: 14.000, endLat: 50.003, endLon: 14.000, sourceId: DE_NATIONAL_ID, pax: 7, frt: 0 }, // row2: DE's own earlier stamp
    { startLat: 50.003, startLon: 14.000, endLat: 50.004, endLon: 14.000 },                                            // row3: unstamped
    { startLat: 50.900, startLon: 14.000, endLat: 50.901, endLon: 14.000, sourceId: CZ_NATIONAL_ID, pax: 40, frt: 8 }, // row4: foreign-owned but OFF the walked path — no candidate, no skip count
  ])

  const stats = await enrichRailwaysByGraphWalk({
    h3r4Dir,
    bbox: BBOX,
    pairs: [{ fromLat: 50.000, fromLon: 14.000, toLat: 50.004, toLon: 14.000, pax: 16, frt: 0 }],
    sourceId: DE_NATIONAL_ID,
    retractSafe: true,
    enableDestructive: false, // guard applies in EVERY mode — stamp-only included
    sidecar: { scope: 'foreign-national-guard-scope', extractFingerprint: 'fp-fng', feeds: [] },
  })

  assert.equal(stats.skippedForeignNational, 2, 'rows 0+1 had a walk candidate withheld; off-path row4 has no candidate and never counts')
  assert.equal(stats.stamped, 2, 'rows 2+3 — DE\'s own row re-stamps, the empty row stamps fresh')

  const c = readCols(resolve(h3r4Dir, HEX_A, 'railways.arrow'))
  assert.equal(c.src(0), CZ_NATIONAL_ID, 'CZ national-measured row survives — shouldOverwrite(110, 9864) alone is a newer-year WIN for DE, only the guard protects it')
  assert.equal(c.pax(0), 30)
  assert.equal(c.frt(0), 6)
  assert.equal(c.src(1), CZ_SILENT_ID, 'CZ silent-residual row survives — baseline tier, nationally owned via the registry flag')
  assert.equal(c.pax(1), 2)
  assert.equal(c.frt(1), 1)
  assert.equal(c.src(2), DE_NATIONAL_ID, 'DE\'s OWN row is not foreign — updates normally')
  assert.equal(c.pax(2), 16, 'walk count replaces the earlier own stamp')
  assert.equal(c.src(3), DE_NATIONAL_ID, 'empty row stamps fresh')
  assert.equal(c.pax(3), 16)
  assert.equal(c.src(4), CZ_NATIONAL_ID, 'off-path foreign row untouched')
  assert.equal(c.pax(4), 40)
})

// ── (f) sidecar: zero-snap semantics (Codex review item 5, 2026-07-16 — a
// run with failure records writes them even when ZERO stops snapped; only a
// walk with neither stops nor failures writes nothing) ──────────────────────

test('sidecar: written with EMPTY stops but full failure diagnostics when zero pair endpoints snap onto the graph', async (t) => {
  const h3r4Dir = freshScopeDir('sidecar-empty')
  putHex(h3r4Dir, HEX_A, [
    { startLat: 50.300, startLon: 14.300, endLat: 50.301, endLon: 14.300 },
  ])

  const errors: string[] = []
  t.mock.method(console, 'error', (msg: string) => { errors.push(String(msg)) })

  const stats = await enrichRailwaysByGraphWalk({
    h3r4Dir,
    bbox: BBOX,
    pairs: [{ fromLat: 10.0, fromLon: 10.0, toLat: 10.001, toLon: 10.0, pax: 5, frt: 0 }], // nowhere near the graph
    sourceId: STAMP_ID,
    retractSafe: true,
    enableDestructive: false,
    sidecar: { scope: 'empty-scope', extractFingerprint: 'fp-empty', feeds: [] },
  })

  assert.equal(stats.walk.failures.snapFailed, 1)
  // Codex item 5: the all-endpoints-missed run is precisely the one whose
  // diagnostics must survive — the old stops-only early return dropped them.
  assert.notEqual(stats.sidecarPath, '', 'failure records exist — the sidecar IS written')
  const sidecar = JSON.parse(readFileSync(stats.sidecarPath, 'utf8'))
  assert.deepEqual(sidecar.stops, [], 'zero stops snapped')
  assert.equal(sidecar.failedPairChords.length, 1)
  assert.equal(sidecar.failedPairChords[0].reason, 'snapFailed')
  assert.ok(errors.some((e) => e.includes('EMPTIED')), 'a loud warning says the stop-exemption evidence for this scope is empty')
})

test('sidecar: a walk with NEITHER snapped stops NOR failure records writes nothing (no pairs at all)', async () => {
  const h3r4Dir = freshScopeDir('sidecar-nothing')
  putHex(h3r4Dir, HEX_A, [
    { startLat: 50.300, startLon: 14.300, endLat: 50.301, endLon: 14.300 },
  ])

  const stats = await enrichRailwaysByGraphWalk({
    h3r4Dir,
    bbox: BBOX,
    pairs: [],
    sourceId: STAMP_ID,
    retractSafe: true,
    enableDestructive: false,
    sidecar: { scope: 'nothing-scope', extractFingerprint: 'fp-nothing', feeds: [] },
  })

  assert.equal(stats.sidecarPath, '', 'nothing to record — nothing written')
  assert.ok(!existsSync(resolve(h3r4Dir, '..', 'rail-stops', 'nothing-scope.json')))
})

test('sidecar: a zero-snap run with failure records REPLACES an earlier sidecar (empty stops + diagnostics) and warns loudly; a no-pairs run leaves it untouched', async (t) => {
  const h3r4Dir = freshScopeDir('sidecar-stale')
  putHex(h3r4Dir, HEX_A, [
    { startLat: 50.400, startLon: 14.400, endLat: 50.401, endLon: 14.400 },
  ])

  // First run: a real pair snaps and writes the sidecar.
  const first = await enrichRailwaysByGraphWalk({
    h3r4Dir,
    bbox: BBOX,
    pairs: [{ fromLat: 50.400, fromLon: 14.400, toLat: 50.401, toLon: 14.400, pax: 5, frt: 0 }],
    sourceId: STAMP_ID,
    retractSafe: true,
    enableDestructive: false,
    sidecar: { scope: 'stale-scope', extractFingerprint: 'fp-stale-1', feeds: ['feedX'] },
  })
  assert.notEqual(first.sidecarPath, '')
  const sidecarBefore = readFileSync(first.sidecarPath, 'utf8')

  // Second run, SAME scope, NO pairs at all: nothing written, the earlier
  // sidecar silently remains in force — with the STALE warning (unchanged
  // pre-item-5 semantics for this sub-case).
  const errors: string[] = []
  t.mock.method(console, 'error', (msg: string) => { errors.push(String(msg)) })

  const second = await enrichRailwaysByGraphWalk({
    h3r4Dir,
    bbox: BBOX,
    pairs: [],
    sourceId: STAMP_ID,
    retractSafe: true,
    enableDestructive: false,
    sidecar: { scope: 'stale-scope', extractFingerprint: 'fp-stale-2', feeds: ['feedX'] },
  })
  assert.equal(second.sidecarPath, '', 'no pairs — this run writes nothing')
  assert.ok(errors.some((e) => e.includes('STALE')), 'a loud warning names the stale sidecar')
  assert.equal(readFileSync(first.sidecarPath, 'utf8'), sidecarBefore, 'the earlier sidecar is left byte-identical')

  // Third run, SAME scope, pairs that snap onto nothing: failure diagnostics
  // exist, so the sidecar IS rewritten — empty stops (R15/R16 evidence for
  // this scope goes dark until a healthy re-run, said loudly) + the records.
  errors.length = 0
  const third = await enrichRailwaysByGraphWalk({
    h3r4Dir,
    bbox: BBOX,
    pairs: [{ fromLat: 10.0, fromLon: 10.0, toLat: 10.001, toLon: 10.0, pax: 5, frt: 0 }], // nowhere near the graph
    sourceId: STAMP_ID,
    retractSafe: true,
    enableDestructive: false,
    sidecar: { scope: 'stale-scope', extractFingerprint: 'fp-stale-3', feeds: ['feedX'] },
  })
  assert.equal(third.sidecarPath, first.sidecarPath, 'same scope — same file, rewritten')
  const rewritten = JSON.parse(readFileSync(first.sidecarPath, 'utf8'))
  assert.deepEqual(rewritten.stops, [], 'the earlier stop evidence was replaced by this run\'s (empty) truth')
  assert.equal(rewritten.extractFingerprint, 'fp-stale-3')
  assert.equal(rewritten.failedPairChords.length, 1)
  assert.ok(errors.some((e) => e.includes('EMPTIED')), 'the replacement is announced loudly')
})

// ── (g) silent residual: divisor rides from the graph's own lateral spread ──
// (2026-07-16 /gg review item 3 — the fix for the "silent divisor loss" bug:
// a silent-stamped row with a parallel sibling must render at its true
// divided count, not the engine's undivided default.)

test('silent residual: a sibling-bearing pair of rows (never walked) both get divisor 2 from the graph\'s own lateral spread', async () => {
  const h3r4Dir = freshScopeDir('silent-divisor-siblings')
  const east = 18.000 + 8 / (111_320 * Math.cos(54 * Math.PI / 180)) // ~8 m east — genuine double-track spacing
  putHex(h3r4Dir, HEX_A, [
    { startLat: 54.000, startLon: 18.000, endLat: 54.001, endLon: 18.000 }, // row0
    { startLat: 54.000, startLon: east, endLat: 54.001, endLon: east },     // row1 — lateral sibling of row0
  ])

  const stats = await enrichRailwaysByGraphWalk({
    h3r4Dir,
    bbox: BBOX,
    pairs: [], // neither row is ever walked
    sourceId: STAMP_ID,
    silentResidual: { sourceId: GLOBAL_GTFS_ID, pax: 2, frt: 1 },
    retractSafe: true,
    enableDestructive: true,
    sidecar: { scope: 'silent-divisor-siblings-scope', extractFingerprint: 'fp-g', feeds: [] },
  })

  assert.equal(stats.stamped, 0, 'nothing walked — pairs is empty')
  assert.equal(stats.silentStamped, 2, 'both rows are eligible — nothing ever failed, so every component is failure-free')

  const c = readCols(resolve(h3r4Dir, HEX_A, 'railways.arrow'))
  assert.equal(c.src(0), GLOBAL_GTFS_ID); assert.equal(c.pax(0), 2); assert.equal(c.frt(0), 1)
  assert.equal(c.src(1), GLOBAL_GTFS_ID); assert.equal(c.pax(1), 2); assert.equal(c.frt(1), 1)
  assert.equal(c.div(0), 2, 'divisor comes from divisorBySegmentKey — recorded for sibling-bearing rows independent of whether the WALK ever stamped them')
  assert.equal(c.div(1), 2)
})

test('second identical run over unchanged silent-eligible siblings is a byte-no-op (divisor preserved, not re-touched)', async () => {
  const h3r4Dir = freshScopeDir('idempotent-silent-rerun')
  const east = 18.000 + 8 / (111_320 * Math.cos(54 * Math.PI / 180))
  putHex(h3r4Dir, HEX_A, [
    { startLat: 54.000, startLon: 18.000, endLat: 54.001, endLon: 18.000 },
    { startLat: 54.000, startLon: east, endLat: 54.001, endLon: east },
  ])
  const path = resolve(h3r4Dir, HEX_A, 'railways.arrow')

  const opts = {
    h3r4Dir, bbox: BBOX, pairs: [],
    sourceId: STAMP_ID,
    silentResidual: { sourceId: GLOBAL_GTFS_ID, pax: 2, frt: 1 },
    retractSafe: true,
    enableDestructive: true,
    sidecar: { scope: 'idempotent-silent-scope', extractFingerprint: 'fp-idem', feeds: [] },
  }

  const stats1 = await enrichRailwaysByGraphWalk(opts)
  assert.equal(stats1.silentStamped, 2)
  const afterFirstRun = readFileSync(path)

  const stats2 = await enrichRailwaysByGraphWalk(opts)
  assert.equal(stats2.silentStamped, 2, 'accepted-ness contract: still counts as a match even though nothing changed')
  const afterSecondRun = readFileSync(path)
  assert.deepEqual(afterSecondRun, afterFirstRun, 'byte-identical re-run: divisor + stamp already correct, no rewrite')
})

// ── (h) an unlocalized pair quarantines only its own chord vicinity — NOT the
// whole run, and not by component either (2026-07-16 Step-B refinement
// replaced the old global silent-residual suppression: it quarantined every
// clean row in a run behind one unrelated stray pair). ──────────────────────

test('an unlocalized pair (neither end snaps) quarantines only stampable segments within 5 km of its own straight chord — a row far from the chord still gets silently stamped', async () => {
  const h3r4Dir = freshScopeDir('unlocalized-chord-vicinity')
  putHex(h3r4Dir, HEX_A, [
    { startLat: 10.010, startLon: 10.000, endLat: 10.011, endLon: 10.000 }, // row0 — sits ON the pair's own chord, INSIDE the 5 km quarantine radius
    { startLat: 55.000, startLon: 19.000, endLat: 55.001, endLon: 19.000 }, // row1 — thousands of km from the chord, untouched
  ])

  const stats = await enrichRailwaysByGraphWalk({
    h3r4Dir,
    bbox: BBOX,
    // Neither end of this pair is anywhere near either row's own nodes —
    // unlocalizable (both ends fail to snap).
    pairs: [{ fromLat: 10.000, fromLon: 10.000, toLat: 11.000, toLon: 10.000, pax: 5, frt: 0 }],
    sourceId: STAMP_ID,
    silentResidual: { sourceId: GLOBAL_GTFS_ID, pax: 2, frt: 1 },
    retractSafe: true,
    enableDestructive: true,
    sidecar: { scope: 'unlocalized-chord-scope', extractFingerprint: 'fp-h', feeds: [] },
  })

  assert.equal(stats.walk.unlocalizedPairs, 1)
  assert.equal(stats.silentStamped, 1, 'only row1 (far from the chord) is silent-eligible — row0 sits inside the 5 km quarantine vicinity')

  const c = readCols(resolve(h3r4Dir, HEX_A, 'railways.arrow'))
  assert.equal(c.src(0), 0, 'row0 quarantined by chord vicinity — stays unstamped')
  assert.equal(c.src(1), GLOBAL_GTFS_ID, 'row1 far from the chord — silently stamped like any other clean row, NOT suppressed by the unrelated unlocalized pair')
})
