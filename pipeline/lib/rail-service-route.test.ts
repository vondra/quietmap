/** Whole-service routing prefers unique OSM orders and otherwise estimates on source edges. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { mkdtempSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { buildRailGraph, type RailGraphSegmentInput } from './rail-graph.js'
import { routeRailServices } from './rail-service-route.js'
import type { RailServicePassages } from './rail-passage.js'
import { SourceTransportTopology } from './transport-topology.js'
import { writeTransportFixture } from './transport-test-fixture.js'
import { flatDist, M_PER_DEG_LAT, M_PER_DEG_LON_EQ } from './spatial.js'
import type { GtfsService, GtfsServiceStop } from './gtfs-service-store.js'

const TEMP = mkdtempSync(join(tmpdir(), 'rail-service-route-'))
after(() => rmSync(TEMP, { recursive: true, force: true }))

const point = (x: number, y: number): [number, number] => [y / M_PER_DEG_LAT, x / M_PER_DEG_LON_EQ]
const square = 'z9/275/173'

/** Sum directed visits per piece — the writer's former accumulatePassageFlow
 *  moved into the final sidecar writer, so the routing tests keep their own. */
function passageFlow(services: readonly RailServicePassages[]): Map<string, { passenger: number; freight: number }> {
  const flow = new Map<string, { passenger: number; freight: number }>()
  for (const service of services) {
    for (const passage of service.passages) {
      if (passage.fromM === passage.toM) continue
      const key = `${passage.wayId}:${passage.segmentIndex}`
      const stamp = flow.get(key) ?? { passenger: 0, freight: 0 }
      stamp.passenger += service.evidence.passenger
      stamp.freight += service.evidence.freight
      flow.set(key, stamp)
    }
  }
  return flow
}

function stop(id: string, at: [number, number], sequence: number): GtfsServiceStop {
  return { sequence, sourceStopId: id, stopId: id, lat: at[0], lon: at[1], name: id,
    arrivalTime: '', departureTime: '', shapeDistance: null }
}

function service(
  tripId: string,
  stops: GtfsServiceStop[],
  shape: Array<[number, number]>,
  passenger = 1,
): GtfsService {
  return {
    tripId, routeId: 'R', serviceId: 'S', directionId: '0', shapeId: tripId,
    departureMultiplier: passenger, stops, shape: shape.map((at, sequence) => (
      { sequence, lat: at[0], lon: at[1], shapeDistance: null }
    )),
  }
}

function segment(
  way: string,
  from: [number, number],
  to: [number, number],
  startNode: string,
  endNode: string,
): RailGraphSegmentInput {
  return {
    key: `${way}:0`, osmId: way, railType: 0, usage: 0, isTraversalOnly: false, corridorToken: way,
    startKey: `node:${startNode}`, endKey: `node:${endNode}`,
    startLat: from[0], startLon: from[1], endLat: to[0], endLon: to[1],
    lengthM: flatDist(...from, ...to),
  }
}

test('a unique complete OSM order clips the aligned itinerary; a duplicate order stays estimated', () => {
  const a = point(0, 0), b = point(1000, 0)
  const uniqueDir = join(TEMP, 'relation-unique')
  writeTransportFixture(uniqueDir, [
    { id: '10', nodes: [['1', a], ['2', b]] },
  ], [
    { way: '10', segment: 0, square, start: [0, 0], end: [1, 0] },
  ], [], [
    { id: '50', members: [['w', '10', 'forward']] },
  ])
  using topology = new SourceTransportTopology(uniqueDir)
  const graph = buildRailGraph([segment('10', a, b, '1', '2')])
  const trip = service('T', [stop('A', a, 1), stop('B', b, 2)], [a, b], 10)
  const unique = routeRailServices([trip], topology, graph, 100)
  assert.equal(unique.relationEstimated, 1)
  assert.equal(unique.services[0].evidence.matching, 'relation_estimated')
  assert.equal(unique.services[0].evidence.relationId, '50')
  assert.equal(unique.services[0].evidence.freightStatus, 'unknown')
  assert.equal(unique.services[0].passages.length, 1)
  assert.equal(unique.services[0].passages[0].wayId, '10')

  const ambiguousDir = join(TEMP, 'relation-ambiguous')
  writeTransportFixture(ambiguousDir, [
    { id: '10', nodes: [['1', a], ['2', b]] },
  ], [
    { way: '10', segment: 0, square, start: [0, 0], end: [1, 0] },
  ], [], [
    { id: '50', members: [['w', '10', 'forward']] },
    { id: '51', members: [['w', '10', 'forward']] },
  ])
  using ambiguous = new SourceTransportTopology(ambiguousDir)
  const estimated = routeRailServices([trip], ambiguous, graph, 100)
  assert.equal(estimated.relationEstimated, 0)
  assert.equal(estimated.graphEstimated, 1)
  assert.equal(estimated.services[0].evidence.matching, 'graph_estimated')
  assert.equal(estimated.services[0].evidence.relationId, undefined)
})

test('nearest track, not nearest node, carries the estimated service; a return keeps two visits', () => {
  const start = point(0, 0), end = point(2000, 0), mid = point(1000, 5)
  const wrongA = point(1000, 80), wrongB = point(1100, 80)
  const prepared = join(TEMP, 'edges')
  writeTransportFixture(prepared, [
    { id: '10', nodes: [['1', start], ['2', end]] },
    { id: '11', nodes: [['3', wrongA], ['4', wrongB]] },
  ], [
    { way: '10', segment: 0, square, start: [0, 0], end: [1, 0] },
    { way: '11', segment: 0, square, start: [0, 0], end: [1, 0] },
  ])
  using topology = new SourceTransportTopology(prepared)
  const graph = buildRailGraph([
    segment('10', start, end, '1', '2'),
    segment('11', wrongA, wrongB, '3', '4'),
  ])
  const through = service('out', [stop('A', start, 1), stop('M', mid, 2), stop('B', end, 3)], [start, mid, end])
  const routed = routeRailServices([through], topology, graph, 100)
  assert.equal(routed.graphEstimated, 1)
  assert.deepEqual([...new Set(routed.services[0].passages.map(passage => passage.wayId))], ['10'])

  const returnDir = join(TEMP, 'return')
  writeTransportFixture(returnDir, [
    { id: '10', nodes: [['1', start], ['2', end]] },
  ], [
    { way: '10', segment: 0, square, start: [0, 0], end: [1, 0] },
  ], [], [
    { id: '70', members: [['w', '10', 'forward'], ['w', '10', 'backward']] },
  ])
  using returning = new SourceTransportTopology(returnDir)
  const loop = service('loop',
    [stop('A', start, 1), stop('B', end, 2), stop('C', start, 3)],
    [start, end, start])
  const back = routeRailServices([loop], returning, buildRailGraph([segment('10', start, end, '1', '2')]), 100)
  assert.equal(back.relationEstimated, 1)
  assert.equal(back.services[0].passages.length, 2)
  const flow = passageFlow(back.services)
  assert.equal(flow.get('10:0')?.passenger, 2)
})

test('an intermediate shape return inside one leg retraces its shared track twice', () => {
  const a = point(0, 0), quarter = point(500, 0), b = point(1000, 0), spur = point(1500, 0)
  const prepared = join(TEMP, 'reversal')
  writeTransportFixture(prepared, [
    { id: '20', nodes: [['1', a], ['2', quarter], ['3', b], ['4', spur]] },
  ], [
    { way: '20', segment: 0, square, start: [0, 0], end: [1, 0] },
    { way: '20', segment: 1, square, start: [1, 0], end: [2, 0] },
    { way: '20', segment: 2, square, start: [2, 0], end: [3, 0] },
  ])
  using topology = new SourceTransportTopology(prepared)
  const graph = buildRailGraph([
    segment('20', a, quarter, '1', '2'),
    { ...segment('20', quarter, b, '2', '3'), key: '20:1' },
    { ...segment('20', b, spur, '3', '4'), key: '20:2' },
  ])
  const trip = service('T', [stop('A', a, 1), stop('B', b, 2)], [a, spur, b])
  const routed = routeRailServices([trip], topology, graph, 100)
  assert.equal(routed.graphEstimated, 1)
  const flow = passageFlow(routed.services)
  assert.equal(flow.get('20:0')?.passenger, 1)
  assert.equal(flow.get('20:1')?.passenger, 1)
  assert.equal(flow.get('20:2')?.passenger, 2)
})

test('mid-edge projections route through both snap-edge endpoints, not the nearer node', () => {
  const x = point(0, 0), m = point(2000, 0), w = point(1200, 800), y = point(2000, 1000)
  const s1 = point(400, 0), s2 = point(2000, 600)
  const prepared = join(TEMP, 'fractional')
  writeTransportFixture(prepared, [
    { id: '30', nodes: [['x', x], ['m', m]] },
    { id: '31', nodes: [['x', x], ['w', w]] },
    { id: '32', nodes: [['w', w], ['y', y]] },
    { id: '33', nodes: [['m', m], ['y', y]] },
  ], [
    { way: '30', segment: 0, square, start: [0, 0], end: [1, 0] },
    { way: '31', segment: 0, square, start: [0, 0], end: [1, 0] },
    { way: '32', segment: 0, square, start: [0, 0], end: [1, 0] },
    { way: '33', segment: 0, square, start: [0, 0], end: [1, 0] },
  ])
  using topology = new SourceTransportTopology(prepared)
  const graph = buildRailGraph([
    segment('30', x, m, 'x', 'm'),
    segment('31', x, w, 'x', 'w'),
    segment('32', w, y, 'w', 'y'),
    segment('33', m, y, 'm', 'y'),
  ])
  // No GTFS shape: the walk must stay shape-less (no stop-polyline corridor),
  // and the shortcut x->w->y (2613 m) must lose to the fractional solution
  // 400 m along way 30 plus 600 m along way 33 even though the nearer nodes
  // of both snapped edges lead onto the shortcut.
  const trip = service('F', [stop('S1', s1, 1), stop('S2', s2, 2)], [])
  const routed = routeRailServices([trip], topology, graph, 100)
  assert.equal(routed.graphEstimated, 1)
  const flow = passageFlow(routed.services)
  assert.equal(flow.get('30:0')?.passenger, 1)
  assert.equal(flow.get('33:0')?.passenger, 1)
  assert.equal(flow.get('31:0'), undefined)
  assert.equal(flow.get('32:0'), undefined)
  const passages = routed.services[0].passages
  assert.equal(passages.length, 2)
  assert.ok(Math.abs(passages[0].fromM - 400) < 1)
  assert.ok(Math.abs(passages[0].toM - 2000) < 1)
  assert.ok(Math.abs(passages[1].toM - 600) < 1)
})

test('north and south shapes keep independent estimated counts', () => {
  const west = point(0, 0), east = point(1000, 0)
  const north = [west, point(500, 500), east]
  const south = [west, point(500, -500), east]
  const prepared = join(TEMP, 'diamond')
  writeTransportFixture(prepared, [
    { id: '1', nodes: [['a', north[0]], ['b', north[1]], ['c', north[2]]] },
    { id: '2', nodes: [['d', south[0]], ['e', south[1]], ['f', south[2]]] },
  ], [
    { way: '1', segment: 0, square, start: [0, 0], end: [1, 0] },
    { way: '1', segment: 1, square, start: [1, 0], end: [2, 0] },
    { way: '2', segment: 0, square, start: [0, 0], end: [1, 0] },
    { way: '2', segment: 1, square, start: [1, 0], end: [2, 0] },
  ])
  using topology = new SourceTransportTopology(prepared)
  const graph = buildRailGraph([
    segment('1', north[0], north[1], 'a', 'b'),
    { ...segment('1', north[1], north[2], 'b', 'c'), key: '1:1' },
    segment('2', south[0], south[1], 'd', 'e'),
    { ...segment('2', south[1], south[2], 'e', 'f'), key: '2:1' },
  ])
  const routed = routeRailServices([
    service('N', [stop('W', west, 1), stop('E', east, 2)], north, 10),
    service('S', [stop('W', west, 1), stop('E', east, 2)], south, 20),
  ], topology, graph, 100)
  const flow = passageFlow(routed.services)
  assert.equal(routed.graphEstimated, 2)
  assert.equal((flow.get('1:0')?.passenger ?? 0) + (flow.get('1:1')?.passenger ?? 0), 20)
  assert.equal((flow.get('2:0')?.passenger ?? 0) + (flow.get('2:1')?.passenger ?? 0), 40)
})

test('failed pattern summaries preserve the cause and sum daily departures across equal patterns', () => {
  const a = point(0, 0), b = point(1000, 0), c = point(2000, 0), d = point(3000, 0)
  const prepared = join(TEMP, 'failure-summary')
  writeTransportFixture(prepared, [
    { id: '1', nodes: [['a', a], ['b', b]] },
    { id: '2', nodes: [['c', c], ['d', d]] },
  ], [
    { way: '1', segment: 0, square, start: [0, 0], end: [1, 0] },
    { way: '2', segment: 0, square, start: [0, 0], end: [1, 0] },
  ])
  using topology = new SourceTransportTopology(prepared)
  const graph = buildRailGraph([segment('1', a, b, 'a', 'b'), segment('2', c, d, 'c', 'd')])
  const disconnected = [stop('A', a, 1), stop('D', d, 2)]
  const far = point(10000, 10000)
  const routed = routeRailServices([
    service('D1', disconnected, [a, d], 2),
    service('D2', disconnected, [a, d], 3.5),
    service('S', [stop('A', a, 1), stop('F', far, 2)], [a, far], 7),
    service('G', [stop('A', a, 1), stop('B', b, 2)], [a, b], 11),
  ], topology, graph, 100)
  assert.equal(routed.total, 3)
  assert.equal(routed.unmatched, 2)
  assert.deepEqual(routed.failures, { snapFailed: 1, disconnected: 1, ambiguous: 0 })
  assert.deepEqual(routed.dailyDepartures, {
    total: 23.5, relationEstimated: 0, graphEstimated: 11, unmatched: 12.5,
    failures: { snapFailed: 7, disconnected: 5.5, ambiguous: 0 },
  })
})

test('joint station snaps use the connected through track when both closest tracks are isolated', () => {
  const a = point(0, 0), b = point(2000, 0), throughA = point(0, 20), throughB = point(2000, 20)
  const leftEnd = point(100, 0), rightStart = point(1900, 0)
  const prepared = join(TEMP, 'joint-station-snaps')
  writeTransportFixture(prepared, [
    { id: '1', nodes: [['a', a], ['l', leftEnd]] },
    { id: '2', nodes: [['r', rightStart], ['b', b]] },
    { id: '3', nodes: [['ta', throughA], ['tb', throughB]] },
  ], [1, 2, 3].map(way => ({ way: String(way), segment: 0, square,
    start: [0, 0] as [number, number], end: [1, 0] as [number, number] })))
  using topology = new SourceTransportTopology(prepared)
  const graph = buildRailGraph([
    segment('1', a, leftEnd, 'a', 'l'), segment('2', rightStart, b, 'r', 'b'),
    segment('3', throughA, throughB, 'ta', 'tb'),
  ])
  const routed = routeRailServices([service('T', [stop('A', a, 1), stop('B', b, 2)], [a, b], 7)], topology, graph, 100)
  assert.equal(routed.graphEstimated, 1)
  assert.equal(routed.unmatched, 0)
  assert.deepEqual(routed.services[0].passages.map(passage => passage.wayId), ['3'])
  assert.equal(routed.services[0].evidence.passenger, 7)
  assert.equal(routed.services[0].evidence.matching, 'graph_estimated')
})

test('station-only shapes use shapeless routing while intermediate geometry remains authoritative', () => {
  const a = point(0, 0), b = point(2000, 0), north = point(1000, 1000), south = point(1000, -1000)
  const prepared = join(TEMP, 'station-only-shape')
  writeTransportFixture(prepared, [
    { id: '1', nodes: [['a', a], ['n', north], ['b', b]] },
    { id: '2', nodes: [['a', a], ['s', south], ['b', b]] },
  ], [1, 2].flatMap(way => [0, 1].map(index => ({
    way: String(way), segment: index, square,
    start: [index, 0] as [number, number], end: [index + 1, 0] as [number, number],
  }))))
  using topology = new SourceTransportTopology(prepared)
  const northSegments = [segment('1', a, north, 'a', 'n'),
    { ...segment('1', north, b, 'n', 'b'), key: '1:1' }]
  const curvedGraph = buildRailGraph(northSegments)
  const stops = [stop('A', a, 1), stop('A-platform', a, 2), stop('B', b, 3)]
  const stationLine = service('station-line', stops, [a, a, b, b], 7)
  const recovered = routeRailServices([stationLine], topology, curvedGraph, 100)
  assert.equal(recovered.dailyDepartures.graphEstimated, 7)
  assert.deepEqual(recovered.services[0].passages.map(passage => passage.wayId), ['1', '1'])

  const endpoints = [stops[0], stops[2]]
  const unsupportedShape = service('south-shape', endpoints, [a, south, b], 7)
  assert.equal(routeRailServices([unsupportedShape], topology, curvedGraph, 100).unmatched, 1)

  const bothCurves = buildRailGraph([...northSegments, segment('2', a, south, 'a', 's'),
    { ...segment('2', south, b, 's', 'b'), key: '2:1' }])
  assert.equal(routeRailServices([stationLine], topology, bothCurves, 100).failures.ambiguous, 1)
  const genuineShape = service('north-shape', endpoints, [a, north, b], 7)
  const aligned = routeRailServices([genuineShape], topology, bothCurves, 100)
  assert.equal(aligned.dailyDepartures.graphEstimated, 7)
  assert.deepEqual(aligned.services[0].passages.map(passage => passage.wayId), ['1', '1'])
})

test('shapeless services accept parallel tracks but keep distinct corridors ambiguous', () => {
  for (const spacingM of [8, 120]) {
    const prepared = join(TEMP, `parallel-${spacingM}`)
    const direct: Array<[string, [number, number]]> = Array.from({ length: 21 },
      (_, index) => [String(index + 1), point(index * 250, 0)])
    const parallel: typeof direct = direct.map((_, index) => [String(index + 101), point(index * 250, spacingM)])
    const ways = [
      { id: '10', nodes: direct }, { id: '20', nodes: parallel },
      { id: '30', nodes: [direct[0], parallel[0]] },
      { id: '40', nodes: [direct[20], parallel[20]] },
    ]
    const pieces = ways.flatMap(way => way.nodes.slice(1).map((_, index) => ({
      way: way.id, segment: index, square,
      start: [index, 0] as [number, number], end: [index + 1, 0] as [number, number],
    })))
    writeTransportFixture(prepared, ways, pieces)
    using topology = new SourceTransportTopology(prepared)
    const graph = buildRailGraph(ways.flatMap(way => way.nodes.slice(1).map((node, index) => ({
      ...segment(way.id, way.nodes[index][1], node[1], way.nodes[index][0], node[0]),
      key: `${way.id}:${index}`, isTraversalOnly: way.id === '30' || way.id === '40',
    }))))
    const trip = service('T', [stop('A', direct[0][1], 1), stop('B', direct[20][1], 2)], [], 20)
    const result = routeRailServices([trip], topology, graph, 100)
    const expected = spacingM === 8 ? 20 : 0
    assert.equal(result.dailyDepartures.graphEstimated, expected, `${spacingM} m separation`)
    assert.equal(result.dailyDepartures.failures.ambiguous, 20 - expected)
    if (expected) {
      assert.equal(result.services[0].evidence.matching, 'graph_estimated')
      assert.equal(result.services[0].passages.length, 20)
      for (const flow of passageFlow(result.services).values()) assert.equal(flow.passenger, 20)
    } else {
      assert.equal(result.services.length, 0)
    }
  }
})
