/** Regression tests for the shared road-enrichment geometry. */

import assert from 'node:assert/strict'
import test from 'node:test'
import {
  buildOneHundredthDegreePointGrid, flatDist, geoJsonAreaCentroid,
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


test('polygon centroid is area-weighted across closing vertices, holes, parts and the dateline', () => {
  const rectangle = [[[77, 20], [79, 20], [79, 22], [77, 22], [77, 20]]]
  assert.deepEqual(geoJsonAreaCentroid(rectangle, 'Polygon'), [78, 21])
  const small = [[[110, 30], [110.0001, 30], [110.0001, 30.0001], [110, 30.0001], [110, 30]]]
  const smallCentroid = geoJsonAreaCentroid(small, 'Polygon')!
  assert.ok(Math.abs(smallCentroid[0] - 110.00005) < 1e-10)
  assert.ok(Math.abs(smallCentroid[1] - 30.00005) < 1e-10)
  const withHole = [[[0, 0], [4, 0], [4, 4], [0, 4], [0, 0]],
    [[0, 0], [1, 0], [1, 1], [0, 1], [0, 0]]]
  const holeCentroid = geoJsonAreaCentroid(withHole, 'Polygon')!
  assert.ok(Math.abs(holeCentroid[0] - 2.1) < 1e-12)
  assert.ok(Math.abs(holeCentroid[1] - 2.1) < 1e-12)
  const multipart = [
    [[[0, 0], [2, 0], [2, 2], [0, 2], [0, 0]]],
    [[[10, 0], [14, 0], [14, 4], [10, 4], [10, 0]]],
  ]
  const multipartCentroid = geoJsonAreaCentroid(multipart, 'MultiPolygon')!
  assert.ok(Math.abs(multipartCentroid[0] - 9.8) < 1e-12)
  assert.ok(Math.abs(multipartCentroid[1] - 1.8) < 1e-12)
  const seam = [[[179, 0], [-179, 0], [-179, 2], [179, 2], [179, 0]]]
  const seamCentroid = geoJsonAreaCentroid(seam, 'Polygon')!
  assert.ok(Math.abs(Math.abs(seamCentroid[0]) - 180) < 1e-12)
  assert.equal(seamCentroid[1], 1)
})
