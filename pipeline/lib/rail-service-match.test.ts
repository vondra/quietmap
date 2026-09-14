/** Complete OSM orders associate uniquely, stay ambiguous, or remain unmatched. */

import assert from 'node:assert/strict'
import { test } from 'node:test'
import { spawnSync } from 'node:child_process'
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
  const index = new CompleteTrainRouteIndex(stops)
  const wider = new CompleteTrainRouteIndex([...stops, { sourceStopId: 'far', name: 'far', lat: 40, lon: 0 }])
  const itinerary = route('1', [way('a', points)])
  for (const relation of [route('remote', [way('r', [[40, 0], [40, .04]])]),
    route('reverse', [way('a', points, true)]), itinerary, route('2', [way('a', points)])]) {
    index.add(relation)
    wider.add(relation)
    for (const query of [stops, [...stops].reverse()]) assert.deepEqual(index.associate(query), wider.associate(query))
  }
  assert.equal(index.associate(stops).status, 'ambiguous')
  const unique = new CompleteTrainRouteIndex(stops)
  unique.add(itinerary)
  const associated = unique.associate(stops)
  assert.equal(associated.status, 'unique_complete_candidate')
  assert.equal(associated.status === 'unique_complete_candidate' && associated.relation.id, '1')
  assert.deepEqual(relationWayPoints(itinerary), points)
  assert.equal(unique.associate([]).status, 'unmatched')
})

test('unqueried world itineraries do not accumulate in the station-scoped index', () => {
  const code = `
    import assert from 'node:assert/strict';
    import { CompleteTrainRouteIndex } from ${JSON.stringify(new URL('./rail-service-match.ts', import.meta.url).href)};
    const stops = [{ lat: 0, lon: 0, sourceStopId: 'a', name: 'a' },
      { lat: 0, lon: .04, sourceStopId: 'b', name: 'b' }];
    const index = new CompleteTrainRouteIndex(stops);
    const relation = (id, latitude) => ({ id, status: 'complete', missingWays: [], unsupportedMembers: [],
      ways: [{ id, role: '', reverse: false,
        nodes: Array.from({length: 100}, (_, n) => [String(n), [latitude, n * .001]]) }] });
    index.add(relation('first', 0));
    for (let n = 0; n < 20000; n++) index.add(relation(String(n), 40));
    const unique = index.associate(stops);
    assert.equal(unique.status, 'unique_complete_candidate');
    assert.equal(unique.relation.id, 'first');
    index.add(relation('second', 0));
    assert.equal(index.associate(stops).status, 'ambiguous');
  `
  const child = spawnSync(process.execPath, ['--max-old-space-size=64', '--import', import.meta.resolve('tsx'),
    '--input-type=module', '-e', code], { encoding: 'utf8', timeout: 30000 })
  assert.equal(child.status, 0, child.stderr)
})
