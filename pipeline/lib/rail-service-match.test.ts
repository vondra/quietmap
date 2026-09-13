/** Complete OSM orders associate uniquely, stay ambiguous, or remain unmatched. */

import assert from 'node:assert/strict'
import { test } from 'node:test'
import { CompleteTrainRouteIndex, matchStopOrder, relationWayPoints } from './rail-service-match.js'
import type { OrientedSourceRailWay, SourceTrainRoute } from './transport-topology.js'

const points: Array<[number, number]> = [[0, 0], [0, 0.02], [0, 0.04]]
const stops = points.map(([lat, lon], index) => (
  { sourceStopId: String(index), name: String(index), lat, lon }
))

function route(id: string, ways: OrientedSourceRailWay[]): SourceTrainRoute {
  return { id, status: 'complete', missingWays: [], unsupportedMembers: [], ways }
}

function way(id: string, geometry: Array<[number, number]>, reverse = false): OrientedSourceRailWay {
  return {
    id, role: '', reverse,
    nodes: geometry.map((coordinate, index) => [String(index), coordinate]),
  }
}

test('stop order rejects a reverse itinerary and treats duplicate complete fits as ambiguous', () => {
  const forward = matchStopOrder(stops, points)
  const backward = matchStopOrder(stops, [...points].reverse())
  assert.ok(forward)
  assert.equal(backward, null)
  const index = new CompleteTrainRouteIndex()
  const itinerary = route('1', [way('a', points)])
  index.add(itinerary)
  index.add(route('2', [way('a', points)]))
  assert.equal(index.associate(stops).status, 'ambiguous')
  const unique = new CompleteTrainRouteIndex()
  unique.add(itinerary)
  const associated = unique.associate(stops)
  assert.equal(associated.status, 'unique_complete_candidate')
  assert.equal(associated.status === 'unique_complete_candidate' && associated.relation.id, '1')
  assert.deepEqual(relationWayPoints(itinerary), points)
  assert.equal(unique.associate([]).status, 'unmatched')
})
