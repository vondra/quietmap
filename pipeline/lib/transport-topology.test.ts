/** Original node identity survives Arrow row order, interpolation and colocated independent ways. */

import assert from 'node:assert/strict'
import { after, mock, test } from 'node:test'
import fs, { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { writeRailwaysFixture, type RailwayFixtureRow } from './rail-test-fixture.js'
import { writeTransportFixture, type FixtureSourceWay, type FixtureSourcePiece } from './transport-test-fixture.js'
import { M_PER_DEG_LON_EQ } from './spatial.js'
import { arrowStreamBatches, railwayGlobalPath } from './railway-globals.js'
import { SourceTransportTopology } from './transport-topology.js'
import { collectZ9RailGraphSegments, enrichZ9RailwaysByGraphWalk } from './rail-walk-enrich.js'

const TEMP = mkdtempSync(join(tmpdir(), 'source-transport-'))
after(() => rmSync(TEMP, { recursive: true, force: true }))
const square = 'z9/275/173'

function arrow(prepared: string, rows: RailwayFixtureRow[]): string {
  const directory = join(prepared, square)
  mkdirSync(directory, { recursive: true })
  const path = join(directory, 'railways.arrow')
  copyFileSync(writeRailwaysFixture('source-topology.arrow', rows), path)
  return path
}

const row = (osmId: bigint, segmentIndex: number, start: number, end: number): RailwayFixtureRow => ({
  osmId, segmentIndex, latitude: 50, longitude: start, endLatitude: 50, endLongitude: end,
})

test('source identities preserve large IDs, fractional joins and explicit zero hops independently of row order', () => {
  const prepared = join(TEMP, 'identities')
  const id = '9007199254740993'
  const rows = [row(BigInt(id), 0, 14, 14.001), row(BigInt(id), 1, 14.001, 14.002),
    row(11n, 0, 14.002, 14.003), row(12n, 0, 14.002, 14.004)]
  arrow(prepared, rows)
  const ways: FixtureSourceWay[] = [
    { id, nodes: [['1', [50, 14]], ['2', [50, 14.002]]] },
    { id: '11', nodes: [['3', [50, 14.002]], ['4', [50, 14.003]]] },
    { id: '12', nodes: [['5', [50, 14.002]], ['6', [50, 14.004]]] },
    { id: '13', nodes: [['2', [50, 14.002]], ['3', [50, 14.002]]] },
  ]
  const pieces: FixtureSourcePiece[] = [
    { way: id, segment: 0, square, start: [0, 0], end: [0, 0.5] },
    { way: id, segment: 1, square, start: [0, 0.5], end: [1, 0] },
    { way: '11', segment: 0, square, start: [0, 0], end: [1, 0] },
    { way: '12', segment: 0, square, start: [0, 0], end: [1, 0] },
  ]
  writeTransportFixture(prepared, ways, pieces, [['railways', '3', '2'], ['roads', '5', '2']])
  using topology = new SourceTransportTopology(prepared)
  const stored = topology.squarePieces(square)
  assert.deepEqual(Array.from({ length: stored.count }, (_, piece) => stored.key(piece)), ['11:0', '12:0', `${id}:0`, `${id}:1`])
  assert.equal(stored.row('11', 0), 0)
  assert.equal(stored.row('11', 1), -1)
  const first = collectZ9RailGraphSegments(prepared, [square])
  assert.deepEqual(first.map(({ key, startKey, endKey }) => [key, startKey, endKey]), [
    [`${id}:0`, 'node:1', `way:${id}:0+0.5`],
    [`${id}:1`, `way:${id}:0+0.5`, 'node:2'],
    ['11:0', 'node:2', 'node:4'],
    ['12:0', 'node:5', 'node:6'],
  ])
  arrow(prepared, [...rows].reverse())
  assert.deepEqual(collectZ9RailGraphSegments(prepared, [square]), [...first].reverse())
})

test('missing or unmatched topology fails before existing traffic is changed', async () => {
  const prepared = join(TEMP, 'incomplete')
  const path = arrow(prepared, [{ ...row(10n, 0, 14, 14.001), sourceId: 100 }])
  const before = readFileSync(path)
  const options = { preparedDirectory: prepared, bbox: [49.99, 13.99, 50.01, 14.01] as const,
    pairs: [], sourceId: 100, countryIso: 'CD', retractSafe: true }
  await assert.rejects(enrichZ9RailwaysByGraphWalk(options), /source topology missing/)
  assert.deepEqual(readFileSync(path), before)
  writeTransportFixture(prepared, [{ id: '10', nodes: [['1', [50, 14]], ['2', null]] },
    { id: '11', nodes: [['1', [50, 14]], ['2', [50, 14.001]]] }],
  [{ way: '11', segment: 0, square, start: [0, 0], end: [1, 0] }])
  await assert.rejects(enrichZ9RailwaysByGraphWalk(options), /source topology missing/)
  assert.deepEqual(readFileSync(path), before)
  using roads = new SourceTransportTopology(prepared, 'roads')
  writeFileSync(join(prepared, square, 'roads.pieces.arrow'), readFileSync(join(prepared, square, 'railways.pieces.arrow')))
  assert.throws(() => roads.squarePieces(square), /not a roads pieces file/)
})

test('train routes preserve ordered occurrences and explicit aliases; unresolved source orders stay unresolved', () => {
  const large = '9007199254740993'
  const ways: FixtureSourceWay[] = [
    { id: large, nodes: [['1', [0, 0]], ['2', [0, 1]]] },
    { id: '11', nodes: [['3', [0, 1]], ['4', [0, 2]]] },
    { id: '12', nodes: [['5', [0, 1]], ['6', [0, 2]]] },
    { id: '13', nodes: [['4', [0, 2]], ['7', null]] },
    { id: '14', family: 'roads', nodes: [['2', [0, 1]], ['4', [0, 2]]] },
    { id: '15', nodes: [['2', [0, 1]], ['3', [0, 1]]] },
  ]
  const member = (id: string, role = ''): [string, string, string] => ['w', id, role]
  const prepared = join(TEMP, 'routes')
  writeTransportFixture(prepared, ways, [], [['railways', '3', '2'], ['roads', '5', '2']], [
    { id: large, members: [['n', '999', 'stop'], member(large, 'forward'),
      member('11'), member('11'), member(large, 'backward'), ['w', '999', 'platform']] },
    { id: '1', members: [member(large, 'forward'), member('12')] },
    { id: '2', members: [member('13'), member('999'), member('14')] },
    { id: '3', members: [['r', '10', ''], member('11', 'unknown')] },
    { id: '4', members: [member(large)] },
    { id: '5', members: [] },
    { id: '6', members: Array.from({ length: 1100 }, () => member('15')) },
  ])
  using topology = new SourceTransportTopology(prepared)
  const routes = new Map([...topology.trainRoutes()].map(route => [route.id, route]))
  const route = routes.get(large)!
  assert.equal(route.status, 'complete')
  assert.deepEqual(route.ways.map(way => [way.id, way.role, way.reverse]), [
    [large, 'forward', false], ['11', '', false], ['11', '', true], [large, 'backward', true],
  ])
  assert.equal(route.ways[1].nodes[0][0], '2')
  assert.equal(routes.get('1')!.status, 'ambiguous_or_disconnected_order')
  assert.deepEqual(routes.get('2')!.missingWays, ['w13', 'w999', 'w14'])
  assert.equal(routes.get('2')!.status, 'missing_source_ways')
  assert.deepEqual(routes.get('3')!.unsupportedMembers, [['r10', ''], ['w11', 'unknown']])
  assert.equal(routes.get('3')!.status, 'unsupported_members')
  for (const id of ['4', '5', '6']) {
    assert.equal(routes.get(id)!.status, 'ambiguous_or_disconnected_order')
    assert.deepEqual(routes.get(id)!.ways, [])
  }
  assert.deepEqual([...routes.keys()], ['1', '2', '3', '4', '5', '6', large])
})

test('physical passages clip acoustic pieces across squares and retain the ordered return', () => {
  const prepared = join(TEMP, 'passage-pieces')
  const id = '9007199254740993', neighbor = 'z9/276/173'
  const way: FixtureSourceWay = { id, nodes: [['1', [0, 0]], ['2', [0, 600 / M_PER_DEG_LON_EQ]], ['3', [0, 1000 / M_PER_DEG_LON_EQ]]] }
  const pieces: FixtureSourcePiece[] = [
    { way: id, segment: 30, square: neighbor, start: [1, .5], end: [2, 0] },
    { way: id, segment: 10, square, start: [0, 0], end: [1, 0] },
    { way: id, segment: 20, square: neighbor, start: [1, 0], end: [1, .5] },
  ]
  writeTransportFixture(prepared, [way], pieces)
  using topology = new SourceTransportTopology(prepared)
  assert.deepEqual(topology.squarePieces(neighbor).wayRows(id), [0, 2])
  const forward = topology.passagePieces({ way: id, from: 100, to: 900 })
  assert.deepEqual(forward, [
    { segmentIndex: 10, square, from: 100, to: 600 },
    { segmentIndex: 20, square: neighbor, from: 600, to: 800 },
    { segmentIndex: 30, square: neighbor, from: 800, to: 900 },
  ])
  assert.deepEqual(topology.passagePieces({ way: id, from: 900, to: 100 }),
    [...forward].reverse().map(piece => ({ ...piece, from: piece.to, to: piece.from })))
  const outward = topology.passagePieces({ way: id, from: 0, to: 600.25 })
  const inward = topology.passagePieces({ way: id, from: 600.25, to: 0 })
  assert.deepEqual(outward.map(piece => [piece.segmentIndex, piece.from, piece.to]), [[10, 0, 600], [20, 600, 600.25]])
  assert.deepEqual(inward.map(piece => [piece.segmentIndex, piece.from, piece.to]), [[20, 600.25, 600], [10, 600, 0]])
  assert.equal([...outward, ...inward].reduce((sum, piece) => sum + Math.abs(piece.to - piece.from), 0), 1200.5)
  assert.deepEqual(topology.pieceExtent(id, 10), { square, from: 0, to: 600 })
  assert.throws(() => topology.passagePieces({ way: id, from: 0, to: 1001 }), /invalid source passage/)
  assert.deepEqual(topology.pieceExtent(id, 30, neighbor), { square: neighbor, from: 800, to: 1000 })
  writeTransportFixture(prepared, [way], [pieces[0], pieces[1]])
  using gapped = new SourceTransportTopology(prepared)
  assert.throws(() => gapped.passagePieces({ way: id, from: 100, to: 900 }), /gap or overlap/)
  writeTransportFixture(prepared, [way], [pieces[0], pieces[1], { ...pieces[2], start: [0, 0.5], end: [2, 0] }])
  using overlapping = new SourceTransportTopology(prepared)
  assert.throws(() => overlapping.passagePieces({ way: id, from: 100, to: 900 }), /gap or overlap/)
})

test('railway ways stream batch by batch through bounded reads', () => {
  const prepared = join(TEMP, 'bounded-reads')
  const ways: FixtureSourceWay[] = Array.from({ length: 5 }, (_, way) =>
    ({ id: String(way + 1), nodes: [[`${way}a`, [0, way]], [`${way}b`, [0, way + 1]]] }))
  writeTransportFixture(prepared, ways, [])
  const reads = mock.method(fs, 'readSync')
  try {
    const batches = [...arrowStreamBatches(railwayGlobalPath(prepared, 'railway-ways'), 'railway_ways_contract', 64)]
    assert.deepEqual(batches.map(batch => [...batch.getChild('way_id')!].map(String)), [['1', '2'], ['3', '4'], ['5']])
    assert.ok(reads.mock.calls.length > 3)
    for (const call of reads.mock.calls) assert.ok(((call.arguments as unknown[])[3] as number) <= 64)
  } finally { reads.mock.restore() }
})

test('the railway square cache evicts by bytes, so a world crossing never retains the world', () => {
  const prepared = join(TEMP, 'cache-bytes'), neighbor = 'z9/276/173'
  const way = (id: string): FixtureSourceWay => ({ id, nodes: [['1', [0, 0]], ['2', [0, 1]]] })
  const piece = (id: string, target: string): FixtureSourcePiece => ({ way: id, segment: 0, square: target, start: [0, 0], end: [1, 0] })
  writeTransportFixture(prepared, [way('1'), way('2')], [piece('1', square), piece('2', neighbor)])
  using cached = new SourceTransportTopology(prepared)
  using evicting = new SourceTransportTopology(prepared, 'railways', 1)
  for (const topology of [cached, evicting]) assert.equal(topology.squarePieces(square).row('1', 0), 0)
  writeTransportFixture(prepared, [way('1'), way('2')], [piece('2', neighbor)])
  rmSync(join(prepared, square, 'railways.pieces.arrow'))
  for (const topology of [cached, evicting]) assert.equal(topology.squarePieces(neighbor).count, 1)
  assert.equal(cached.squarePieces(square).count, 1)
  assert.equal(evicting.squarePieces(square).count, 0)
})
