/** Peru dIMD, road classification and regional vehicle split regression. */

import assert from 'node:assert/strict'
import { test } from 'node:test'
import { matchPeruRoad, type PeruRoadSource } from './lib/roads-pe-source.js'
import { buildRoadLineVertexGrid, type PinnedRoadLine } from './lib/pinned-road-lines.js'
import type { RoadRow } from './lib/roads-arrow.js'

const line = (properties: Record<string, unknown>, relativePath = 'pe/roads-nacional-mtc.geojson'): PinnedRoadLine => ({
  coordinates: [[-77, -12], [-76.99, -12]], properties, relativePath,
})
const source = (lines: PinnedRoadLine[]): PeruRoadSource => ({ roads: buildRoadLineVertexGrid(lines),
  sourceRows: 0, sourceLines: 0, invalidGeometrySkipped: 0, observedTrafficLines: 0 })
const road: RoadRow = { startLat: -12, startLon: -77, endLat: -12, endLon: -77,
  midLat: -12, midLon: -77, roadClass: 1, ref: null, name: null, osmId: 1, existingSourceId: 0 }

test('Peru uses dIMD before classification and applies the Lima split', () => {
  assert.deepEqual(matchPeruRoad(road, source([line({ dIMD: 1000, cSuperfici: '1', cClasifica: 'TRANSVERSAL' })])),
    { kind: 'imd', light: 1300, medium: 120, heavy: 280, moto: 300 })
})

test('Peru retains departmental paved fallback and rejects lower OSM classes', () => {
  const departmental = source([line({ SUPERFIC: '1' }, 'pe/roads-departamental.geojson')])
  assert.deepEqual(matchPeruRoad(road, departmental),
    { kind: 'network', light: 3250, medium: 300, heavy: 700, moto: 750 })
  assert.equal(matchPeruRoad({ ...road, roadClass: 4 }, departmental), null)
})
