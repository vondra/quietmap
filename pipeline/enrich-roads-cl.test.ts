/** Chile observed TMDA priority and Vialidad fallback regression. */

import assert from 'node:assert/strict'
import { test } from 'node:test'
import { matchChileRoad, type ChileRoadSource } from './lib/roads-cl-source.js'
import { buildRoadLineVertexGrid, type PinnedRoadLine } from './lib/pinned-road-lines.js'
import type { RoadRow } from './lib/roads-arrow.js'
import { buildOneHundredthDegreePointGrid } from './lib/spatial.js'

const line: PinnedRoadLine = { coordinates: [[-70.7, -33.5], [-70.69, -33.5]],
  properties: { CARPETA: 'PAVIMENTO', CONCESIONADO: 'SI' }, relativePath: 'fixture' }
const source = (tmda: Array<{ latitude: number; longitude: number; aadt: number }>): ChileRoadSource => ({
  network: buildRoadLineVertexGrid([line]), tmda: buildOneHundredthDegreePointGrid(tmda),
  sourceRows: 0, sourceLines: 0, tmdaPoints: tmda.length, invalidGeometrySkipped: 0, unavailableTrafficSkipped: 0,
})
const road: RoadRow = { startLat: -33.5, startLon: -70.7, endLat: -33.5, endLon: -70.7,
  midLat: -33.5, midLon: -70.7, roadClass: 1, ref: null, name: null, osmId: 1, existingSourceId: 0 }

test('Chile prioritizes observed TMDA and applies the Santiago split', () => {
  assert.deepEqual(matchChileRoad(road, source([{ latitude: -33.5, longitude: -70.7, aadt: 1000 }])),
    { kind: 'tmda', light: 1500, medium: 200, heavy: 200, moto: 100 })
})

test('Chile uses Vialidad classifications only for major roads', () => {
  assert.deepEqual(matchChileRoad(road, source([])),
    { kind: 'network', light: 52500, medium: 7000, heavy: 7000, moto: 3500 })
  assert.equal(matchChileRoad({ ...road, roadClass: 3 }, source([])), null)
})
