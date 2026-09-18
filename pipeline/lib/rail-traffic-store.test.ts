/** Durability of one (square, country) interval session: a failed mutation and the two-rename crash window. */

import assert from 'node:assert/strict'
import { copyFileSync, readFileSync, readdirSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { test } from 'node:test'
import { withRailTrafficSquare, type RailSquareInterval } from './rail-traffic-store.js'
import { writeClippedRailPassages, type RailServicePassages, type WriteClippedRailPassagesRequest } from './rail-passage.js'
import { RAIL_TEST_DIRECTORY, writePreparedRailwaySquare } from './rail-test-fixture.js'
import { writeSyntheticRailTopology } from './transport-test-fixture.js'
import { SOURCE_ID_GLOBAL_GTFS_TRANSIT as SOURCE } from './source-ids.generated.js'

const SQUARE = 'z9/276/173'
const STORE_FILES = ['rail-intervals.DE.arrow', 'rail-quarantine.DE.arrow']

function preparedSquare(name: string): string {
  const prepared = join(RAIL_TEST_DIRECTORY, name)
  writePreparedRailwaySquare(prepared, SQUARE, `${name}.arrow`, [
    { osmId: 50_000, segmentIndex: 0, latitude: 50, longitude: 14, country: 'DE' },
    { osmId: 50_001, segmentIndex: 0, latitude: 50.01, longitude: 14, country: 'DE' },
  ])
  writeSyntheticRailTopology(prepared, [SQUARE])
  return prepared
}

test('a throwing mutation renames neither the quarantine nor the interval file', async () => {
  const prepared = preparedSquare('store-mutate-throws'), directory = join(prepared, SQUARE)
  const interval = (osmId: number): RailSquareInterval => ({ osmId, segmentIndex: 0, fromM: 0, toM: 10, occurrence: 0,
    sourceId: SOURCE, passenger: 1, freight: 0, passengerStatus: 2, freightStatus: 0, matching: 1 })
  await withRailTrafficSquare(prepared, SQUARE, 'DE', session => {
    session.replaceQuarantine(SOURCE, [{ osmId: 50_000, segmentIndex: 0 }])
    session.insert(interval(50_000))
  })
  const stored = () => STORE_FILES.map(name => [readFileSync(join(directory, name)), statSync(join(directory, name)).ino])
  const before = stored(), names = readdirSync(directory)
  await assert.rejects(withRailTrafficSquare(prepared, SQUARE, 'DE', session => {
    session.replaceQuarantine(SOURCE, [{ osmId: 50_001, segmentIndex: 0 }])
    session.insert(interval(50_001))
    throw new Error('mutation failed')
  }), /mutation failed/)
  assert.deepEqual(stored(), before)
  assert.deepEqual(readdirSync(directory), names)
})

test('a crash between the quarantine and the interval rename is repaired by the rerun', async () => {
  const service = (wayId: string, passenger: number, fromM: number): RailServicePassages => ({
    evidence: { sourceId: SOURCE, passenger, freight: 0, passengerStatus: 'known', freightStatus: 'unknown', matching: 'relation_estimated' },
    passages: [{ wayId, segmentIndex: 0, square: SQUARE, fromM, toM: 50, occurrence: 0 }],
  })
  const run = async (prepared: string, crashAfterQuarantineRename: boolean): Promise<Buffer[]> => {
    const directory = join(prepared, SQUARE), intervals = join(directory, STORE_FILES[0])
    const request: WriteClippedRailPassagesRequest = {
      preparedDirectory: prepared, squares: [SQUARE], countryIso: 'DE', sourceId: SOURCE, retractSafe: true,
      quarantinedPieceKeys: new Set(), services: [service('50000', 2.5, 0), service('50001', 3.5, 0)],
    }
    await writeClippedRailPassages(request)
    copyFileSync(intervals, `${intervals}.before`)
    const changed = { ...request, quarantinedPieceKeys: new Set(['50001:0']), services: [service('50000', 4.25, 10)] }
    await writeClippedRailPassages(changed)
    if (crashAfterQuarantineRename) {
      copyFileSync(`${intervals}.before`, intervals)
      await writeClippedRailPassages(changed)
    }
    return STORE_FILES.map(name => readFileSync(join(directory, name)))
  }
  const uninterrupted = await run(preparedSquare('store-uninterrupted'), false)
  assert.deepEqual(await run(preparedSquare('store-crash-window'), true), uninterrupted)
})
