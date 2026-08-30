/**
 * Routing, parallel-spread and R15/R16 detector tests for
 * rail-graph-metrics.ts. Builds graphs via `buildRailGraph` (rail-graph.ts)
 * and exercises `walkRailStationPairs` / `findRailFlowJumps` /
 * `findRailContinuityGaps` end to end — see rail-graph.test.ts for pure
 * graph-construction tests (T-junction healing topology, snap, effective
 * traffic table).
 *
 * Run: `cd pipeline && npx tsx --test lib/rail-graph-metrics.test.ts`
 */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { flatDist } from './spatial.js'
import { buildRailGraph, snapToNearestRailGraphNode, effectiveRailTraffic, buildRailStopsIndex, type RailGraphSegmentInput, type RailEndpointRow } from './rail-graph.js'
import {
  walkRailStationPairs, findRailFlowJumps, findRailContinuityGaps,
  dijkstraShortestPath, createDijkstraScratch,
} from './rail-graph-metrics.js'

function seg(over: Partial<RailGraphSegmentInput> & Pick<RailGraphSegmentInput, 'key' | 'startLat' | 'startLon' | 'endLat' | 'endLon'>): RailGraphSegmentInput {
  const lengthM = flatDist(over.startLat, over.startLon, over.endLat, over.endLon)
  return {
    osmId: over.key, railType: 0, usage: 0, isTraversalOnly: false, corridorToken: '',
    lengthM, ...over,
  }
}

function componentOfSegment(graph: ReturnType<typeof buildRailGraph>, key: string): number {
  const edge = graph.edges.find((candidate) => candidate.parentKey === key)
  assert.ok(edge, `missing graph edge for segment ${key}`)
  return graph.componentOfNode[edge.nodeA]
}

// ── T-junction healing: stamps map back to the ORIGINAL parent key ─────────

test('walk: a path crossing a healed T-junction stamps the parent key once, not once per sub-edge', () => {
  const trunk = seg({ key: 'trunk', startLat: 50, startLon: 14.000, endLat: 50, endLon: 14.010 })
  const branch = seg({ key: 'branch', startLat: 50, startLon: 14.005, endLat: 50.005, endLon: 14.005 })
  const g = buildRailGraph([trunk, branch])
  assert.equal(g.edges.filter((e) => e.parentKey === 'trunk').length, 2, 'fixture sanity: trunk is healed into 2 sub-edges')

  const result = walkRailStationPairs(g, [
    { fromLat: 50, fromLon: 14.000, toLat: 50, toLon: 14.010, pax: 20, frt: 0 },
  ])
  assert.equal(result.failures.snapFailed + result.failures.disconnected + result.failures.detourRejected + result.failures.ambiguous, 0)
  assert.equal(result.pairsWalked, 1)
  assert.equal(result.stampsBySegmentKey.size, 1, 'ONE entry for the trunk despite crossing 2 healed sub-edges')
  assert.deepEqual(result.stampsBySegmentKey.get('trunk'), { pax: 20, frt: 0, divisor: 1 })
  assert.equal(result.stampsBySegmentKey.has('branch'), false, 'branch was never on this path')
})

// ── Traversal-only crossover: connects, never stamped ───────────────────────

test('walk: a traversal-only crossover carries the route but never appears in stampsBySegmentKey', () => {
  const crossover = seg({ key: 'crossXY', startLat: 50, startLon: 14.000, endLat: 50, endLon: 14.001, isTraversalOnly: true })
  const g = buildRailGraph([crossover])
  const result = walkRailStationPairs(g, [
    { fromLat: 50, fromLon: 14.000, toLat: 50, toLon: 14.001, pax: 30, frt: 10 },
  ])
  assert.equal(result.pairsWalked, 1, 'the crossover carries the route (routable)')
  assert.equal(result.stampsBySegmentKey.size, 0, 'but is never itself the recipient of a stamp')
})

// ── Detour gate + meander ────────────────────────────────────────────────────

test('walk: detour gate rejects a route with no reasonable alternative to the huge-detour-only path', () => {
  // The ONLY connection between A and B detours ~110 km south — nowhere near
  // 2.5x the ~7.2 km chord + 2 km slack.
  const a = seg({ key: 'a-p', startLat: 50.000, startLon: 14.000, endLat: 49.000, endLon: 14.050 })
  const b = seg({ key: 'p-b', startLat: 49.000, startLon: 14.050, endLat: 50.000, endLon: 14.100 })
  const g = buildRailGraph([a, b])
  const result = walkRailStationPairs(g, [
    { fromLat: 50.000, fromLon: 14.000, toLat: 50.000, toLon: 14.100, pax: 5, frt: 0 },
  ])
  assert.equal(result.failures.detourRejected, 1)
  assert.equal(result.pairsWalked, 0)
  assert.equal(result.stampsBySegmentKey.size, 0)
})

test('walk: a meander well within the detour gate is fully stamped on every edge', () => {
  // Only path from A to B detours ~1.5 km north of the direct line — longer
  // than the ~7.2 km chord but nowhere near the 2.5x+2km bound (~19.9 km).
  const a = seg({ key: 'a-m', startLat: 50.000, startLon: 14.000, endLat: 49.985, endLon: 14.050 })
  const b = seg({ key: 'm-b', startLat: 49.985, startLon: 14.050, endLat: 50.000, endLon: 14.100 })
  const g = buildRailGraph([a, b])
  const result = walkRailStationPairs(g, [
    { fromLat: 50.000, fromLon: 14.000, toLat: 50.000, toLon: 14.100, pax: 12, frt: 3 },
  ])
  assert.equal(result.pairsWalked, 1)
  assert.deepEqual(result.stampsBySegmentKey.get('a-m'), { pax: 12, frt: 3, divisor: 1 })
  assert.deepEqual(result.stampsBySegmentKey.get('m-b'), { pax: 12, frt: 3, divisor: 1 })
})

// ── Direction sum + express/local summation ─────────────────────────────────

test('walk: A->B and B->A on the same OD SUM into one canonical pair (engine wants total trains/day)', () => {
  const ab = seg({ key: 'ab', startLat: 50, startLon: 14.000, endLat: 50, endLon: 14.020 })
  const g = buildRailGraph([ab])
  const result = walkRailStationPairs(g, [
    { fromLat: 50, fromLon: 14.000, toLat: 50, toLon: 14.020, pax: 16, frt: 0 },
    { fromLat: 50, fromLon: 14.020, toLat: 50, toLon: 14.000, pax: 16, frt: 0 }, // reverse direction
  ])
  assert.equal(result.pairsTotal, 1, 'both directions canonicalize into ONE pair')
  assert.deepEqual(result.stampsBySegmentKey.get('ab'), { pax: 32, frt: 0, divisor: 1 })
})

test('walk: express + local pairs on a shared trunk edge SUM (different OD pairs, same track)', () => {
  const am = seg({ key: 'AM', startLat: 50, startLon: 14.000, endLat: 50, endLon: 14.010 })
  const mc = seg({ key: 'MC', startLat: 50, startLon: 14.010, endLat: 50, endLon: 14.020 })
  const g = buildRailGraph([am, mc])
  const result = walkRailStationPairs(g, [
    { fromLat: 50, fromLon: 14.000, toLat: 50, toLon: 14.010, pax: 5, frt: 0 },  // local: A-M only
    { fromLat: 50, fromLon: 14.000, toLat: 50, toLon: 14.020, pax: 8, frt: 0 },  // express: A-C via AM+MC
  ])
  assert.equal(result.pairsTotal, 2, 'two distinct OD pairs, not merged')
  assert.deepEqual(result.stampsBySegmentKey.get('AM'), { pax: 13, frt: 0, divisor: 1 }, 'local + express both cross AM')
  assert.deepEqual(result.stampsBySegmentKey.get('MC'), { pax: 8, frt: 0, divisor: 1 }, 'only express reaches MC')
})

// ── Ambiguity ────────────────────────────────────────────────────────────────

function buildTwoCorridorGraph(): { g: ReturnType<typeof buildRailGraph> } {
  // North route via N (~1.1 km north of the direct line) and south route via
  // S (~1.1 km south) are near-mirror-image in length and share zero edges.
  const an = seg({ key: 'A-N', startLat: 50.000, startLon: 14.000, endLat: 50.010, endLon: 14.050 })
  const nb = seg({ key: 'N-B', startLat: 50.010, startLon: 14.050, endLat: 50.000, endLon: 14.100 })
  const as_ = seg({ key: 'A-S', startLat: 50.000, startLon: 14.000, endLat: 49.990, endLon: 14.050 })
  const sb = seg({ key: 'S-B', startLat: 49.990, startLon: 14.050, endLat: 50.000, endLon: 14.100 })
  return { g: buildRailGraph([an, nb, as_, sb]) }
}

test('walk: two competing similar-length disjoint corridors fail as ambiguous (no guessing)', () => {
  const { g } = buildTwoCorridorGraph()
  const result = walkRailStationPairs(g, [
    { fromLat: 50.000, fromLon: 14.000, toLat: 50.000, toLon: 14.100, pax: 7, frt: 0 },
  ])
  assert.equal(result.failures.ambiguous, 1)
  assert.equal(result.pairsWalked, 0)
  assert.equal(result.stampsBySegmentKey.size, 0)
})

// DE Step A v2 diagnostics (2026-07-16 failure analysis, fix 3): an
// 'ambiguous' failure record must carry the alt-vs-best path geometry
// summary — the v3 twin-gate tuning input (see summarizeAmbiguousGeometry's
// doc, rail-graph-metrics.ts). Reuses the SAME two-mirror-corridor fixture
// as the test above (~1.1 km north / ~1.1 km south of the direct chord —
// clearly not a parallel-twin double-track), so the alt path's lateral
// spread from the best path must be on the order of kilometres, not metres.
test('walk: an ambiguous failure record carries the alt-path geometry summary (lateral spread + heading delta) for v3 twin-gate tuning', () => {
  const { g } = buildTwoCorridorGraph()
  const result = walkRailStationPairs(g, [
    { fromLat: 50.000, fromLon: 14.000, toLat: 50.000, toLon: 14.100, pax: 7, frt: 0 },
  ])
  assert.equal(result.failures.ambiguous, 1)
  assert.equal(result.failedPairChords.length, 1)
  const rec = result.failedPairChords[0]
  assert.equal(rec.reason, 'ambiguous')
  assert.equal(rec.snapDistanceM, undefined, 'diagnostics are reason-specific — ambiguous never carries snapDistanceM')
  assert.ok(rec.ambiguousGeometry, 'ambiguous record must carry the geometry summary')
  const { lateralSpreadM, headingDeltaDeg } = rec.ambiguousGeometry!
  assert.ok(
    lateralSpreadM.min <= lateralSpreadM.median && lateralSpreadM.median <= lateralSpreadM.max,
    `min <= median <= max (got ${JSON.stringify(lateralSpreadM)})`,
  )
  assert.ok(lateralSpreadM.min > 500, 'the two mirror corridors sit ~2.2 km apart at their widest — nowhere near a parallel-twin spacing (tens of metres)')
  assert.ok(headingDeltaDeg >= 0 && headingDeltaDeg <= 90, 'heading delta is folded mod 180, so it never exceeds 90')
})

test('walk: a shapePolyline hugging one corridor disambiguates it (ambiguity probe skipped)', () => {
  const { g } = buildTwoCorridorGraph()
  const result = walkRailStationPairs(g, [
    {
      fromLat: 50.000, fromLon: 14.000, toLat: 50.000, toLon: 14.100, pax: 7, frt: 0,
      shapePolyline: [[50.000, 14.000], [50.010, 14.050], [50.000, 14.100]], // hugs the north route
    },
  ])
  assert.equal(result.failures.ambiguous, 0)
  assert.equal(result.pairsWalked, 1)
  assert.deepEqual(result.stampsBySegmentKey.get('A-N'), { pax: 7, frt: 0, divisor: 1 })
  assert.deepEqual(result.stampsBySegmentKey.get('N-B'), { pax: 7, frt: 0, divisor: 1 })
  assert.equal(result.stampsBySegmentKey.has('A-S'), false, 'south corridor excluded by the shape constraint')
})

// ── Twin-track ambiguity exemption (2026-07-16 Step-B refinement, plan item 1:
// verified on live CZ Step-A data — 113 of 150 failed pairs were double-track
// lines the ambiguity probe mistook for a genuine second corridor). A
// double-track line joined at both ends via crossovers produces exactly this
// shape: the direct track is shorter (no crossover detour) and wins as
// `best`; the ONLY alternate route once `best`'s edges are penalized is
// crossover -> sibling track -> crossover, fully disjoint from `best` and
// near-equal length — genuinely ambiguous-LOOKING, but the sibling passes the
// parallel-spread's own lateral-twin gate, so it is not a different corridor
// at all. ─────────────────────────────────────────────────────────────────

function buildCrossoverJoinedDoubleTrack(offsetM: number) {
  const offsetDeg = offsetM / 110_540
  const track1 = seg({ key: 'track1', startLat: 50.000, startLon: 14.000, endLat: 50.000, endLon: 14.030 })
  const track2 = seg({ key: 'track2', osmId: 'osmTrack2', startLat: 50.000 + offsetDeg, startLon: 14.000, endLat: 50.000 + offsetDeg, endLon: 14.030 })
  // Crossovers (service=crossover -> isTraversalOnly): connect the two tracks
  // at each end so the SAME two graph nodes (A/B, snapped by the query below)
  // admit two topologically distinct routes — a real double-track's own
  // switches, not a fixture artifact.
  const crossIn = seg({ key: 'crossIn', startLat: 50.000, startLon: 14.000, endLat: 50.000 + offsetDeg, endLon: 14.000, isTraversalOnly: true })
  const crossOut = seg({ key: 'crossOut', startLat: 50.000, startLon: 14.030, endLat: 50.000 + offsetDeg, endLon: 14.030, isTraversalOnly: true })
  return buildRailGraph([track1, track2, crossIn, crossOut])
}

test('walk: a token-less double-track joined at both ends via crossovers is NOT ambiguous — walks the direct track and spreads to the sibling', () => {
  const g = buildCrossoverJoinedDoubleTrack(8) // ~8 m lateral — genuine double-track spacing
  const result = walkRailStationPairs(g, [
    { fromLat: 50.000, fromLon: 14.000, toLat: 50.000, toLon: 14.030, pax: 20, frt: 4 },
  ])
  assert.equal(result.failures.ambiguous, 0, 'the sibling-track alt path is exempted as a parallel twin, not a genuine second corridor')
  assert.equal(result.pairsWalked, 1)
  assert.deepEqual(result.stampsBySegmentKey.get('track1'), { pax: 20, frt: 4, divisor: 2 }, 'walked directly')
  assert.deepEqual(result.stampsBySegmentKey.get('track2'), { pax: 20, frt: 4, divisor: 2 }, 'unwalked sibling still spreads — the parallel-track pass runs exactly as normal once the pair is no longer failed')
})

test('walk: two corridors ~100 m+ apart (beyond even the confirmed-token 50 m radius) are genuinely disjoint and STAY ambiguous', () => {
  const g = buildCrossoverJoinedDoubleTrack(120) // ~120 m — real distinct-corridor spacing, not a double-track
  const result = walkRailStationPairs(g, [
    { fromLat: 50.000, fromLon: 14.000, toLat: 50.000, toLon: 14.030, pax: 20, frt: 4 },
  ])
  assert.equal(result.failures.ambiguous, 1, 'no lateral-twin gate passes at 120 m — a genuinely different corridor, not a sibling track')
  assert.equal(result.pairsWalked, 0)
  assert.equal(result.stampsBySegmentKey.size, 0)
})

test('twin exemption is LENGTH-weighted (item 5): 8 short twin stubs + 2 long ~100 m-lateral edges stay ambiguous', () => {
  // The reviewer's repro: the alt path carries 8 x ~8 m sibling stubs at 8 m
  // offset plus 2 long edges (~659 m + ~723 m) bulging ~100 m laterally off
  // the best track. By EDGE COUNT the twins are 8/10 = 0.8 — the pre-item-5
  // bug exempted this as a parallel twin. LENGTH-weighted, ~96 % of the alt
  // sits at the long edges' ~58 m midpoint lateral, so the quantile gate's
  // median (~58 m) exceeds WALK_TWIN_MEDIAN_LATERAL_M = 50 and the pair
  // must STAY ambiguous (deliberately close to the threshold — this repro
  // marks the boundary between an S-Bahn-style parallel system and a
  // separate corridor).
  const latOff8 = 50 + 8 / 110_540           // sibling track ~8 m north of the best track
  const latOff108 = latOff8 + 100 / 110_540  // the bulge apex ~108 m north
  const dLon8 = 8 / (111_320 * Math.cos(50 * Math.PI / 180)) // ~8 m of longitude at lat 50
  const altSegs = []
  for (let i = 0; i < 8; i++) {
    altSegs.push(seg({
      key: `stub${i}`, osmId: 'osmAlt',
      startLat: latOff8, startLon: 14.000 + i * dLon8,
      endLat: latOff8, endLon: 14.000 + (i + 1) * dLon8,
    }))
  }
  const stubsEndLon = 14.000 + 8 * dLon8
  const g = buildRailGraph([
    seg({ key: 'track1', osmId: 'osmBest', startLat: 50.000, startLon: 14.000, endLat: 50.000, endLon: 14.020 }),
    ...altSegs,
    seg({ key: 'long1', osmId: 'osmAlt', startLat: latOff8, startLon: stubsEndLon, endLat: latOff108, endLon: 14.010 }),
    seg({ key: 'long2', osmId: 'osmAlt', startLat: latOff108, startLon: 14.010, endLat: latOff8, endLon: 14.020 }),
    // Crossovers join the two tracks at both ends so the penalized re-run can
    // route A -> sibling -> B at all (same shape as buildCrossoverJoinedDoubleTrack).
    seg({ key: 'crossIn', startLat: 50.000, startLon: 14.000, endLat: latOff8, endLon: 14.000, isTraversalOnly: true }),
    seg({ key: 'crossOut', startLat: 50.000, startLon: 14.020, endLat: latOff8, endLon: 14.020, isTraversalOnly: true }),
  ])
  const result = walkRailStationPairs(g, [
    { fromLat: 50.000, fromLon: 14.000, toLat: 50.000, toLon: 14.020, pax: 20, frt: 4 },
  ])
  assert.equal(result.failures.ambiguous, 1, 'length-weighted median lateral ~58 m > WALK_TWIN_MEDIAN_LATERAL_M = 50 — an edge-count fraction (8/10 = 0.8) would wrongly exempt this')
  assert.equal(result.pairsWalked, 0)
  assert.equal(result.stampsBySegmentKey.size, 0)
})

test('twin classification has its OWN radii (review round): a station throat widening to 30 m for 400 m stays a twin — NOT ambiguous — while the 15 m spread radius stays strict', () => {
  // CZ Step-A v3 regression (ambiguous 29 -> 71): around island platforms
  // parallel tracks legitimately spread to 20-40 m for 300-600 m. Under the
  // spread's 15 m token-less radius those throat stretches read as non-twin;
  // under the quantile gate the sibling's length-weighted median lateral is
  // ~8 m (<= WALK_TWIN_MEDIAN_LATERAL_M) and nothing sits FAR, so the
  // throat stays a twin and the pair walks cleanly. The sibling track: 800 m at 8 m offset,
  // a 200 m transition out (heading delta ~6.3° < 10°), 400 m at 30 m (the
  // island-platform passage), 200 m back, ~547 m at 8 m — joined to the
  // straight track by crossovers at both ends.
  const latOff8 = 50 + 8 / 110_540
  const latOff30 = 50 + 30 / 110_540
  const g = buildRailGraph([
    seg({ key: 'track1', osmId: 'osmBest', startLat: 50, startLon: lonAtM(0), endLat: 50, endLon: lonAtM(2147) }),
    seg({ key: 's1', osmId: 'osmAlt', startLat: latOff8, startLon: lonAtM(0), endLat: latOff8, endLon: lonAtM(800) }),
    seg({ key: 't1', osmId: 'osmAlt', startLat: latOff8, startLon: lonAtM(800), endLat: latOff30, endLon: lonAtM(1000) }),
    seg({ key: 's2', osmId: 'osmAlt', startLat: latOff30, startLon: lonAtM(1000), endLat: latOff30, endLon: lonAtM(1400) }),
    seg({ key: 't2', osmId: 'osmAlt', startLat: latOff30, startLon: lonAtM(1400), endLat: latOff8, endLon: lonAtM(1600) }),
    seg({ key: 's3', osmId: 'osmAlt', startLat: latOff8, startLon: lonAtM(1600), endLat: latOff8, endLon: lonAtM(2147) }),
    seg({ key: 'crossIn', startLat: 50, startLon: lonAtM(0), endLat: latOff8, endLon: lonAtM(0), isTraversalOnly: true }),
    seg({ key: 'crossOut', startLat: 50, startLon: lonAtM(2147), endLat: latOff8, endLon: lonAtM(2147), isTraversalOnly: true }),
  ])
  const result = walkRailStationPairs(g, [
    { fromLat: 50, fromLon: lonAtM(0), toLat: 50, toLon: lonAtM(2147), pax: 20, frt: 4 },
  ])
  assert.equal(result.failures.ambiguous, 0, 'the sibling with a 30 m station-throat passage is a twin under the quantile gate — the spread\'s 15 m arm would have read the throat as non-twin')
  assert.equal(result.pairsWalked, 1)
  assert.deepEqual(result.stampsBySegmentKey.get('track1'), { pax: 20, frt: 4, divisor: 1 }, 'walked directly; no divisor — the SPREAD still refuses the 30 m throat (and the 8 m stretches fail its overlap fraction at track1\'s own midpoint)')
  assert.equal(result.stampsBySegmentKey.has('s2'), false, 'the throat itself gets NO spread stamp: energy division keeps the strict 15 m radius — classification tolerance never leaks into acoustics')
})

// ── DE v3 quantile gate: bounded station/yard excursions vs far corridors ──

test('twin gate (DE v3): a sibling with one steep ~360 m-lateral yard excursion (~22 % of length beyond 120 m) is a TWIN — the walk succeeds', () => {
  // The DE Step A v2 failure mass (1 775 ambiguous pairs, median lateral
  // p75 = 5.2 m, MAX lateral p25-p75 = 293-552 m): a sibling track that
  // swings out around a yard/station for a bounded stretch. v2's 300 m
  // contiguous non-twin-run cap (calibrated on ~300 m CZ throats) failed
  // exactly this shape; the quantile gate reads it as: length-weighted
  // median ~8 m (sibling spacing), p75 ~8 m (beyond-120 m length ~22 % <=
  // 25 %), nothing at/beyond FAR (apex 358 m < 500) -> twin. The p75 gate
  // (Codex C2) draws the boundary INSIDE this class: a steep, short swing
  // like this one stays a twin, while a gently-ramped ~1.4 km parallel
  // stretch at ~180 m (the earlier fixture shape) now reads as a separate
  // alignment and stays ambiguous — that is the intended trade. Both routes
  // near-equal length (ratio ~1.01 < 1.2), fully edge-disjoint, so the
  // ambiguity probe fires and only the twin exemption saves the pair.
  const off8 = 50 + 8 / 110_540
  const off358 = 50 + 358 / 110_540
  const g = buildRailGraph([
    seg({ key: 'best', osmId: 'osmBest', startLat: 50, startLon: lonAtM(0), endLat: 50, endLon: lonAtM(6000) }),
    seg({ key: 's1', osmId: 'osmAlt', startLat: off8, startLon: lonAtM(0), endLat: off8, endLon: lonAtM(2400) }),
    seg({ key: 't1', osmId: 'osmAlt', startLat: off8, startLon: lonAtM(2400), endLat: off358, endLon: lonAtM(2700) }),
    seg({ key: 's2', osmId: 'osmAlt', startLat: off358, startLon: lonAtM(2700), endLat: off358, endLon: lonAtM(3100) }),
    seg({ key: 't2', osmId: 'osmAlt', startLat: off358, startLon: lonAtM(3100), endLat: off8, endLon: lonAtM(3400) }),
    seg({ key: 's3', osmId: 'osmAlt', startLat: off8, startLon: lonAtM(3400), endLat: off8, endLon: lonAtM(6000) }),
    seg({ key: 'crossIn', startLat: 50, startLon: lonAtM(0), endLat: off8, endLon: lonAtM(0), isTraversalOnly: true }),
    seg({ key: 'crossOut', startLat: 50, startLon: lonAtM(6000), endLat: off8, endLon: lonAtM(6000), isTraversalOnly: true }),
  ])
  const result = walkRailStationPairs(g, [
    { fromLat: 50, fromLon: lonAtM(0), toLat: 50, toLon: lonAtM(6000), pax: 40, frt: 20 },
  ])
  assert.equal(result.failures.ambiguous, 0, 'median ~8 m, p75 ~8 m, far fraction 0 — a steep bounded excursion no longer votes the pair ambiguous')
  assert.equal(result.pairsWalked, 1)
  assert.deepEqual(result.stampsBySegmentKey.get('best'), { pax: 40, frt: 20, divisor: 1 })
})

test('twin gate (Codex C2): a gently-ramped ~1.4 km parallel stretch at ~180 m STAYS ambiguous — the p75 gate closes the 50-500 m middle band', () => {
  // The reviewed counterexample class: a disjoint alternate that is a true
  // sibling for most of its length but runs a ~180 m-lateral parallel
  // alignment (long gentle ramps + apex) for well over a quarter of it —
  // median ~8 m and nothing >= 500 m would have passed it; the length-
  // weighted p75 (~183 m > 120 m) correctly reads it as a separate
  // alignment. Same geometry as the steep-excursion twin fixture above but
  // with 700 m ramps and a 500 m apex (beyond-120 m length ~32 %).
  const off8 = 50 + 8 / 110_540
  const off358 = 50 + 358 / 110_540
  const g = buildRailGraph([
    seg({ key: 'best', osmId: 'osmBest', startLat: 50, startLon: lonAtM(0), endLat: 50, endLon: lonAtM(6000) }),
    seg({ key: 's1', osmId: 'osmAlt', startLat: off8, startLon: lonAtM(0), endLat: off8, endLon: lonAtM(2000) }),
    seg({ key: 't1', osmId: 'osmAlt', startLat: off8, startLon: lonAtM(2000), endLat: off358, endLon: lonAtM(2700) }),
    seg({ key: 's2', osmId: 'osmAlt', startLat: off358, startLon: lonAtM(2700), endLat: off358, endLon: lonAtM(3200) }),
    seg({ key: 't2', osmId: 'osmAlt', startLat: off358, startLon: lonAtM(3200), endLat: off8, endLon: lonAtM(3900) }),
    seg({ key: 's3', osmId: 'osmAlt', startLat: off8, startLon: lonAtM(3900), endLat: off8, endLon: lonAtM(6000) }),
    seg({ key: 'crossIn', startLat: 50, startLon: lonAtM(0), endLat: off8, endLon: lonAtM(0), isTraversalOnly: true }),
    seg({ key: 'crossOut', startLat: 50, startLon: lonAtM(6000), endLat: off8, endLon: lonAtM(6000), isTraversalOnly: true }),
  ])
  const result = walkRailStationPairs(g, [
    { fromLat: 50, fromLon: lonAtM(0), toLat: 50, toLon: lonAtM(6000), pax: 40, frt: 20 },
  ])
  assert.equal(result.failures.ambiguous, 1, 'length-weighted p75 ~183 m > WALK_TWIN_P75_LATERAL_M = 120 — a quarter+ of the alt runs a separate alignment')
  const rec = result.failedPairChords.find((r) => r.reason === 'ambiguous')
  assert.ok(rec!.ambiguousGeometry!.twinGate!.p75LateralM > 120, 'the p75 arm is what failed this pair')
  assert.ok(rec!.ambiguousGeometry!.twinGate!.medianLateralM <= 50, 'median alone would have passed it')
})

test('twin gate (DE v3): an alt spending ~20 % of its length >= 500 m away STAYS ambiguous even with a tiny median', () => {
  // The far-fraction arm: a mostly-sibling alt that runs a genuinely
  // separate alignment (600 m away, past WALK_TWIN_FAR_LATERAL_M) for a
  // fifth of its length is not "the same corridor with an excursion" — the
  // count could really belong to either alignment there. Median alone (8 m)
  // would pass it; the far gate must not.
  const off8 = 50 + 8 / 110_540
  const off600 = 50 + 600 / 110_540
  const g = buildRailGraph([
    seg({ key: 'best', osmId: 'osmBest', startLat: 50, startLon: lonAtM(0), endLat: 50, endLon: lonAtM(5000) }),
    seg({ key: 's1', osmId: 'osmAlt', startLat: off8, startLon: lonAtM(0), endLat: off8, endLon: lonAtM(3000) }),
    seg({ key: 't1', osmId: 'osmAlt', startLat: off8, startLon: lonAtM(3000), endLat: off600, endLon: lonAtM(3400) }),
    seg({ key: 's2', osmId: 'osmAlt', startLat: off600, startLon: lonAtM(3400), endLat: off600, endLon: lonAtM(4500) }),
    seg({ key: 't2', osmId: 'osmAlt', startLat: off600, startLon: lonAtM(4500), endLat: off8, endLon: lonAtM(4900) }),
    seg({ key: 's3', osmId: 'osmAlt', startLat: off8, startLon: lonAtM(4900), endLat: off8, endLon: lonAtM(5000) }),
    seg({ key: 'crossIn', startLat: 50, startLon: lonAtM(0), endLat: off8, endLon: lonAtM(0), isTraversalOnly: true }),
    seg({ key: 'crossOut', startLat: 50, startLon: lonAtM(5000), endLat: off8, endLon: lonAtM(5000), isTraversalOnly: true }),
  ])
  const result = walkRailStationPairs(g, [
    { fromLat: 50, fromLon: lonAtM(0), toLat: 50, toLon: lonAtM(5000), pax: 40, frt: 20 },
  ])
  assert.equal(result.failures.ambiguous, 1, '1 100 m of ~5 640 m (~20 %) sits at 600 m >= WALK_TWIN_FAR_LATERAL_M — over the 10 % allowance')
  assert.equal(result.pairsWalked, 0)
  const rec = result.failedPairChords.find((r) => r.reason === 'ambiguous')
  assert.ok(rec?.ambiguousGeometry?.twinGate, 'the diagnostic carries the gate\'s own numbers')
  assert.ok(rec!.ambiguousGeometry!.twinGate!.farLengthFraction > 0.10, 'far fraction is what failed this pair')
  assert.ok(rec!.ambiguousGeometry!.twinGate!.medianLateralM <= 50, 'median alone would have passed it')
})

// ── Narrow gauge (rail_type 3) is walkable ──────────────────────────────────

test('walk: a station pair on a narrow-gauge line (railType 3) snaps, walks and stamps — the RhB/Osoblaha class', () => {
  // CH Step A: 826 snap + 643 unlocalized failures were the metre-gauge
  // networks — their edges never entered the graph, so stops had nothing to
  // snap to and >100-trains/day lines fell to the engine's 10/day
  // narrow-gauge default. isWalkableRailType now admits railType 3.
  const a = seg({ key: 'ng1', railType: 3, startLat: 50, startLon: 14.000, endLat: 50, endLon: 14.010 })
  const b = seg({ key: 'ng2', railType: 3, startLat: 50, startLon: 14.010, endLat: 50, endLon: 14.020 })
  const g = buildRailGraph([a, b])
  const result = walkRailStationPairs(g, [
    { fromLat: 50, fromLon: 14.000, toLat: 50, toLon: 14.020, pax: 104, frt: 2 },
  ])
  assert.equal(result.failures.snapFailed, 0, 'narrow-gauge nodes are in the graph — the stop snaps')
  assert.equal(result.pairsWalked, 1)
  assert.deepEqual(result.stampsBySegmentKey.get('ng1'), { pax: 104, frt: 2, divisor: 1 })
  assert.deepEqual(result.stampsBySegmentKey.get('ng2'), { pax: 104, frt: 2, divisor: 1 })
})

// ── Disconnected pair ────────────────────────────────────────────────────────

test('walk: a pair across two disconnected components fails and quarantines both endpoint tracks', () => {
  const track1 = seg({ key: 't1', startLat: 50.000, startLon: 14.000, endLat: 50.000, endLon: 14.010 })
  const track2 = seg({ key: 't2', startLat: 51.000, startLon: 14.000, endLat: 51.000, endLon: 14.010 }) // far, untouching
  const g = buildRailGraph([track1, track2])
  const result = walkRailStationPairs(g, [
    { fromLat: 50.000, fromLon: 14.000, toLat: 51.000, toLon: 14.000, pax: 4, frt: 0 },
  ])
  assert.equal(result.failures.disconnected, 1)
  assert.equal(result.pairsWalked, 0)
  assert.equal(result.stampsBySegmentKey.size, 0)
  assert.equal(result.quarantinedSegmentKeys.has('t1'), true, 'disconnected takes chord band + fingers — the from-side track (at focus A) is inside')
  assert.equal(result.quarantinedSegmentKeys.has('t2'), true, 'and the to-side track too — the trains exist, the graph is broken between them')
})

// ── snapFailed localization (2026-07-16 /gg review item 4): one snapped
// end anchors a bounded evidence shape; neither snapped end uses the chord
// vicinity and increments the unlocalized count. ─────────────────────────────

test('walk: a pair with only ONE end snapping stays localized and quarantines the snapped end', () => {
  const track1 = seg({ key: 't1', startLat: 50.000, startLon: 14.000, endLat: 50.000, endLon: 14.010 })
  const g = buildRailGraph([track1])
  // toLat/toLon sit ~1100 km away — far outside STATION_SNAP_RADIUS_M, so only
  // the `from` end snaps.
  const result = walkRailStationPairs(g, [
    { fromLat: 50.000, fromLon: 14.000, toLat: 60.000, toLon: 14.000, pax: 5, frt: 0 },
  ])
  assert.equal(result.failures.snapFailed, 1)
  assert.equal(result.unlocalizedPairs, 0, 'one end resolved — this pair is localizable, not counted as unlocalized')
})

// DE Step A v2 diagnostics (2026-07-16 failure analysis, fix 3): a snapFailed
// record must carry the pair's own coords plus the TRUE distance from each
// unsnapped endpoint to the nearest graph node, so a v3 tuning pass can tell
// "missed the 300 m radius by a few metres" from "nowhere near this network
// at all" without re-running the whole enrichment.
test('walk: a snapFailed record carries the pair coords plus the true distance to nearest graph node for the end that failed to snap', () => {
  const track1 = seg({ key: 't1', startLat: 50.000, startLon: 14.000, endLat: 50.000, endLon: 14.010 })
  const g = buildRailGraph([track1])
  // The `to` end sits ~5 km north of the track — well outside
  // STATION_SNAP_RADIUS_M (300 m) but close enough to stay under
  // nearestRailGraphNodeDistanceM's search ceiling, so the diagnostic
  // resolves to a real, assertable number instead of the 'unreachable'
  // sentinel.
  const result = walkRailStationPairs(g, [
    { fromLat: 50.000, fromLon: 14.000, toLat: 50.045, toLon: 14.005, pax: 5, frt: 0 },
  ])
  assert.equal(result.failures.snapFailed, 1)
  assert.equal(result.failedPairChords.length, 1)
  const rec = result.failedPairChords[0]
  assert.equal(rec.reason, 'snapFailed')
  assert.equal(rec.fromLat, 50.000)
  assert.equal(rec.toLat, 50.045)
  assert.equal(rec.ambiguousGeometry, undefined, 'diagnostics are reason-specific — snapFailed never carries ambiguousGeometry')
  assert.ok(rec.snapDistanceM, 'must carry per-end snap distances')
  assert.equal(rec.snapDistanceM!.from, null, 'the FROM end snapped fine — nothing to diagnose there')
  const to = rec.snapDistanceM!.to
  assert.ok(typeof to === 'number' && to > 300 && to < 10_000,
    `the TO end is ~5 km from the only track — well beyond STATION_SNAP_RADIUS_M but still a sane, findable distance (got ${to})`)
})

test('walk: an unlocalized pair (neither end snaps) records the true distance to the nearest graph node for BOTH ends', () => {
  const track1 = seg({ key: 't1', startLat: 50.000, startLon: 14.000, endLat: 50.000, endLon: 14.010 })
  const g = buildRailGraph([track1])
  const result = walkRailStationPairs(g, [
    { fromLat: 50.040, fromLon: 14.000, toLat: 50.045, toLon: 14.010, pax: 5, frt: 0 }, // both ~4-5 km from the only track
  ])
  assert.equal(result.unlocalizedPairs, 1)
  assert.equal(result.failedPairChords.length, 1)
  const rec = result.failedPairChords[0]
  assert.ok(rec.snapDistanceM, 'must carry per-end snap distances')
  assert.ok(typeof rec.snapDistanceM!.from === 'number' && rec.snapDistanceM!.from > 300, 'the FROM end also failed to snap')
  assert.ok(typeof rec.snapDistanceM!.to === 'number' && rec.snapDistanceM!.to > 300, 'the TO end also failed to snap')
})

// Codex review item 3 (2026-07-16): these records are JSON-persisted
// (rail-stops sidecar), Infinity serializes to null, and null already means
// "this end snapped fine" — an endpoint beyond the search ceiling must carry
// the explicit 'unreachable' sentinel instead, and it must SURVIVE a JSON
// round-trip distinguishable from the snapped-fine null.
test('walk: an endpoint with no graph node within the search ceiling records \'unreachable\', never Infinity — JSON round-trip keeps it distinct from null', () => {
  const track1 = seg({ key: 't1', startLat: 50.000, startLon: 14.000, endLat: 50.000, endLon: 14.010 })
  const g = buildRailGraph([track1])
  // FROM snaps exactly; TO sits ~1100 km away — beyond the 200 km ceiling.
  const result = walkRailStationPairs(g, [
    { fromLat: 50.000, fromLon: 14.000, toLat: 60.000, toLon: 14.000, pax: 5, frt: 0 },
  ])
  assert.equal(result.failures.snapFailed, 1)
  const rec = result.failedPairChords[0]
  assert.equal(rec.snapDistanceM!.from, null, 'snapped fine')
  assert.equal(rec.snapDistanceM!.to, 'unreachable', 'beyond the ceiling — the JSON-safe sentinel, never Infinity')
  const roundTripped = JSON.parse(JSON.stringify(rec)) as typeof rec
  assert.equal(roundTripped.snapDistanceM!.from, null)
  assert.equal(roundTripped.snapDistanceM!.to, 'unreachable', 'survives JSON — Infinity would have collapsed to null here')
})

test('walk: a pair where NEITHER end snaps increments unlocalizedPairs', () => {
  const track1 = seg({ key: 't1', startLat: 50.000, startLon: 14.000, endLat: 50.000, endLon: 14.010 })
  const g = buildRailGraph([track1])
  const result = walkRailStationPairs(g, [
    { fromLat: 60.000, fromLon: 14.000, toLat: 61.000, toLon: 14.000, pax: 5, frt: 0 }, // both ends far from the only track
  ])
  assert.equal(result.failures.snapFailed, 1)
  assert.equal(result.unlocalizedPairs, 1, 'unlocatable pair counted separately — it could belong to ANY component')
})

// ── Per-pair failure quarantine (2026-07-16 Step-B refinement + /gg fix
// batch item 4 + review round) — replaces per-component retract/silent
// withholding with the FAILED pair's own evidence shape PER FAILURE REASON:
// 'ambiguous' -> the TRUE admissible-path ellipse (admissible paths exist);
// 'detourRejected'/'disconnected' -> endpoint-radius balls around both
// snapped ends (the trains exist, the graph failed to place them);
// one-end 'snapFailed' -> the ball around the snapped end; unlocalized ->
// chord vicinity. Verified on live CZ Step-A data: rail is one connected
// component nationwide, so whole-component gating had withheld
// retract/silent across the ENTIRE 31 245 km mainline behind just 9 of 150
// failed pairs. ──────────────────────────────────────────────────────────────

test('quarantine (DE/NL redesign): a detour-rejected pair takes chord band + graph fingers — corridor protected, far branches out', () => {
  // Chord A-B ~7.16 km -> bound ~19.9 km. The graph's only A-B route detours
  // ~110 km south (detourRejected), and A anchors a three-hop dead-end chain
  // due north (~10.50 km/hop). Mixed criterion (quarantineGraphlessPair):
  // a node u qualifies when distGraph(focus,u) + distGeo(u,otherFocus) <=
  // bound; plus the 5 km chord band. chain0 qualifies through its A
  // endpoint (0 + 7.16 <= 19.9); chain1's nearer endpoint M1 sits 10.5 km
  // up the dead end (10.5 + geo(M1,B) 12.7 = 23.2 > 19.9) -> OUT (the old
  // full-bound flood-ball leaked here — scaled to a country, one
  // cross-border leg blanketed the whole network, DE: ~95 000 km). The
  // absurd detour arms a-p/p-b stay quarantined ONLY through their
  // focus-incident endpoints (edge granularity; real rows are ~250 m
  // microsegments, so this over-quarantines metres, not corridors).
  const a = seg({ key: 'a-p', startLat: 50.000, startLon: 14.000, endLat: 49.000, endLon: 14.050 })
  const b = seg({ key: 'p-b', startLat: 49.000, startLon: 14.050, endLat: 50.000, endLon: 14.100 })
  const chain0 = seg({ key: 'chain0', startLat: 50.000, startLon: 14.000, endLat: 50.095, endLon: 14.000 })
  const chain1 = seg({ key: 'chain1', startLat: 50.095, startLon: 14.000, endLat: 50.190, endLon: 14.000 })
  const chain2 = seg({ key: 'chain2', startLat: 50.190, startLon: 14.000, endLat: 50.285, endLon: 14.000 })
  const g = buildRailGraph([a, b, chain0, chain1, chain2])

  const result = walkRailStationPairs(g, [
    { fromLat: 50.000, fromLon: 14.000, toLat: 50.000, toLon: 14.100, pax: 5, frt: 0 },
  ])
  assert.equal(result.failures.detourRejected, 1, 'fixture sanity: same shape as the existing detour-gate test')
  assert.equal(
    componentOfSegment(g, 'a-p'), componentOfSegment(g, 'chain2'),
    'fixture sanity: ONE connected component spans the detour pair AND the far chain',
  )
  assert.equal(result.quarantinedSegmentKeys.has('chain0'), true, 'qualifies through its A endpoint (distGraph 0 + geo(A,B) 7.16 km <= bound) — the chord corridor stays protected from the silent residual')
  assert.equal(result.quarantinedSegmentKeys.has('chain1'), false, 'nearer endpoint 10.5 km up the dead end: 10.5 + 12.7 = 23.2 km > the ~19.9 km bound — no admissible A-B path reaches it (the old full-bound ball leaked here)')
  assert.equal(result.quarantinedSegmentKeys.has('chain2'), false, 'farther still — outside')
  assert.equal(result.quarantinedSegmentKeys.has('a-p'), true, 'incident to focus A itself (distGraph 0) — edge granularity keeps it; real rows are microsegments')
  assert.equal(result.quarantinedSegmentKeys.has('p-b'), true, 'incident to focus B')
})

test('quarantine path-union: an ambiguous pair quarantines exactly its candidate corridors — nothing else, not even a dead-end tail at the station', () => {
  // The two-corridor ambiguous shape (buildTwoCorridorGraph geometry inline)
  // plus a two-hop dead-end tail running due WEST from A, opposite the
  // target. The union of candidate paths (best + penalized re-runs within
  // WALK_AMBIGUITY_LENGTH_RATIO) is both corridors and ONLY both corridors:
  // the tail carries no admissible A-B path at all — the earlier
  // graph-distance ELLIPSE still leaked onto tail1 through its A endpoint
  // (distA 0 + distB ~7.49 km <= the ~19.9 km bound), and scaled to a
  // long-haul leg it blanketed a country (DE: one 415 km ambiguous ICE leg
  // covered most of the network; ~94 % quarantined through every other
  // improvement).
  const an = seg({ key: 'A-N', startLat: 50.000, startLon: 14.000, endLat: 50.010, endLon: 14.050 })
  const nb = seg({ key: 'N-B', startLat: 50.010, startLon: 14.050, endLat: 50.000, endLon: 14.100 })
  const as_ = seg({ key: 'A-S', startLat: 50.000, startLon: 14.000, endLat: 49.990, endLon: 14.050 })
  const sb = seg({ key: 'S-B', startLat: 49.990, startLon: 14.050, endLat: 50.000, endLon: 14.100 })
  const tail1 = seg({ key: 'tail1', startLat: 50.000, startLon: 14.000, endLat: 50.000, endLon: 13.902 })
  const tail2 = seg({ key: 'tail2', startLat: 50.000, startLon: 13.902, endLat: 50.000, endLon: 13.804 })
  const g = buildRailGraph([an, nb, as_, sb, tail1, tail2])

  const result = walkRailStationPairs(g, [
    { fromLat: 50.000, fromLon: 14.000, toLat: 50.000, toLon: 14.100, pax: 7, frt: 0 },
  ])
  assert.equal(result.failures.ambiguous, 1, 'fixture sanity: two disjoint similar-length corridors, no shape')
  for (const k of ['A-N', 'N-B', 'A-S', 'S-B']) {
    assert.equal(result.quarantinedSegmentKeys.has(k), true, `${k} lies on a candidate corridor — inside the union`)
  }
  assert.equal(result.quarantinedSegmentKeys.has('tail1'), false, 'no admissible A-B path uses the tail — the ellipse leaked here, the path union does not')
  assert.equal(result.quarantinedSegmentKeys.has('tail2'), false, 'nor farther up the dead end')
})
test('quarantine: an unlocalized pair (neither end snaps) quarantines only stampable segments within 5 km of its own straight chord', () => {
  // The chord runs due north along lon 10.000 from (10.000,10.000) to
  // (11.000,10.000) (~111 km). `near` sits exactly on that chord; `far` sits
  // thousands of km away. Neither track's own nodes are within
  // STATION_SNAP_RADIUS_M of either query endpoint, so the pair itself fails
  // to snap on both ends.
  const near = seg({ key: 'near', startLat: 10.010, startLon: 10.000, endLat: 10.011, endLon: 10.000 })
  const far = seg({ key: 'far', startLat: 55.000, startLon: 19.000, endLat: 55.001, endLon: 19.000 })
  const g = buildRailGraph([near, far])

  const result = walkRailStationPairs(g, [
    { fromLat: 10.000, fromLon: 10.000, toLat: 11.000, toLon: 10.000, pax: 5, frt: 0 },
  ])
  assert.equal(result.unlocalizedPairs, 1, 'fixture sanity: neither end snaps onto either track')
  assert.equal(result.quarantinedSegmentKeys.has('near'), true, 'within UNLOCALIZED_PAIR_QUARANTINE_RADIUS_M (5 km) of the chord')
  assert.equal(result.quarantinedSegmentKeys.has('far'), false, 'thousands of km from the chord — untouched')
})

// ── Dijkstra scratch reuse ───────────────────────────────────────────────────
// walkRailStationPairs threads ONE DijkstraScratch through every search on a
// graph (the Europe-union-graph scale fix) — these tests drive
// dijkstraShortestPath directly to prove reuse never leaks state between
// searches: a scratch run TWICE on the same pair, then once on a different
// pair, must match a fresh (no-scratch) call every time.

test('dijkstraShortestPath: a reused scratch run on the same pair twice, then a different pair, matches fresh-allocation results', () => {
  const trunk = seg({ key: 'trunk', startLat: 50, startLon: 14.000, endLat: 50, endLon: 14.030 })
  const spur = seg({ key: 'spur', startLat: 50, startLon: 14.010, endLat: 50.01, endLon: 14.010 })
  const g = buildRailGraph([trunk, spur])
  const fromNode = snapToNearestRailGraphNode(g, 50, 14.000)
  const toNode = snapToNearestRailGraphNode(g, 50, 14.030)
  const otherToNode = snapToNearestRailGraphNode(g, 50.01, 14.010)
  const weight = (_i: number, e: { lengthM: number }) => e.lengthM

  const scratch = createDijkstraScratch(g.nodeCount)
  const run1 = dijkstraShortestPath(g, fromNode, toNode, null, weight, scratch)
  const run2 = dijkstraShortestPath(g, fromNode, toNode, null, weight, scratch) // same pair AGAIN, same scratch
  const run3 = dijkstraShortestPath(g, fromNode, otherToNode, null, weight, scratch) // a DIFFERENT pair, same scratch

  const fresh1 = dijkstraShortestPath(g, fromNode, toNode, null, weight) // no scratch => fresh allocation
  const fresh3 = dijkstraShortestPath(g, fromNode, otherToNode, null, weight)

  assert.ok(run1 && fresh1)
  assert.equal(run1!.lengthM, fresh1!.lengthM)
  assert.deepEqual([...run1!.edgeIndices].sort(), [...fresh1!.edgeIndices].sort())

  assert.ok(run2)
  assert.equal(run2!.lengthM, fresh1!.lengthM, 'repeating the SAME search on a reused scratch matches fresh allocation')
  assert.deepEqual([...run2!.edgeIndices].sort(), [...fresh1!.edgeIndices].sort())

  assert.ok(run3 && fresh3)
  assert.equal(run3!.lengthM, fresh3!.lengthM, 'a DIFFERENT search right after, on the same reused scratch, also matches fresh allocation')
  assert.deepEqual([...run3!.edgeIndices].sort(), [...fresh3!.edgeIndices].sort())
})

// ── Parallel spread ──────────────────────────────────────────────────────────

/** N parallel tracks, all pairwise < PARALLEL_SPREAD_RADIUS_M = 50 m,
 *  identical heading/longitude span, distinct osmId, no shared node. Offsets
 *  are chosen to ALSO stay clear of each other's canonical-pair 4-dp
 *  rounding bucket (~11 m) — otherwise two distinct station pairs would
 *  collapse into one canonical pair before routing even starts, which is
 *  the correct behaviour for real close-together station platforms but
 *  would defeat this fixture's goal of forcing each pair onto its own
 *  track. Each pair is snapped exactly onto its own track's endpoints
 *  (0 m away, closer than any neighbour track) so each walk is forced onto
 *  its own isolated track. */
function buildParallelTracks(offsetsDeg: number[], corridorToken: string, paxByIndex: number[], frtByIndex: number[]) {
  const segs: RailGraphSegmentInput[] = []
  const pairs: Array<{ fromLat: number; fromLon: number; toLat: number; toLon: number; pax: number; frt: number }> = []
  offsetsDeg.forEach((offset, i) => {
    const lat = 50 + offset
    const s = seg({ key: `track${i}`, osmId: `osm${i}`, startLat: lat, startLon: 14.000, endLat: lat, endLon: 14.010, corridorToken })
    segs.push(s)
    pairs.push({ fromLat: lat, fromLon: 14.000, toLat: lat, toLon: 14.010, pax: paxByIndex[i], frt: frtByIndex[i] })
  })
  return { g: buildRailGraph(segs), pairs }
}

test('parallel spread N=2: confirmed group conserves total effective traffic before/after', () => {
  const paxByIndex = [10, 8], frtByIndex = [4, 6]
  const { g, pairs } = buildParallelTracks([0, 0.00035], 'PARL', paxByIndex, frtByIndex)
  const before = paxByIndex.reduce((s, p, i) => s + effectiveRailTraffic(p, frtByIndex[i], 0, 0, 1).total, 0)

  const result = walkRailStationPairs(g, pairs)
  assert.deepEqual(result.stampsBySegmentKey.get('track0'), { pax: 18, frt: 10, divisor: 2 })
  assert.deepEqual(result.stampsBySegmentKey.get('track1'), { pax: 18, frt: 10, divisor: 2 })

  const after = [0, 1].reduce((s, i) => {
    const stamp = result.stampsBySegmentKey.get(`track${i}`)!
    return s + effectiveRailTraffic(stamp.pax, stamp.frt, 0, 0, stamp.divisor).total
  }, 0)
  assert.equal(before, 28)
  assert.equal(after, before, 'N * (total/N) = total — corridor total conserved')
})

test('parallel spread N=4: confirmed group of 4 conserves total effective traffic before/after', () => {
  const paxByIndex = [10, 12, 14, 16], frtByIndex = [3, 3, 3, 3]
  const { g, pairs } = buildParallelTracks([0, 0.00012, 0.00024, 0.00036], 'PARL', paxByIndex, frtByIndex)
  const before = paxByIndex.reduce((s, p, i) => s + effectiveRailTraffic(p, frtByIndex[i], 0, 0, 1).total, 0)

  const result = walkRailStationPairs(g, pairs)
  for (let i = 0; i < 4; i++) {
    assert.deepEqual(result.stampsBySegmentKey.get(`track${i}`), { pax: 52, frt: 12, divisor: 4 })
  }
  const after = [0, 1, 2, 3].reduce((s, i) => {
    const stamp = result.stampsBySegmentKey.get(`track${i}`)!
    return s + effectiveRailTraffic(stamp.pax, stamp.frt, 0, 0, stamp.divisor).total
  }, 0)
  assert.equal(before, 64)
  assert.equal(after, before)
})

test('parallel spread: token-less lines 30 m apart do NOT spread (beyond the strict 15 m token-less radius)', () => {
  // Without a corridor token, only the STRICT arm applies — 30 m is real
  // track-pair spacing for two DISTINCT lines sharing a corridor, not a
  // double-track (4-10 m), so no group forms.
  const paxByIndex = [10, 8], frtByIndex = [4, 6]
  const { g, pairs } = buildParallelTracks([0, 0.000271], '', paxByIndex, frtByIndex) // ~30 m apart, empty corridorToken
  const result = walkRailStationPairs(g, pairs)
  assert.deepEqual(result.stampsBySegmentKey.get('track0'), { pax: 10, frt: 4, divisor: 1 }, 'untouched — no group formed')
  assert.deepEqual(result.stampsBySegmentKey.get('track1'), { pax: 8, frt: 6, divisor: 1 })
})

test('parallel spread: token-less double-track 8 m apart — walk stamps ONE track, spread reaches the unstamped sibling', () => {
  // A canonical pair is walked exactly once along ONE shortest path, so only
  // track0 ever gets a walk stamp; without unstamped-sibling spread, track1
  // would render at the full engine class default next to its divided twin
  // (the DE +8..+17 dB double-count shape).
  const siblingLat = 50 + 8 / 110_540 // ~8 m north — genuine double-track spacing
  const t0 = seg({ key: 'track0', osmId: 'osm0', startLat: 50, startLon: 14.000, endLat: 50, endLon: 14.010 })
  const t1 = seg({ key: 'track1', osmId: 'osm1', startLat: siblingLat, startLon: 14.000, endLat: siblingLat, endLon: 14.010 })
  const g = buildRailGraph([t0, t1])
  const result = walkRailStationPairs(g, [
    { fromLat: 50, fromLon: 14.000, toLat: 50, toLon: 14.010, pax: 20, frt: 6 }, // ONE pair — snaps onto track0 only
  ])
  assert.equal(result.pairsWalked, 1)
  assert.deepEqual(result.stampsBySegmentKey.get('track0'), { pax: 20, frt: 6, divisor: 2 })
  assert.deepEqual(result.stampsBySegmentKey.get('track1'), { pax: 20, frt: 6, divisor: 2 }, 'unstamped sibling receives the group stamp')

  const singleTrackTotal = effectiveRailTraffic(20, 6, 0, 0, 1).total
  const after = ['track0', 'track1'].reduce((s, k) => {
    const stamp = result.stampsBySegmentKey.get(k)!
    return s + effectiveRailTraffic(stamp.pax, stamp.frt, 0, 0, stamp.divisor).total
  }, 0)
  assert.equal(after, singleTrackTotal, 'corridor total conserved: 2 x (26/2) = 26')
})

test('parallel spread: different non-empty corridor tokens never pair, even 8 m apart', () => {
  const siblingLat = 50 + 8 / 110_540
  const t0 = seg({ key: 'track0', osmId: 'osm0', startLat: 50, startLon: 14.000, endLat: 50, endLon: 14.010, corridorToken: 'LINE-A' })
  const t1 = seg({ key: 'track1', osmId: 'osm1', startLat: siblingLat, startLon: 14.000, endLat: siblingLat, endLon: 14.010, corridorToken: 'LINE-B' })
  const g = buildRailGraph([t0, t1])
  const result = walkRailStationPairs(g, [
    { fromLat: 50, fromLon: 14.000, toLat: 50, toLon: 14.010, pax: 20, frt: 6 },
  ])
  assert.deepEqual(result.stampsBySegmentKey.get('track0'), { pax: 20, frt: 6, divisor: 1 }, 'named corridors stay independent')
  assert.equal(result.stampsBySegmentKey.has('track1'), false)
})

// ── Staggered fixtures (review round 2): microsegments are cut per OSM way,
// so real parallel tracks carry 0-250 m longitudinal midpoint stagger — the
// round-1 midpoint-distance metric never paired these at all. ──────────────

/** Metres -> longitude degrees at lat 50, matching the flat-earth constants. */
const DEG_PER_M_LON_AT_50 = 1 / (111_320 * Math.cos(50 * Math.PI / 180))
const lonAtM = (m: number) => 14 + m * DEG_PER_M_LON_AT_50

test('parallel spread: STAGGERED token-less double-track (250 m segments, 125 m offset), one track stamped — every segment gets value=T divisor=2', () => {
  const latA = 50, latB = 50 + 5 / 110_540 // 5 m lateral — genuine double-track spacing
  const g = buildRailGraph([
    seg({ key: 'A1', osmId: 'osmA', startLat: latA, startLon: lonAtM(0), endLat: latA, endLon: lonAtM(250) }),
    seg({ key: 'A2', osmId: 'osmA', startLat: latA, startLon: lonAtM(250), endLat: latA, endLon: lonAtM(500) }),
    seg({ key: 'B1', osmId: 'osmB', startLat: latB, startLon: lonAtM(125), endLat: latB, endLon: lonAtM(375) }),
    seg({ key: 'B2', osmId: 'osmB', startLat: latB, startLon: lonAtM(375), endLat: latB, endLon: lonAtM(625) }),
  ])
  const result = walkRailStationPairs(g, [
    { fromLat: latA, fromLon: lonAtM(0), toLat: latA, toLon: lonAtM(500), pax: 20, frt: 6 }, // walk stamps A1+A2 only
  ])
  assert.equal(result.pairsWalked, 1)
  for (const k of ['A1', 'A2', 'B1', 'B2']) {
    assert.deepEqual(result.stampsBySegmentKey.get(k), { pax: 20, frt: 6, divisor: 2 }, `${k} carries the corridor value at divisor 2`)
  }
  // Cross-section conservation at ~300 m (tracks A2 + B1 present there):
  const crossSection = ['A2', 'B1'].reduce((s, k) => {
    const stamp = result.stampsBySegmentKey.get(k)!
    return s + effectiveRailTraffic(stamp.pax, stamp.frt, 0, 0, stamp.divisor).total
  }, 0)
  assert.equal(crossSection, effectiveRailTraffic(20, 6, 0, 0, 1).total, 'cross-section total = single-track total')
})

test('parallel spread: STAGGERED double-track, BOTH tracks stamped — each side carries T_A+T_B at divisor 2, cross-section conserved', () => {
  const latA = 50, latB = 50 + 5 / 110_540
  const g = buildRailGraph([
    seg({ key: 'A1', osmId: 'osmA', startLat: latA, startLon: lonAtM(0), endLat: latA, endLon: lonAtM(250) }),
    seg({ key: 'A2', osmId: 'osmA', startLat: latA, startLon: lonAtM(250), endLat: latA, endLon: lonAtM(500) }),
    seg({ key: 'B1', osmId: 'osmB', startLat: latB, startLon: lonAtM(125), endLat: latB, endLon: lonAtM(375) }),
    seg({ key: 'B2', osmId: 'osmB', startLat: latB, startLon: lonAtM(375), endLat: latB, endLon: lonAtM(625) }),
  ])
  const result = walkRailStationPairs(g, [
    { fromLat: latA, fromLon: lonAtM(0), toLat: latA, toLon: lonAtM(500), pax: 20, frt: 6 },   // T_A on A1+A2
    { fromLat: latB, fromLon: lonAtM(125), toLat: latB, toLon: lonAtM(625), pax: 8, frt: 2 }, // T_B on B1+B2
  ])
  assert.equal(result.pairsWalked, 2)
  for (const k of ['A1', 'A2', 'B1', 'B2']) {
    assert.deepEqual(result.stampsBySegmentKey.get(k), { pax: 28, frt: 8, divisor: 2 }, `${k} carries T_A+T_B`)
  }
  const crossSection = ['A2', 'B1'].reduce((s, k) => {
    const stamp = result.stampsBySegmentKey.get(k)!
    return s + effectiveRailTraffic(stamp.pax, stamp.frt, 0, 0, stamp.divisor).total
  }, 0)
  assert.equal(crossSection, 36, 'cross-section total = T_A(26) + T_B(10)')
})

test('parallel spread: third track only mid-corridor — divisor 3 on the overlapping stretch, 2 elsewhere, every cross-section conserved', () => {
  const latA = 50, latB = 50 + 5 / 110_540, latC = 50 + 10 / 110_540
  const g = buildRailGraph([
    seg({ key: 'A1', osmId: 'osmA', startLat: latA, startLon: lonAtM(0), endLat: latA, endLon: lonAtM(250) }),
    seg({ key: 'A2', osmId: 'osmA', startLat: latA, startLon: lonAtM(250), endLat: latA, endLon: lonAtM(500) }),
    seg({ key: 'A3', osmId: 'osmA', startLat: latA, startLon: lonAtM(500), endLat: latA, endLon: lonAtM(750) }),
    seg({ key: 'B1', osmId: 'osmB', startLat: latB, startLon: lonAtM(0), endLat: latB, endLon: lonAtM(250) }),
    seg({ key: 'B2', osmId: 'osmB', startLat: latB, startLon: lonAtM(250), endLat: latB, endLon: lonAtM(500) }),
    seg({ key: 'B3', osmId: 'osmB', startLat: latB, startLon: lonAtM(500), endLat: latB, endLon: lonAtM(750) }),
    seg({ key: 'C1', osmId: 'osmC', startLat: latC, startLon: lonAtM(250), endLat: latC, endLon: lonAtM(500) }), // mid-corridor only
  ])
  const result = walkRailStationPairs(g, [
    { fromLat: latA, fromLon: lonAtM(0), toLat: latA, toLon: lonAtM(750), pax: 30, frt: 9 }, // stamps A1+A2+A3
  ])
  assert.equal(result.pairsWalked, 1)
  for (const k of ['A2', 'B2', 'C1']) {
    assert.deepEqual(result.stampsBySegmentKey.get(k), { pax: 30, frt: 9, divisor: 3 }, `${k}: three tracks overlap here`)
  }
  for (const k of ['A1', 'A3', 'B1', 'B3']) {
    assert.deepEqual(result.stampsBySegmentKey.get(k), { pax: 30, frt: 9, divisor: 2 }, `${k}: only two tracks here`)
  }
  const singleTrackTotal = effectiveRailTraffic(30, 9, 0, 0, 1).total // 39
  const threeTrackSection = ['A2', 'B2', 'C1'].reduce((s, k) => {
    const stamp = result.stampsBySegmentKey.get(k)!
    return s + effectiveRailTraffic(stamp.pax, stamp.frt, 0, 0, stamp.divisor).total
  }, 0)
  const twoTrackSection = ['A1', 'B1'].reduce((s, k) => {
    const stamp = result.stampsBySegmentKey.get(k)!
    return s + effectiveRailTraffic(stamp.pax, stamp.frt, 0, 0, stamp.divisor).total
  }, 0)
  assert.equal(threeTrackSection, singleTrackTotal, '3-track cross-section conserved')
  assert.equal(twoTrackSection, singleTrackTotal, '2-track cross-section conserved')
})

// ── Overlap-fraction gate: accepted asymmetric-residual bound (2026-07-16 /gg
// review item 7 — documented in applyParallelSpread's doc as ACCEPTED ERROR,
// not a bug) ─────────────────────────────────────────────────────────────────

test('parallel spread: asymmetric overlap sliver — short segment accepts a long neighbour, long segment rejects the short one back (deterministic, accepted bound)', () => {
  // A spans 0-250 m (walked, carries T=20); B spans 200-300 m, laterally ~8 m
  // away (genuine double-track spacing), never itself walked. Their overlap
  // is 50 m: 50/250=20% of A's length (< the 30 m/30% gate — A rejects B),
  // but 50/100=50% of B's length (>= the gate — B accepts A). Exact
  // conservation would require splitting either row at the overlap boundary;
  // this pass doesn't, and the resulting sliver mismatch is the accepted
  // error applyParallelSpread's doc quantifies at <=1.76 dB over <=250 m.
  const latA = 50, latB = 50 + 8 / 110_540
  const g = buildRailGraph([
    seg({ key: 'A', osmId: 'osmA', startLat: latA, startLon: lonAtM(0), endLat: latA, endLon: lonAtM(250) }),
    seg({ key: 'B', osmId: 'osmB', startLat: latB, startLon: lonAtM(200), endLat: latB, endLon: lonAtM(300) }),
  ])
  const result = walkRailStationPairs(g, [
    { fromLat: latA, fromLon: lonAtM(0), toLat: latA, toLon: lonAtM(250), pax: 20, frt: 0 },
  ])
  assert.equal(result.pairsWalked, 1)
  assert.deepEqual(result.stampsBySegmentKey.get('A'), { pax: 20, frt: 0, divisor: 1 }, 'A (250 m) sees only a 20% overlap with B — below its own 30% gate, no sibling found, renders T/1 unchanged')
  assert.deepEqual(result.stampsBySegmentKey.get('B'), { pax: 20, frt: 0, divisor: 2 }, 'B (100 m) sees a 50% overlap with A — clears its own gate, accepts A as sibling, renders T/2')
})

// ── R15 findRailFlowJumps ────────────────────────────────────────────────────

function czShapeRows(): RailEndpointRow[] {
  // The diagnosed trať 200 shape: one mainline segment stamped 16 pax + 0
  // frt (source 110), meeting a neighbour stamped 2 pax + 1 frt (source
  // 9863) at a shared endpoint — raw looks tame, effective is 36 vs 3.
  return [
    { key: 'segA', osmId: 'wA', railType: 0, usage: 0, service: 0, sourceId: 110, pax: 16, frt: 0, parallelDivisor: 1, startLat: 49.700, startLon: 14.000, endLat: 49.669, endLon: 14.0015 },
    { key: 'segB', osmId: 'wB', railType: 0, usage: 0, service: 0, sourceId: 9863, pax: 2, frt: 1, parallelDivisor: 1, startLat: 49.669, startLon: 14.0015, endLat: 49.640, endLon: 14.003 },
  ]
}

test('findRailFlowJumps: fires on the CZ shape (effective 36 vs 3 at the shared endpoint)', () => {
  const violations = findRailFlowJumps(czShapeRows(), null)
  assert.equal(violations.length, 1)
  const v = violations[0]
  assert.equal(v.aSourceId, 110)
  assert.equal(v.bSourceId, 9863)
  assert.equal(v.effA.total, 36)
  assert.equal(v.effB.total, 3)
  // Per-column check reports whichever column has the WORST ratio — here the
  // freight zero-default (20 vs 1 = 20x) is more extreme than the 12x total
  // jump, exactly the "same totals can hide a real seam" case the plan calls
  // out for comparing per-column AND total.
  assert.equal(v.column, 'frt')
  assert.ok(v.ratio > 3)
})

test('findRailFlowJumps: exempted by a rail stop within 300 m of the endpoint', () => {
  const stopsIndex = buildRailStopsIndex([{ lat: 49.669, lon: 14.0015 }])
  const violations = findRailFlowJumps(czShapeRows(), stopsIndex)
  assert.equal(violations.length, 0)
})

test('findRailFlowJumps: exempted by a 3rd heavy-rail non-service branch at the junction', () => {
  const rows = czShapeRows()
  rows.push({
    key: 'segC', osmId: 'wC', railType: 0, usage: 1, service: 0, sourceId: 0, pax: 0, frt: 0, parallelDivisor: 1,
    startLat: 49.669, startLon: 14.0015, endLat: 49.660, endLon: 14.020,
  })
  const violations = findRailFlowJumps(rows, null)
  assert.equal(violations.length, 0, 'a real junction (3rd branch) explains the jump')
})

test('findRailFlowJumps: pax 2 vs 7 does NOT fire just because frt (100 vs 100) clears the floor — the floor is PER COLUMN (2026-07-16 /gg review item 5)', () => {
  const rows: RailEndpointRow[] = [
    { key: 'segA', osmId: 'wA', railType: 0, usage: 0, service: 0, sourceId: 110, pax: 2, frt: 100, parallelDivisor: 1, startLat: 49.000, startLon: 15.000, endLat: 49.010, endLon: 15.001 },
    { key: 'segB', osmId: 'wB', railType: 0, usage: 0, service: 0, sourceId: 9863, pax: 7, frt: 100, parallelDivisor: 1, startLat: 49.010, startLon: 15.001, endLat: 49.020, endLon: 15.002 },
  ]
  const violations = findRailFlowJumps(rows, null)
  assert.equal(violations.length, 0, 'pax ratio is 3.5x (> RAIL_JUMP_RATIO) but max(2,7)=7 never clears its OWN 20/day floor; frt is 1:1 and total is ~1:1 too')
})

// ── R16 findRailContinuityGaps ───────────────────────────────────────────────

function continuityRows(measuredPax: number, measuredFrt: number): RailEndpointRow[] {
  return [
    { key: 'measured', osmId: 'wM', railType: 0, usage: 0, service: 0, sourceId: 110, pax: measuredPax, frt: measuredFrt, parallelDivisor: 1, startLat: 49.500, startLon: 14.100, endLat: 49.480, endLon: 14.110 },
    { key: 'gap', osmId: 'wG', railType: 0, usage: 0, service: 0, sourceId: 0, pax: 0, frt: 0, parallelDivisor: 1, startLat: 49.480, startLon: 14.110, endLat: 49.460, endLon: 14.120 },
  ]
}

test('findRailContinuityGaps: fires on stamped-12 vs default-100 (coverage gap)', () => {
  const violations = findRailContinuityGaps(continuityRows(8, 4), null) // 8+4=12
  assert.equal(violations.length, 1)
  const v = violations[0]
  assert.equal(v.effA.total, 12)
  assert.equal(v.effB.total, 100)
  assert.ok(v.ratio > 3)
})

test('findRailContinuityGaps: does NOT fire on stamped-90 vs default-100 (within tolerance)', () => {
  const violations = findRailContinuityGaps(continuityRows(70, 20), null) // 70+20=90
  assert.equal(violations.length, 0)
})
