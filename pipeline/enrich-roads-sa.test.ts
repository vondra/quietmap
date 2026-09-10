/** Saudi MoT priority and class-compatible spatial fallback regression. */

import assert from 'node:assert/strict'
import { test } from 'node:test'
import { matchSaudiRoad, parseSaudiMotSource, type SaudiRoadSource } from './lib/roads-sa-source.js'
import { buildRoadLineVertexGrid, type PinnedRoadLine } from './lib/pinned-road-lines.js'
import type { RoadRow } from './lib/roads-arrow.js'

const line = (properties: Record<string, unknown>): PinnedRoadLine => ({
  coordinates: [[46.7, 24.7], [46.71, 24.7]], properties, relativePath: 'fixture',
})
const source = (refAadt = new Map<string, number>(), riyadh: PinnedRoadLine[] = [], atlas: PinnedRoadLine[] = []): SaudiRoadSource => ({
  refAadt, riyadh: buildRoadLineVertexGrid(riyadh), atlas: buildRoadLineVertexGrid(atlas),
  sourceRows: 0, stations: 0, unavailableTrafficRows: 0, sourceLines: 0, invalidGeometrySkipped: 0,
})
const road: RoadRow = { startLat: 24.7, startLon: 46.7, endLat: 24.7, endLon: 46.7,
  midLat: 24.7, midLon: 46.7, roadClass: 1, ref: 'E40; 65', name: null, osmId: 1, existingSourceId: 0 }

test('Saudi MoT parser averages valid stations by road reference', () => {
  const parsed = parseSaudiMotSource('Road No.,Latitude,Longitude,24 Hour Total\n40,20,41,"1,000"\n40,21,42,"3,000"\n,,,\n')
  assert.deepEqual({ rows: parsed.sourceRows, stations: parsed.stations, unavailable: parsed.unavailableTrafficRows,
    aadt: parsed.refAadt.get('40') }, { rows: 3, stations: 2, unavailable: 1, aadt: 2000 })
})

test('Saudi MoT ref wins and Riyadh fallback enforces source-class compatibility', () => {
  assert.deepEqual(matchSaudiRoad(road, source(new Map([['40', 10_000]]), [line({ CLASS: 'B', NO_OF_LANE: 4 })])),
    { kind: 'mot', light: 7800, medium: 1000, heavy: 1100, moto: 100 })
  assert.deepEqual(matchSaudiRoad({ ...road, ref: null }, source(new Map(), [line({ CLASS: 'B', NO_OF_LANE: 4 })])),
    { kind: 'riyadh', light: 14040, medium: 1800, heavy: 1980, moto: 180 })
  assert.equal(matchSaudiRoad({ ...road, ref: null, roadClass: 4 },
    source(new Map(), [line({ CLASS: 'A', NO_OF_LANE: 5 })])), null)
})
