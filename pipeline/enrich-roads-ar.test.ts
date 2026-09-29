/** Argentina observed TMDA priority and DNV fallback regression. */

import assert from 'node:assert/strict'
import { test } from 'node:test'
import { matchArgentinaRoad, type ArgentinaRoadSource } from './lib/roads-ar-source.js'
import { buildRoadLineVertexGrid, type PinnedRoadLine } from './lib/pinned-road-lines.js'
import type { RoadRow } from './lib/roads-arrow.js'

const line = (properties: Record<string, unknown>, relativePath = 'ar/roads-national.geojson'): PinnedRoadLine => ({
  observationId: 'fixture', coordinates: [[-58.5, -34.6], [-58.49, -34.6]], properties, relativePath,
})
const source = (dnv: PinnedRoadLine[], tmda: PinnedRoadLine[]): ArgentinaRoadSource => ({
  dnv: buildRoadLineVertexGrid(dnv), tmda: buildRoadLineVertexGrid(tmda),
  sourceRows: 0, sourceLines: 0, invalidGeometrySkipped: 0, unavailableTrafficRows: 0,
})
const road: RoadRow = { startLat: -34.6, startLon: -58.5, endLat: -34.6, endLon: -58.5,
  midLat: -34.6, midLon: -58.5, roadClass: 1, ref: null, name: null, osmId: 1, existingSourceId: 0 }

test('Argentina keeps observed TMDA unscaled inside Buenos Aires, with its split', () => {
  assert.deepEqual(matchArgentinaRoad(road, source([line({ tipo_de_superficie_de_via: 'PAVIMENTO' })],
    [line({ valor: 1000 }, 'ar/tmda-2017-18.geojson')])),
  { countBasis: 'both-directions', observationId: 'fixture', kind: 'tmda', light: 750, medium: 100, heavy: 100, moto: 50 })
})

test('Argentina distinguishes national and provincial DNV fallbacks', () => {
  assert.deepEqual(matchArgentinaRoad(road, source([line({ tipo_de_superficie_de_via: 'PAVIMENTO' },
    'ar/roads-provincial.geojson')], [])),
  { countBasis: 'both-directions', observationId: 'fixture', kind: 'dnv-provincial', light: 18000, medium: 2400, heavy: 2400, moto: 1200 })
})
