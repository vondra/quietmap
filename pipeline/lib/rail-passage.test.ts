/** Sidecar writer: clipped visits, unknown freight, retract, no Arrow mutation. */

import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { test } from 'node:test'
import { listRailIntervals } from './rail-traffic-store.js'
import { writeClippedRailPassages, type RailServicePassages, type WriteClippedRailPassagesRequest } from './rail-passage.js'
import { RAIL_TEST_DIRECTORY, writePreparedRailwaySquare } from './rail-test-fixture.js'
import { writeSyntheticRailTopology } from './transport-test-fixture.js'
import { SOURCE_ID_GLOBAL_GTFS_TRANSIT } from './source-ids.generated.js'

const SQUARE = 'z9/276/173'
const SOURCE = SOURCE_ID_GLOBAL_GTFS_TRANSIT

test('clipped visits stay separate, unknown freight is omitted, Arrow bytes are unchanged', async () => {
  const prepared = join(RAIL_TEST_DIRECTORY, 'passage-clip')
  const path = writePreparedRailwaySquare(prepared, SQUARE, 'passage-clip.arrow', [
    { osmId: 50_000, segmentIndex: 0, latitude: 50, longitude: 14, country: 'DE' },
  ])
  writeSyntheticRailTopology(prepared, [SQUARE])
  const before = readFileSync(path)
  const result = await writeClippedRailPassages({
    preparedDirectory: prepared,
    squares: [SQUARE],
    countryIso: 'DE',
    sourceId: SOURCE,
    retractSafe: true,
    quarantinedPieceKeys: new Set(),
    services: [{
      evidence: {
        sourceId: SOURCE, passenger: 1.5, freight: 0,
        passengerStatus: 'estimated', freightStatus: 'unknown',
        matching: 'relation_estimated',
      },
      passages: [
        { wayId: '50000', segmentIndex: 0, square: SQUARE, fromM: 10, toM: 40, occurrence: 0 },
        { wayId: '50000', segmentIndex: 0, square: SQUARE, fromM: 10, toM: 40, occurrence: 1 },
      ],
    }],
  })
  assert.equal(result.walkStamped, 1)
  assert.deepEqual(readFileSync(path), before)
  const rows = listRailIntervals(prepared, SQUARE)
  assert.equal(rows.length, 2)
  assert.equal(rows[0].passenger, 1.5)
  assert.equal(rows[0].passengerStatus, 2)
  assert.equal(rows[0].freightStatus, 0)
  assert.equal(rows[0].matching, 1)
  assert.equal(rows[1].occurrence, 1)
})

test('retractSafe removes previous visits except quarantine across square owners', async () => {
  const prepared = join(RAIL_TEST_DIRECTORY, 'passage-retract')
  writePreparedRailwaySquare(prepared, SQUARE, 'passage-retract.arrow', [
    { osmId: 50_000, segmentIndex: 0, latitude: 50, longitude: 14, country: 'DE' },
  ])
  const otherSquare = 'z9/277/173'
  writePreparedRailwaySquare(prepared, otherSquare, 'passage-quarantine-other.arrow', [
    { osmId: 50_001, segmentIndex: 0, latitude: 50.01, longitude: 14, country: 'DE' },
  ])
  writeSyntheticRailTopology(prepared, [SQUARE, otherSquare])
  const request: WriteClippedRailPassagesRequest = {
    preparedDirectory: prepared,
    squares: [SQUARE, otherSquare],
    countryIso: 'DE',
    sourceId: SOURCE,
    retractSafe: true,
    quarantinedPieceKeys: new Set(['50001:0']),
    services: [{
      evidence: {
        sourceId: SOURCE, passenger: 8, freight: 0,
        passengerStatus: 'estimated', freightStatus: 'unknown',
        matching: 'graph_estimated',
      },
      passages: [
        { wayId: '50000', segmentIndex: 0, square: SQUARE, fromM: 0, toM: 50, occurrence: 0 },
      ],
    }],
  }
  await writeClippedRailPassages({ ...request, quarantinedPieceKeys: new Set(),
    services: [{ ...request.services[0], passages: [
      ...request.services[0].passages,
      { ...request.services[0].passages[0], wayId: '50001', square: otherSquare },
    ] }],
  })
  const preserved = listRailIntervals(prepared).filter(row => row.osmId === 50001)
  await writeClippedRailPassages({ ...request,
    services: [{ evidence: { ...request.services[0].evidence, passenger: 999 },
      passages: [{ ...request.services[0].passages[0], wayId: '50001', square: otherSquare }] }],
  })
  assert.deepEqual(listRailIntervals(prepared), preserved)
  const empty = await writeClippedRailPassages({ ...request, services: [] })
  assert.equal(empty.retracted, 0)
  assert.deepEqual(listRailIntervals(prepared), preserved)
})

test('different services sharing local visit zero sum, real returns repeat, and reruns replace', async () => {
  const prepared = join(RAIL_TEST_DIRECTORY, 'passage-services')
  const path = writePreparedRailwaySquare(prepared, SQUARE, 'passage-services.arrow', [
    { osmId: 50_000, segmentIndex: 0, latitude: 50, longitude: 14, country: 'DE' },
  ])
  writeSyntheticRailTopology(prepared, [SQUARE])
  const before = readFileSync(path)
  const service = (passenger: number, matching: 'relation_estimated' | 'graph_estimated'): RailServicePassages => ({
    evidence: { sourceId: SOURCE, passenger, freight: 99,
      passengerStatus: 'estimated', freightStatus: 'unknown', matching },
    passages: [{ wayId: '50000', segmentIndex: 0, square: SQUARE, fromM: 10, toM: 40, occurrence: 0 }],
  })
  const local = service(2, 'relation_estimated'), express = service(3, 'graph_estimated')
  const request: WriteClippedRailPassagesRequest = {
    preparedDirectory: prepared, squares: [SQUARE], countryIso: 'DE', sourceId: SOURCE,
    retractSafe: true, quarantinedPieceKeys: new Set<string>(), services: [local, express],
  }
  await writeClippedRailPassages(request)
  const first = listRailIntervals(prepared, SQUARE)
  assert.deepEqual(first.map(row => row.passenger), [2, 3])
  assert.equal(first.reduce((sum, row) => sum + row.passenger, 0), 5)
  assert.ok(first.every(row => row.freight === 0 && row.freightStatus === 0))
  assert.deepEqual(first.map(row => row.matching), [1, 2])
  for (const retractSafe of [true, false]) {
    await writeClippedRailPassages({ ...request, retractSafe })
    assert.deepEqual(listRailIntervals(prepared, SQUARE), first)
  }
  local.passages.push({ ...local.passages[0], fromM: 40, toM: 10, occurrence: 1 })
  await writeClippedRailPassages(request)
  const returning = listRailIntervals(prepared, SQUARE)
  assert.deepEqual(returning.map(row => row.passenger), [2, 2, 3])
  assert.equal(returning.reduce((sum, row) => sum + row.passenger, 0), 7)
  await writeClippedRailPassages(request)
  assert.deepEqual(listRailIntervals(prepared, SQUARE), returning)
  await writeClippedRailPassages({ ...request, services: [express] })
  assert.equal(listRailIntervals(prepared, SQUARE).reduce((sum, row) => sum + row.passenger, 0), 3)
  assert.deepEqual(readFileSync(path), before)
})
