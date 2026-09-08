import assert from 'node:assert/strict'
import test from 'node:test'

import {
  FAN_TERRAIN_DB_THRESHOLD,
  classifyFanSlice,
  segmentFanHighlight,
} from '../src/components/noise/fanGeometry.ts'

test('classifyFanSlice: cause from blocked + 1 kHz terrain', () => {
  assert.equal(classifyFanSlice({ blocked: false, terrain_db: 0 }), 'clear')
  assert.equal(classifyFanSlice({ blocked: true, terrain_db: 0 }), 'building')
  assert.equal(classifyFanSlice({ blocked: true, terrain_db: 0.4 }), 'building')
  assert.equal(classifyFanSlice({ blocked: false, terrain_db: 3.2 }), 'terrain')
  assert.equal(classifyFanSlice({ blocked: true, terrain_db: 3.2 }), 'mixed')
})

test('classifyFanSlice: threshold boundary is inclusive', () => {
  assert.equal(classifyFanSlice({ blocked: false, terrain_db: FAN_TERRAIN_DB_THRESHOLD }), 'terrain')
  assert.equal(
    classifyFanSlice({ blocked: false, terrain_db: FAN_TERRAIN_DB_THRESHOLD - 0.01 }),
    'clear',
  )
})

/** Receiver at (50.0, 14.0); 200 m E-W segment 120 m north of it. */
function northSegmentTrace(overrides = {}) {
  return {
    kind: 'road',
    start_lat: 50.001078,
    start_lon: 13.9986,
    end_lat: 50.001078,
    end_lon: 14.0014,
    cp_lat: 50.001078,
    cp_lon: 14.0,
    dist_m: 120,
    propagation: {
      model: 'cnossos',
      screening: {
        fan: {
          span_deg: 40,
          blocked_fraction: 0.5,
          quadrature: 'arc',
          intervals: [
            { from_deg: -20, to_deg: 0, blocked: true, obstacle: { kind: 'building', height_m: 9 }, terrain_db: 0, screen_db: 12, contains_cp: false },
            { from_deg: 0, to_deg: 20, blocked: false, terrain_db: 0, screen_db: 0, contains_cp: true },
          ],
        },
      },
      path_profile: { rcv_lat: 50.0, rcv_lon: 14.0 },
    },
    ...overrides,
  }
}

test('segmentFanHighlight: one triangle per interval + cp ray + segment', () => {
  const collection = segmentFanHighlight(northSegmentTrace())
  assert.ok(collection)
  assert.equal(collection.type, 'FeatureCollection')
  assert.equal(collection.features.length, 4)
  const polys = collection.features.filter(f => f.geometry.type === 'Polygon')
  assert.equal(polys.length, 2)
  assert.deepEqual(
    polys.map(f => f.properties.fanKind),
    ['building', 'clear'],
  )
  for (const poly of polys) {
    const ring = poly.geometry.coordinates[0]
    assert.equal(ring.length, 4)
    assert.deepEqual(ring[0], ring[3], 'ring closes')
    assert.deepEqual(ring[0], [14.0, 50.0], 'triangle starts at receiver')
  }
  const lines = collection.features.filter(f => f.geometry.type === 'LineString')
  assert.equal(lines.length, 2)
  assert.deepEqual(
    lines.map(f => f.properties.lineKind).sort(),
    ['cp', 'segment'],
  )
})

test('segmentFanHighlight: null without a fan', () => {
  const noFan = northSegmentTrace()
  noFan.propagation.screening = {}
  assert.equal(segmentFanHighlight(noFan), null)

  const pointSource = { kind: 'building', dist_m: 10, propagation: { model: 'cnossos' } }
  assert.equal(segmentFanHighlight(pointSource), null)

  const doc29 = northSegmentTrace()
  doc29.propagation.model = 'doc29'
  assert.equal(segmentFanHighlight(doc29), null)
})

test('segmentFanHighlight: null on degenerate geometry', () => {
  const onSegment = northSegmentTrace()
  onSegment.propagation.path_profile = { rcv_lat: 50.001078, rcv_lon: 14.0 }
  assert.equal(segmentFanHighlight(onSegment), null)

  const zeroLength = northSegmentTrace()
  zeroLength.end_lat = zeroLength.start_lat
  zeroLength.end_lon = zeroLength.start_lon
  assert.equal(segmentFanHighlight(zeroLength), null)
})
