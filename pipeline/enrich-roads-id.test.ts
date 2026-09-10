/** Indonesia source priority, observed LHRT and proxy split tests. */

import assert from 'node:assert/strict'
import { test } from 'node:test'
import { matchIndonesiaRoad, type IndonesiaRoadSource } from './lib/roads-id-source.js'
import { buildRoadLineVertexGrid, type PinnedRoadLine } from './lib/pinned-road-lines.js'
import type { RoadRow } from './lib/roads-arrow.js'

const line = (properties: Record<string, unknown>): PinnedRoadLine => ({
  coordinates: [[106.8, -6.2], [106.81, -6.2]], properties, relativePath: 'fixture',
})
const source = (values: { toll?: PinnedRoadLine[]; regional?: PinnedRoadLine[]; national?: PinnedRoadLine[] }): IndonesiaRoadSource => ({
  toll: buildRoadLineVertexGrid(values.toll ?? []),
  regional: buildRoadLineVertexGrid(values.regional ?? []),
  national: buildRoadLineVertexGrid(values.national ?? []),
  sourceRows: 0, sourceLines: 0, invalidGeometrySkipped: 0,
})
const road: RoadRow = { startLat: -6.2, startLon: 106.8, endLat: -6.2, endLon: 106.8,
  midLat: -6.2, midLon: 106.8, roadClass: 1, ref: null, name: null, osmId: 1, existingSourceId: 0 }

test('Indonesia prioritizes toll, preserves observed LHRT and rejects lower road classes', () => {
  assert.deepEqual(matchIndonesiaRoad(road, source({ toll: [line({})], regional: [line({ LHRT: 1234 })] })),
    { kind: 'toll', light: 48000, medium: 8000, heavy: 8000, moto: 96000 })
  assert.deepEqual(matchIndonesiaRoad(road, source({ regional: [line({ LHRT: 1234 })] })),
    { kind: 'lhrt', light: 370, medium: 62, heavy: 62, moto: 740 })
  assert.equal(matchIndonesiaRoad({ ...road, roadClass: 3 }, source({ toll: [line({})] })), null)
})

test('Indonesia uses regional status then national network defaults', () => {
  assert.deepEqual(matchIndonesiaRoad(road, source({ regional: [line({ STATUS: 'Jalan Kota' })] })),
    { kind: 'regional', light: 7200, medium: 1200, heavy: 1200, moto: 14400 })
  assert.deepEqual(matchIndonesiaRoad(road, source({ national: [line({})] })),
    { kind: 'national', light: 18000, medium: 3000, heavy: 3000, moto: 36000 })
})
