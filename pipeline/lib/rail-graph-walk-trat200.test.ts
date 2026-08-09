/**
 * Real-data regression test for the trať 200 banding bug (2026-07-15 railway
 * enrichment fix) — the fixture this locks in is
 * `pipeline/lib/fixtures/trat200-milin-breznice.json`: real
 * `railways.arrow` segments + real CZPTT station pairs from the
 * Březnice-Příbram-Jince corridor, WITH their pre-fix stamps (source 110 /
 * 9863) preserved so this test can verify the graph-walk actually corrects
 * the checkerboard those two sources left behind, not just that it runs.
 *
 * Read-only fixture (skipped gracefully if unreadable in a sandbox — see
 * `t.skip` below). Numbers asserted here were COMPUTED by actually running
 * `buildRailGraph`/`walkRailStationPairs` against this exact fixture (never
 * estimated — AGENTS.md rule), and are smaller than the plan's aspirational
 * "pairsWalked >= 15" figure: only 5 of the fixture's 28 raw CZPTT pair
 * entries have BOTH endpoints GPS-resolved (the rest are pseudo-locations —
 * "Km 67,500", "vl. v km 75,245", "odb.výh.101", or genuinely un-tagged OSM
 * stations like "Jince"/"Hudčice"), and those 10 raw (5 forward + 5 reverse)
 * canonicalize into exactly 5 station-pairs. This is a real property of the
 * committed fixture slice (one train's round trip + one short local hop), not
 * a bug in the walk — see the diagnostic numbers in this test's own asserts.
 *
 * Run: `cd pipeline && npx tsx --test lib/rail-graph-walk-trat200.test.ts`
 */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { buildRailGraph, type RailGraphSegmentInput } from './rail-graph.js'
import { walkRailStationPairs } from './rail-graph-metrics.js'

const FIXTURE_PATH = resolve(import.meta.dirname, 'fixtures', 'trat200-milin-breznice.json')

interface FixtureSegment {
  hexId: string
  rowIdx: number
  osmId: string
  railType: number
  usage: number
  service: number
  ref: string
  name: string
  startLat: number
  startLon: number
  endLat: number
  endLon: number
  lengthM: number
  trainsPassenger: number
  trainsFreight: number
  sourceId: number
  parallelDivisor: number
}

interface FixturePair {
  key: string
  fromName: string
  toName: string
  fromGps: { lat: number; lon: number } | null
  toGps: { lat: number; lon: number } | null
  passenger: number
  freight: number
}

interface Fixture {
  segments: FixtureSegment[]
  pairs: FixturePair[]
}

test('trať 200 real-corridor regression: the graph-walk corrects the pre-fix 110/9863 checkerboard into a uniform per-hop stamp', (t) => {
  let fixture: Fixture
  try {
    fixture = JSON.parse(readFileSync(FIXTURE_PATH, 'utf8')) as Fixture
  } catch {
    t.skip(`fixture unreadable in this sandbox: ${FIXTURE_PATH}`)
    return
  }

  // ── Build the graph exactly like rail-walk-enrich.ts::collectGraphSegments ──
  const segments: RailGraphSegmentInput[] = []
  for (const s of fixture.segments) {
    const isTraversalOnly = s.railType === 0 && s.service === 4
    if (!isTraversalOnly && !(s.railType === 0 && s.service === 0)) continue // never in the graph
    segments.push({
      key: `${s.hexId}:${s.rowIdx}`,
      osmId: s.osmId,
      railType: s.railType,
      usage: s.usage,
      isTraversalOnly,
      corridorToken: s.ref || s.name || '',
      startLat: s.startLat, startLon: s.startLon, endLat: s.endLat, endLon: s.endLon,
      lengthM: s.lengthM,
    })
  }
  const graph = buildRailGraph(segments)

  // ── Form pairs: only fixture.pairs entries with BOTH endpoints GPS-resolved ──
  const pairs = fixture.pairs
    .filter((p) => p.fromGps && p.toGps)
    .map((p) => ({
      fromLat: p.fromGps!.lat, fromLon: p.fromGps!.lon,
      toLat: p.toGps!.lat, toLon: p.toGps!.lon,
      pax: p.passenger, frt: p.freight,
    }))

  const result = walkRailStationPairs(graph, pairs)

  // (a) Every walkable pair on this corridor succeeds cleanly — no ambiguity,
  // no detour rejection (and, more strongly, no disconnection/snap failure
  // either: this real corridor slice is fully clean end to end). Only 5
  // canonical pairs exist in this fixture slice (see module doc) — the
  // plan's "pairsWalked >= 15" describes a fuller timetable than this
  // committed fixture happens to carry; the honest, computed number is 5/5.
  assert.equal(result.pairsTotal, 5, 'this fixture slice canonicalizes to exactly 5 station-pairs (10 raw both-GPS entries, 5 forward+reverse)')
  assert.equal(result.pairsWalked, 5, 'all 5 walk cleanly')
  assert.equal(result.failures.ambiguous, 0)
  assert.equal(result.failures.detourRejected, 0)
  assert.equal(result.failures.disconnected, 0)
  assert.equal(result.failures.snapFailed, 0)
  // 2026-07-16 Step-B refinement: zero failed pairs on this corridor slice
  // means zero quarantine — the per-pair quarantine shapes (rail-graph-
  // metrics.ts's quarantineAdmissiblePathEllipse / quarantineEndpointRadius)
  // never fire without a failure to localize in the first place.
  assert.equal(result.quarantinedSegmentKeys.size, 0, 'no failed pairs on this corridor slice — nothing quarantined')

  // ── (b) The walk corrects rows on BOTH sides of the old 110/9863 seam ──
  const oldStampByKey = new Map<string, { sourceId: number; pax: number }>()
  for (const s of fixture.segments) {
    if (s.railType === 0 && s.service === 0 && (s.sourceId === 110 || s.sourceId === 9863)) {
      oldStampByKey.set(`${s.hexId}:${s.rowIdx}`, { sourceId: s.sourceId, pax: s.trainsPassenger })
    }
  }
  assert.equal(oldStampByKey.size, 616, 'fixture sanity: 616 mainline rows carried a pre-fix 110/9863 stamp')

  let previously110 = 0, previously9863 = 0, freshlyTouchedUnstamped = 0
  const newPaxValues = new Set<number>()
  const oldPaxValuesTouched = new Set<number>()
  for (const [key, stamp] of result.stampsBySegmentKey) {
    const old = oldStampByKey.get(key)
    if (!old) { freshlyTouchedUnstamped++; continue }
    if (old.sourceId === 110) previously110++
    else previously9863++
    newPaxValues.add(stamp.pax)
    oldPaxValuesTouched.add(old.pax)
  }

  assert.equal(result.stampsBySegmentKey.size, 133, 'exactly the rows physically spanned by the 5 walked OD pairs')
  assert.equal(previously110, 117, 'rows the OLD cz-szcd-gtfs chord matcher (110) had stamped, now corrected')
  assert.equal(previously9863, 16, 'rows the OLD timetable-silent residual (9863) had stamped, now corrected — CROSSING the old source boundary the banding bug straddled')
  assert.equal(freshlyTouchedUnstamped, 0, 'the walk never spuriously stamps virgin (never-110/9863) territory on this corridor')

  // (c) Uniformity — the anti-banding assertion: every corrected row's NEW
  // value is one of exactly two per-hop totals, each the sum of both
  // directions of that hop's real CZPTT pair count (16+16=32 for the
  // Tochovice/Ostrov/Milín/Příbram-sídliště hops; 15+15=30 for the
  // Dobrá-Voda-u-Březnice/Březnice hop) — never the old per-row raw values.
  assert.deepEqual([...newPaxValues].sort((a, b) => a - b), [30, 32])
  const the16Plus16Pair = fixture.pairs.find((p) => p.key === '76944-76914') // Ostrov u Tochovic -> Milín, 16 each way
  assert.ok(the16Plus16Pair)
  assert.equal(the16Plus16Pair!.passenger, 16)
  assert.equal(32, the16Plus16Pair!.passenger * 2, 'the task\'s own worked example: 32 = 16 (one direction) + 16 (the other)')

  // (d) Zero corridor rows retain the OLD banded raw signature after the fix
  // — the corrected rows' pre-fix values (15, 16 from source 110; 2 from the
  // 9863 silent residual) are a DISJOINT set from the new uniform values
  // (30, 32): every touched row's stamp changed value, none is a no-op that
  // would have left the checkerboard in place.
  assert.deepEqual([...oldPaxValuesTouched].sort((a, b) => a - b), [2, 15, 16], 'fixture sanity: the OLD alternating pattern really was there pre-fix')
  for (const oldValue of oldPaxValuesTouched) {
    assert.ok(!newPaxValues.has(oldValue), `old banded value ${oldValue} must not survive as a NEW stamped value`)
  }
})
