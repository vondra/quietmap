/** Colombia TPDA class observations and Red Vial fallback regression. */

import assert from 'node:assert/strict'
import { test } from 'node:test'
import { matchColombiaRoad, type ColombiaRoadSource } from './lib/roads-co-source.js'
import { buildRoadLineVertexGrid, type PinnedRoadLine } from './lib/pinned-road-lines.js'
import type { RoadRow } from './lib/roads-arrow.js'

const line = (properties: Record<string, unknown>): PinnedRoadLine => ({
  coordinates: [[-74.1, 4.6], [-74.09, 4.6]], properties, relativePath: 'fixture',
})
const source = (network: PinnedRoadLine[], tpda: PinnedRoadLine[]): ColombiaRoadSource => ({
  network: buildRoadLineVertexGrid(network), tpda: buildRoadLineVertexGrid(tpda),
  sourceRows: 0, sourceLines: 0, invalidGeometrySkipped: 0, unavailableTrafficRows: 0,
})
const road: RoadRow = { startLat: 4.6, startLon: -74.1, endLat: 4.6, endLon: -74.1,
  midLat: 4.6, midLon: -74.1, roadClass: 1, ref: null, name: null, osmId: 1, existingSourceId: 0 }

test('Colombia prioritizes TPDA and preserves its observed vehicle percentages', () => {
  assert.deepEqual(matchColombiaRoad(road, source([line({ superficie: '1', administrador: '2', calzada: '2' })],
    [line({ conteo: 1000, au_p: 60, bu_p: 10, ca_p: 30 })])),
  { kind: 'tpda', light: 1140, medium: 190, heavy: 570, moto: 100 })
})

test('Colombia uses Red Vial defaults only for major roads', () => {
  const network = source([line({ superficie: '1', administrador: '2', calzada: '2' })], [])
  assert.deepEqual(matchColombiaRoad(road, network),
    { kind: 'network', light: 27500, medium: 2500, heavy: 5000, moto: 15000 })
  assert.equal(matchColombiaRoad({ ...road, roadClass: 3 }, network), null)
})
