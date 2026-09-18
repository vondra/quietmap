/** Interval-file writer: clipped visits, unknown freight, retract, no Arrow mutation. */

import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { test } from 'node:test'
import { tableFromIPC } from 'apache-arrow'
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

test('accepted passages replace their source on quarantine without claiming complete traffic', async () => {
  const prepared = join(RAIL_TEST_DIRECTORY, 'passage-quarantine')
  const path = writePreparedRailwaySquare(prepared, SQUARE, 'passage-quarantine.arrow', [
    { osmId: 50_000, segmentIndex: 0, latitude: 50, longitude: 14, country: 'DE' },
  ])
  const otherSquare = 'z9/277/173'
  const otherPath = writePreparedRailwaySquare(prepared, otherSquare, 'passage-quarantine-other.arrow', [
    { osmId: 50_001, segmentIndex: 0, latitude: 50.01, longitude: 14, country: 'DE' },
  ])
  writeSyntheticRailTopology(prepared, [SQUARE, otherSquare])
  const before = [readFileSync(path), readFileSync(otherPath)]
  const service = (passenger: number, fromM: number): RailServicePassages => ({
    evidence: { sourceId: SOURCE, passenger, freight: 0,
      passengerStatus: 'known', freightStatus: 'unknown', matching: 'relation_estimated' },
    passages: [{ wayId: '50001', segmentIndex: 0, square: otherSquare,
      fromM, toM: 50, occurrence: 0 }],
  })
  const request: WriteClippedRailPassagesRequest = {
    preparedDirectory: prepared, squares: [SQUARE, otherSquare], countryIso: 'DE',
    sourceId: SOURCE, retractSafe: true, quarantinedPieceKeys: new Set(['50001:0']),
    services: [service(2.5, 0), service(3.5, 0)],
  }
  // Another dataset's accepted evidence on exactly the same piece is not ours to retract.
  const national = service(17, 0)
  national.evidence.sourceId = 9864
  await writeClippedRailPassages({ ...request, sourceId: 9864,
    services: [national], quarantinedPieceKeys: new Set() })
  const foreign = listRailIntervals(prepared).filter(row => row.sourceId === 9864)
  const first = await writeClippedRailPassages(request)
  assert.equal(first.walkStamped, 1, 'an unrelated failed service cannot erase accepted passages')
  const owned = () => listRailIntervals(prepared).filter(row => row.sourceId === SOURCE)
  assert.deepEqual(owned().map(row => row.passenger), [2.5, 3.5])
  assert.ok(owned().every(row => row.passengerStatus === 2 && row.freightStatus === 0 && row.matching === 1))
  for (const retractSafe of [true, false]) {
    await writeClippedRailPassages({ ...request, retractSafe })
    assert.deepEqual(owned().map(row => row.passenger), [2.5, 3.5])
  }
  // Changed interval and fewer services must remove old occurrence/extent keys, not add to them.
  await writeClippedRailPassages({ ...request, services: [service(4.25, 10)] })
  const retained = owned()
  assert.equal(retained.length, 1)
  assert.equal(retained[0].passenger, 4.25)
  assert.equal(retained[0].fromM, 10)
  for (const fromM of [50, Number.NaN, Number.POSITIVE_INFINITY]) {
    const invalid = await writeClippedRailPassages({ ...request, services: [service(99, fromM)] })
    assert.equal(invalid.walkStamped, 0)
    assert.deepEqual(owned(), retained, 'invalid replacement cannot retract accepted evidence')
  }
  const empty = await writeClippedRailPassages({ ...request, services: [],
    extraMatch: () => { throw new Error('quarantine must still block fallback testimony') },
    squares: [otherSquare] })
  assert.equal(empty.retracted, 0)
  assert.deepEqual(owned(), retained, 'no accepted replacement preserves earlier evidence')
  assert.deepEqual(listRailIntervals(prepared).filter(row => row.sourceId === 9864), foreign)
  const quarantine = tableFromIPC(readFileSync(join(prepared, otherSquare, 'rail-quarantine.DE.arrow')))
  assert.deepEqual(Array.from(quarantine.getChild('source_id')!.toArray() as Uint16Array), [SOURCE])
  assert.deepEqual([readFileSync(path), readFileSync(otherPath)], before)
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
