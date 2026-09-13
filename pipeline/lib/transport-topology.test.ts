/** Original node identity survives Arrow row order, interpolation and colocated independent ways. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { DatabaseSync } from 'node:sqlite'
import { writeRailwaysFixture, type RailwayFixtureRow } from './rail-test-fixture.js'
import { writeTransportFixture, type FixtureSourceWay, type FixtureSourcePiece } from './transport-test-fixture.js'
import { SourceTransportTopology, transportTopologyPath } from './transport-topology.js'
import { collectZ9RailGraphSegments, enrichZ9RailwaysByGraphWalk } from './rail-walk-enrich.js'

const TEMP = mkdtempSync(join(tmpdir(), 'source-transport-'))
after(() => rmSync(TEMP, { recursive: true, force: true }))
const square = 'z9/275/173'

function arrow(prepared: string, rows: RailwayFixtureRow[]): string {
  const directory = join(prepared, square)
  mkdirSync(directory, { recursive: true })
  const path = join(directory, 'railways.arrow')
  copyFileSync(writeRailwaysFixture('source-topology.arrow', rows, { includeTraffic: true }), path)
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

test('missing, incomplete or unmatched topology fails before existing traffic is changed', async () => {
  const prepared = join(TEMP, 'incomplete')
  const path = arrow(prepared, [{ ...row(10n, 0, 14, 14.001), sourceId: 100, passenger: 7 }])
  const before = readFileSync(path)
  const options = { preparedDirectory: prepared, bbox: [49.99, 13.99, 50.01, 14.01] as const,
    pairs: [], sourceId: 100, countryIso: 'CD', retractSafe: true }
  await assert.rejects(enrichZ9RailwaysByGraphWalk(options), /unable to open database/)
  assert.deepEqual(readFileSync(path), before)
  writeTransportFixture(prepared, [{ id: '10', nodes: [['1', [50, 14]], ['2', null]] }], [])
  await assert.rejects(enrichZ9RailwaysByGraphWalk(options), /source topology missing/)
  assert.deepEqual(readFileSync(path), before)
  {
    using database = new DatabaseSync(transportTopologyPath(prepared))
    database.exec("INSERT INTO source_pieces VALUES (10, 0, 'z9/275/173', 0, 0, 1, 0)")
  }
  await assert.rejects(enrichZ9RailwaysByGraphWalk(options), /invalid source position/)
  assert.deepEqual(readFileSync(path), before)
  {
    using database = new DatabaseSync(transportTopologyPath(prepared))
    database.exec('PRAGMA user_version = 0')
  }
  assert.throws(() => new SourceTransportTopology(prepared), /incomplete or.*unsupported/)
})
