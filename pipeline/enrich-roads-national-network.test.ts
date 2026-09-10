/** Source registry and Brazil policy regression for the shared national network runner. */

import assert from 'node:assert/strict'
import { test } from 'node:test'
import { DATASETS } from './lib/enrichment-datasets.js'
import { NATIONAL_ROAD_NETWORK_POLICIES } from './lib/road-national-network-policies/index.js'
import type { PinnedRoadLine } from './lib/pinned-road-lines.js'
import type { RoadRow } from './lib/roads-arrow.js'

const road = (latitude: number, longitude: number): RoadRow => ({
  startLat: latitude, startLon: longitude, endLat: latitude, endLon: longitude,
  midLat: latitude, midLon: longitude, roadClass: 1, ref: null, name: null,
  osmId: 1, existingSourceId: 0,
})
const line = (properties: Record<string, unknown>): PinnedRoadLine => ({
  coordinates: [[-46.7, -23.6], [-46.6, -23.5]], properties, relativePath: 'br/test',
})

test('network proxy registry coverage agrees with the source registry', () => {
  for (const policy of NATIONAL_ROAD_NETWORK_POLICIES.values()) {
    const dataset = DATASETS.find(candidate => candidate.key === `${policy.country.toLowerCase()}-national-roads`)!
    assert.equal(dataset.measurement, 'proxy')
    assert.deepEqual([...policy.coverage], dataset.roadCoverage)
  }
})

test('Brazil retains DNIT surface, concession, city multiplier and vehicle mix', () => {
  const policy = NATIONAL_ROAD_NETWORK_POLICIES.get('BR')!
  assert.deepEqual(policy.traffic(road(-23.6, -46.7), line({ Superficie: 'PAV', Administra: 'Concession' })),
    { light: 49000, medium: 7000, heavy: 10500, moto: 3500 })
  assert.deepEqual(policy.traffic(road(-10, -50), line({ Superficie: 'TER', Administra: '' })),
    { light: 1800, medium: 300, heavy: 750, moto: 150 })
})
