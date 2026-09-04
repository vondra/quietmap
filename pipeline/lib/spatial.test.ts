/** Regression tests for the shared road-enrichment geometry. */

import assert from 'node:assert/strict'
import test from 'node:test'
import {
  buildOneHundredthDegreePointGrid, flatDist,
  nearestCompatiblePointWithin200Metres, pointToPolylineDist,
} from './spatial.js'

test('distance helpers use the short arc across the antimeridian', () => {
  const pointDistance = flatDist(0, 179.9, 0, -179.9)
  assert.ok(pointDistance > 20_000 && pointDistance < 25_000, `${pointDistance} metres`)

  const lineDistance = pointToPolylineDist(0, 180, [[179.995, 0], [-179.995, 0]])
  assert.ok(lineDistance < 100, `${lineDistance} metres`)
})

test('polyline distance handles empty, singleton, body and endpoint cases', () => {
  assert.equal(pointToPolylineDist(50, 14, []), Infinity)
  assert.ok(pointToPolylineDist(50, 14, [[14, 50]]) < 1e-6)
  assert.ok(pointToPolylineDist(50.001, 14.005, [[14, 50], [14.01, 50]]) > 100)
  assert.ok(pointToPolylineDist(50, 13.99, [[14, 50], [14.01, 50]]) > 700)
})

test('ranked point grid enforces class, strict radius and antimeridian neighbors', () => {
  const incompatible = { latitude: 0, longitude: 179.999, rank: 4, id: 'wrong-class' }
  const compatible = { latitude: 0, longitude: -179.999, rank: 1, id: 'seam-neighbor' }
  const grid = buildOneHundredthDegreePointGrid([incompatible, compatible])
  assert.equal(nearestCompatiblePointWithin200Metres(0, 180, 1, 1, grid), compatible)
  assert.equal(nearestCompatiblePointWithin200Metres(0.01, 180, 1, 1, grid), null)
})
