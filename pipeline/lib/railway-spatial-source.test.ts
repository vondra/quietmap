/** CN/IN spatial railway family separation and long-segment indexing regressions. */

import assert from 'node:assert/strict'
import { test } from 'node:test'
import {
  buildSpatialRailGrid, nearestSpatialRailFeature, spatialRailFeatureKinds,
  trafficForSpatialRailFeature, type SpatialRailFeature,
} from './railway-spatial-source.js'

const feature = (
  kind: 'national' | 'metro',
  properties: Record<string, unknown> = {},
): SpatialRailFeature => ({ coordinates: [[14, 50], [14.1, 50]], kind, properties })

test('family routing prevents Indian tram rows inheriting heavy-rail features', () => {
  assert.deepEqual(spatialRailFeatureKinds('IN', 0), ['national'])
  assert.deepEqual(spatialRailFeatureKinds('IN', 1), ['metro'])
  assert.deepEqual(spatialRailFeatureKinds('CN', 0), ['national', 'metro'])
  assert.deepEqual(spatialRailFeatureKinds('CN', 4), [])
})

test('spatial grid indexes the interior of a long source segment', () => {
  const source = feature('national')
  const grid = buildSpatialRailGrid([source])
  assert.equal(nearestSpatialRailFeature({ midLat: 50, midLon: 14.05 }, [grid]), source)
})

test('CN speed and IN suburban classifications preserve dev1 traffic tiers', () => {
  assert.deepEqual(
    trafficForSpatialRailFeature('CN', feature('national', { TopSpeed: 350 }), 40, 116),
    { passenger: 180, freight: 0 },
  )
  assert.deepEqual(
    trafficForSpatialRailFeature('IN', feature('national', {
      type: 'Broad Gauge', railwayzone: 'Central Railway', speed: 80,
    }), 19.1, 72.9),
    { passenger: 1300, freight: 30 },
  )
  assert.deepEqual(
    trafficForSpatialRailFeature('IN', feature('metro'), 19.1, 72.9),
    { passenger: 400, freight: 0 },
  )
})
