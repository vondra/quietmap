/** Independent physical paths distinguish true returns, noisy progress, source junctions and longitude wrapping. */

import assert from 'node:assert/strict'
import { test } from 'node:test'
import { alignRailServiceShape } from './rail-service-shape.js'
import { flatDist, M_PER_DEG_LAT, M_PER_DEG_LON_EQ } from './spatial.js'
import { fixtureNodeDistances } from './transport-test-fixture.js'
import type { OrientedSourceRailWay } from './transport-topology.js'

type Point = [number, number]
const point = (x: number, y: number): Point => [y / M_PER_DEG_LAT, x / M_PER_DEG_LON_EQ]
function way(id: string, points: Point[], nodes: string[], reverse = false): OrientedSourceRailWay {
  assert.equal(points.length, nodes.length)
  const chain: OrientedSourceRailWay['nodes'] = points.map((coordinate, index) => [nodes[index], coordinate])
  return { id, nodes: chain, distances: fixtureNodeDistances(chain)!, role: '', reverse }
}

const interval = (from: number, to: number) => [from, to]

test('noise and a geographic bend cannot invent a reverse passage', () => {
  const monotone = [point(-1000, 0), point(-500, 0),
    ...Array.from({ length: 60 }, (_, index) => point(-.059 + index * .001, 0)),
    ...Array.from({ length: 60 }, (_, index) => point((index + 1) * 250 / 60, 3))]
  const through = alignRailServiceShape(monotone, [way('through', [point(-1000, 0), point(1000, 0)], ['a', 'b'])])
  assert.equal(through.status, 'aligned')
  assert.equal(through.passages.length, 1)
  assert.ok(through.passages.every(passage => passage.to > passage.from))
  const bend = [point(0, 0), point(1000, 0), point(1000, 1000), point(0, 1000)]
  const curved = alignRailServiceShape(bend, [way('curve', bend, ['a', 'b', 'c', 'd'])])
  assert.equal(curved.status, 'aligned')
  assert.equal(curved.passages.length, 1)
  assert.ok(Math.abs(curved.lengthM - bend.slice(1).reduce((sum, p, i) => sum + flatDist(...bend[i], ...p), 0)) < 1e-8)
})

test('a terminal return preserves two fractional passages and keeps the turning estimate uncertain', () => {
  const source = way('terminal', [point(0, 0), point(1000, 0)], ['a', 'b'])
  const result = alignRailServiceShape([point(0, 0), point(300, 0), point(600, 0), point(300, 0), point(0, 0)],
    [source, { ...source, reverse: true }])
  assert.equal(result.status, 'aligned')
  assert.deepEqual(result.passages.map(passage => interval(passage.from, passage.to)), [[0, 600], [600, 0]])
  assert.equal(result.lengthM, 1200)
  assert.equal(result.interpretation, 'estimated_alignment')
  assert.deepEqual(result.turns, [{ incoming: 0, outgoing: 1, way: 'terminal', estimateM: 600,
    projectionAdmissibleRangeM: [600, 1000] }])
})

test('source interior junctions preserve short branches in either direction and never heal coordinate-only crossings', () => {
  for (const extent of [400, 150]) {
    const shared = way('shared', [point(0, 0), point(600, 0), point(1000, 0)], ['a', 'junction', 'b'])
    const branch = way('branch', [point(600, 0), point(600, extent)], ['junction', 'c'])
    const shape = [point(0, 0), point(200, 0), point(400, 0), point(600, 0),
      ...[.2, .5, .75, 1].map(fraction => point(600, extent * fraction))]
    const result = alignRailServiceShape(shape, [shared, branch])
    assert.equal(result.status, 'aligned')
    assert.deepEqual(result.passages.map(passage => [passage.way, passage.from, passage.to]),
      [['shared', 0, 600], ['branch', 0, extent]])
    assert.equal(result.lengthM, 600 + extent)
    const disconnected = { ...branch, nodes: [['unrelated', point(600, 0)], ['c', point(600, extent)]] } as OrientedSourceRailWay
    assert.equal(alignRailServiceShape(shape, [shared, disconnected]).status, 'unmatched')
  }
  const reversed = alignRailServiceShape([point(1000, 0), point(800, 0), point(600, 0), point(600, 200), point(600, 400)], [
    way('shared', [point(0, 0), point(600, 0), point(1000, 0)], ['a', 'junction', 'b'], true),
    way('branch', [point(600, 400), point(600, 0), point(600, -500)], ['c', 'junction', 'd'], true),
  ])
  assert.equal(reversed.status, 'aligned')
  assert.equal(reversed.lengthM, 800)
  assert.deepEqual(reversed.passages.map(passage => interval(passage.from, passage.to)), [[1000, 600], [400, 0]])
})

test('explicit zero hops repeat one physical junction while different visits to a shared node remain ambiguous', () => {
  const shared = way('shared', [point(0, 0), point(600, 0), point(600, 0), point(1000, 0)], ['a', 'junction', 'junction', 'b'])
  const branch = way('branch', [point(600, 0), point(600, 0), point(600, 400)], ['junction', 'junction', 'c'])
  const shape = [point(0, 0), point(300, 0), point(600, 0), point(600, 400)]
  const result = alignRailServiceShape(shape, [shared, branch])
  assert.equal(result.status, 'aligned')
  assert.equal(result.lengthM, 1000)
  assert.deepEqual(result.passages.map(passage => interval(passage.from, passage.to)), [[0, 600], [0, 400]])
  const loop = way('loop', [point(0, 0), point(600, 0), point(800, 0), point(600, 0)], ['a', 'junction', 'b', 'junction'])
  assert.equal(alignRailServiceShape(shape, [loop, branch]).status, 'unmatched')
})

test('a dateline passage follows the short source edge instead of projecting across the globe', () => {
  const shape: Point[] = [[0, 179.99], [0, 180], [0, -179.99]]
  const result = alignRailServiceShape(shape, [way('dateline', [shape[0], shape[2]], ['a', 'b'])])
  assert.equal(result.status, 'aligned')
  assert.equal(result.passages.length, 1)
  assert.equal(result.lengthM, flatDist(...shape[0], ...shape[2]))
  assert.ok(result.maxErrorM < 1e-8)
})

test('a skipped reversal or insufficient observations cannot certify a passage', () => {
  const main = way('main', [point(0, 0), point(1000, 0)], ['a', 'b'])
  const branch = way('branch', [point(0, 0), point(0, 1000)], ['a', 'c'])
  const approach = way('approach', [point(-1000, 0), point(0, 0)], ['before', 'a'])
  const result = alignRailServiceShape([point(-1000, 0), point(-500, 0), point(0, 1000)],
    [approach, main, { ...main, reverse: true }, branch])
  assert.equal(result.status, 'unmatched')
  assert.equal(result.failedVertex, 2)
  assert.equal(alignRailServiceShape([], [main]).status, 'unmatched')
  assert.equal(alignRailServiceShape([point(0, 0)], [main]).status, 'unmatched')
})
