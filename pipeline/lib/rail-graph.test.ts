/** Railway topology, junction healing and nearest-station snapping invariants. */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { flatDist } from './spatial.js'
import {
  buildRailGraph, snapToNearestRailGraphNode, nearestRailGraphNodeDistanceM,
  STATION_SNAP_RADIUS_M, type RailGraphSegmentInput,
} from './rail-graph.js'

function seg(over: Partial<RailGraphSegmentInput> & Pick<RailGraphSegmentInput, 'key' | 'startLat' | 'startLon' | 'endLat' | 'endLon'>): RailGraphSegmentInput {
  const lengthM = flatDist(over.startLat, over.startLon, over.endLat, over.endLon)
  return {
    osmId: over.key, railType: 0, usage: 0, isTraversalOnly: false, corridorToken: '',
    startKey: `${over.startLat},${over.startLon}`, endKey: `${over.endLat},${over.endLon}`,
    lengthM, ...over,
  }
}

function componentOfSegment(graph: ReturnType<typeof buildRailGraph>, key: string): number {
  const edge = graph.edges.find((candidate) => candidate.parentKey === key)
  assert.ok(edge, `missing graph edge for segment ${key}`)
  return graph.componentOfNode[edge.nodeA]
}

test('buildRailGraph: a single segment interns exactly two nodes and one edge', () => {
  const g = buildRailGraph([seg({ key: 'a', startLat: 50, startLon: 14, endLat: 50, endLon: 14.01 })])
  assert.equal(g.nodeCount, 2)
  assert.equal(g.edgeCount, 1)
  assert.equal(componentOfSegment(g, 'a'), 0)
})

test('T-junction healing: a branch touching a trunk mid-chord splits the trunk and connects the components', () => {
  // Trunk runs straight east-west at lat 50; its exact midpoint (50, 14.005)
  // is NOT one of its own endpoints, so endpoint identity would otherwise intern the
  // branch's foot as an isolated 3rd node with no edge to the trunk.
  const trunk = seg({ key: 'trunk', startLat: 50, startLon: 14.000, endLat: 50, endLon: 14.010, corridorToken: 'TRUNK' })
  const branch = seg({ key: 'branch', startLat: 50, startLon: 14.005, endLat: 50.005, endLon: 14.005, corridorToken: 'BRANCH' })
  // Unrelated far-away segment: proves components stay SEPARATE where nothing touches.
  const island = seg({ key: 'island', startLat: 60, startLon: 14.000, endLat: 60, endLon: 14.010 })

  const g = buildRailGraph([trunk, branch, island])

  assert.equal(g.nodeCount, 6, 'trunk start/end + junction + branch end, plus the island\'s own 2 nodes')
  assert.equal(g.edgeCount, 4, 'trunk healed into 2 sub-edges + 1 branch edge + 1 island edge')

  const trunkEdges = g.edges.filter((e) => e.parentKey === 'trunk')
  assert.equal(trunkEdges.length, 2, 'trunk split into two sub-edges at the junction')
  for (const e of trunkEdges) assert.equal(e.corridorToken, 'TRUNK', 'sub-edges keep the parent segment fields')

  const trunkComp = componentOfSegment(g, 'trunk')
  const branchComp = componentOfSegment(g, 'branch')
  const islandComp = componentOfSegment(g, 'island')
  assert.equal(trunkComp, branchComp, 'healing connects trunk and branch into ONE component')
  assert.notEqual(trunkComp, islandComp, 'an untouched segment stays its own component')
})

test('T-junction healing: no split when nothing touches the segment body', () => {
  const a = seg({ key: 'a', startLat: 50, startLon: 14.000, endLat: 50, endLon: 14.010 })
  const b = seg({ key: 'b', startLat: 51, startLon: 14.000, endLat: 51, endLon: 14.010 }) // far away, no touch
  const g = buildRailGraph([a, b])
  assert.equal(g.edgeCount, 2, 'neither segment is split')
  assert.notEqual(componentOfSegment(g, 'a'), componentOfSegment(g, 'b'))
})

test('snapToNearestRailGraphNode: within radius resolves to the nearest node, beyond radius fails (-1)', () => {
  const g = buildRailGraph([seg({ key: 'a', startLat: 50, startLon: 14, endLat: 50, endLon: 14.01 })])
  const nodeAId = 0 // trunk start interned first
  assert.equal(snapToNearestRailGraphNode(g, 50, 14), nodeAId, 'exact coordinate match')
  // ~50 m north — comfortably inside the default 300 m radius.
  const near = snapToNearestRailGraphNode(g, 50 + 50 / 110_540, 14, STATION_SNAP_RADIUS_M)
  assert.equal(near, nodeAId)
  // ~1000 m away — outside the default radius.
  const far = snapToNearestRailGraphNode(g, 50 + 1000 / 110_540, 14)
  assert.equal(far, -1)
})

// DE Step A v2 diagnostics (2026-07-16 failure analysis, fix 3):
// nearestRailGraphNodeDistanceM reports the TRUE distance regardless of
// STATION_SNAP_RADIUS_M — unlike snapToNearestRailGraphNode, which returns
// -1 past the radius with no further detail.
test('nearestRailGraphNodeDistanceM: exact match is 0; beyond the snap radius still resolves to the true distance instead of -1', () => {
  const g = buildRailGraph([seg({ key: 'a', startLat: 50, startLon: 14, endLat: 50, endLon: 14.01 })])
  assert.equal(nearestRailGraphNodeDistanceM(g, 50, 14), 0, 'exact coordinate match')
  // ~1000 m away — snapToNearestRailGraphNode already returns -1 here (see
  // the test above); the diagnostic function must still report ~1000 m.
  const d = nearestRailGraphNodeDistanceM(g, 50 + 1000 / 110_540, 14)
  assert.ok(Math.abs(d - 1000) < 1, `expected ~1000 m, got ${d}`)
  assert.equal(snapToNearestRailGraphNode(g, 50 + 1000 / 110_540, 14), -1, 'fixture sanity: this distance does fail the ordinary snap')
})

// Codex review item 1 (2026-07-16): box != circle — the first non-empty grid
// BOX can hold a node farther than one sitting just outside the box, so the
// raw box-best is NOT the true nearest; the ONE refining rescan (radius =
// first hit's own distance, whose box fully covers that circle) is.
test('nearestRailGraphNodeDistanceM: refining rescan — a nearer node OUTSIDE the first non-empty box beats the box-local best', () => {
  // Query (50, 14). Grid cells are 0.01°: the first scan (radius 300 m) covers
  // ±1 cell. Node A sits ~1.6 km NORTH — still inside the ±1-cell lat box
  // (lat 50.0149 is in cell +1). Node B sits ~1.4 km EAST — lon 14.0201 is
  // 2 lon cells away, OUTSIDE the ±1 box, yet TRULY NEARER than A.
  const nodeALat = 50.0149, nodeALon = 14.0
  const nodeBLat = 50.0, nodeBLon = 14.0201
  const g = buildRailGraph([
    seg({ key: 'a', startLat: nodeALat, startLon: nodeALon, endLat: nodeALat, endLon: nodeALon + 0.01 }),
    seg({ key: 'b', startLat: nodeBLat, startLon: nodeBLon, endLat: nodeBLat, endLon: nodeBLon + 0.01 }),
  ])
  const distToA = flatDist(50, 14, nodeALat, nodeALon)
  const distToB = flatDist(50, 14, nodeBLat, nodeBLon)
  assert.ok(distToB < distToA, `fixture sanity: B (${distToB.toFixed(1)} m) must be nearer than A (${distToA.toFixed(1)} m)`)
  const d = nearestRailGraphNodeDistanceM(g, 50, 14)
  assert.ok(Math.abs(d - distToB) < 1e-6, `true nearest is B at ${distToB.toFixed(2)} m — an unrefined box scan would report A at ${distToA.toFixed(2)} m (got ${d})`)
})

// Codex review item 2 (2026-07-16): the bare x4 ladder jumped 76.8 km ->
// 307.2 km and never scanned the 76.8-200 km band at all (verified: a node at
// 99 486 m returned Infinity). The widening is now clamped to one final pass
// exactly AT the ceiling.
test('nearestRailGraphNodeDistanceM: a node in the 76.8-200 km band is found by the clamped final ceiling pass; nothing within the ceiling is Infinity', () => {
  // Node at (50, 14); query 0.9° of latitude away — ~99.5 km, axis-aligned so
  // no earlier box corner reaches it.
  const g = buildRailGraph([seg({ key: 'far', startLat: 50, startLon: 14, endLat: 50, endLon: 14.01 })])
  const expected = flatDist(50.9, 14, 50, 14)
  assert.ok(expected > 76_800 && expected < 200_000, `fixture sanity: ${expected.toFixed(0)} m sits in the skipped band`)
  const d = nearestRailGraphNodeDistanceM(g, 50.9, 14)
  assert.ok(Math.abs(d - expected) < 1e-6, `expected ~${expected.toFixed(0)} m, got ${d}`)
  // Beyond the ceiling: genuinely nothing within 200 km -> Infinity (the
  // walk maps this to the JSON-safe 'unreachable' sentinel).
  assert.equal(nearestRailGraphNodeDistanceM(g, 60, 14), Infinity, '~1100 km from the only node — unreachable within the 200 km ceiling')
})
