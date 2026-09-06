/** Behavioral contracts for the terminal z9 parallel-track divisor pass. */

import assert from 'node:assert/strict'
import { copyFileSync, mkdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { test } from 'node:test'
import { tableFromIPC } from 'apache-arrow'
import { segmentGeometryReader } from './lib/prepared-grid.js'
import { M_PER_DEG_LAT } from './lib/spatial.js'
import {
  computeParallelDivisors, enrichRailwayParallelDirectory, enrichRailwayParallelSquare,
  type ParallelRailRow,
} from './enrich-railways-parallel.js'
import {
  SOURCE_ID_CZ_SZCD_GTFS, SOURCE_ID_CZ_TIMETABLE_SILENT, SOURCE_ID_GLOBAL_GTFS_TRANSIT,
} from './lib/source-ids.generated.js'
import {
  RAIL_TEST_DIRECTORY, railwayBytes, writeRailwaysFixture, type RailwayFixtureRow,
} from './lib/rail-test-fixture.js'

const LON_STEP = 0.0002

function northSouth(longitude: number): Pick<RailwayFixtureRow, 'latitude' | 'longitude' | 'endLatitude' | 'endLongitude'> {
  return { latitude: 50, longitude, endLatitude: 50.0009, endLongitude: longitude }
}

function readDivisors(path: string): Array<number | null> {
  const table = tableFromIPC(railwayBytes(path))
  const divisors = table.getChild('parallel_divisor')
  return divisors
    ? [...Array(table.numRows)].map((_, index) => divisors.get(index) as number)
    : new Array(table.numRows).fill(null)
}

function row(
  osmId: number,
  corridorToken: string,
  startLat: number,
  startLon: number,
  endLat: number,
  endLon: number,
  overrides: Partial<ParallelRailRow> = {},
): ParallelRailRow {
  return {
    osmId: String(osmId),
    corridorToken,
    railType: 0,
    usage: 0,
    service: 0,
    sourceId: 0,
    startKey: `${startLat}_${startLon}`,
    endKey: `${endLat}_${endLon}`,
    startLat,
    startLon,
    endLat,
    endLon,
    midLat: (startLat + endLat) / 2,
    midLon: startLon + (endLon - startLon) / 2,
    ...overrides,
  }
}

test('corridor identity excludes service/tokenless rows and splits rail family', () => {
  const rows = [
    row(1, '2600', 50, 14, 50.001, 14),
    row(2, '2600', 50, 14 + LON_STEP, 50.001, 14 + LON_STEP),
    row(3, '2600', 50, 14 + 2 * LON_STEP, 50.001, 14 + 2 * LON_STEP, { railType: 1 }),
    row(6, '2600', 50, 14 + 3 * LON_STEP, 50.001, 14 + 3 * LON_STEP, { usage: 1 }),
    row(4, '2600', 50, 14 - LON_STEP, 50.001, 14 - LON_STEP, { service: 2 }),
    row(5, '', 50, 14 + 4 * LON_STEP, 50.001, 14 + 4 * LON_STEP),
  ]
  assert.deepEqual(computeParallelDivisors(rows), [2, 2, 1, 1, null, null])
})

test('name fallback groups distinct ways and the divisor clamps at three', async () => {
  const path = writeRailwaysFixture('parallel-clamp.arrow', [...Array(5)].map((_, index) => ({
    osmId: 100 + index,
    name: 'Stammstrecke',
    ...northSouth(14 + index * LON_STEP),
  })))
  const result = await enrichRailwayParallelSquare(path)
  assert.deepEqual(result.computedHist, [0, 0, 5])
  assert.deepEqual(result.writtenHist, [0, 0, 5])
  assert.deepEqual(readDivisors(path), [3, 3, 3, 3, 3])
})

test('same OSM way and ways sharing an endpoint are continuity, not parallel tracks', () => {
  const rows = [
    row(20, 'S', 50, 14, 50.0005, 14),
    row(20, 'S', 50, 14 + LON_STEP, 50.0005, 14 + LON_STEP),
    row(30, 'J', 50, 15, 50.0005, 15),
    row(31, 'J', 50.0005, 15, 50.001, 15),
  ]
  assert.deepEqual(computeParallelDivisors(rows), [1, 1, 1, 1])
})

test('the 50 metre radius holds for native Arrow segments, including both dateline directions', async () => {
  for (const [startLongitude, endLongitude] of [[14, 14.0008], [179.9996, -179.9996], [-179.9996, 179.9996]]) {
    for (const separationMetres of [49, 51]) {
      const path = writeRailwaysFixture(`parallel-radius-${startLongitude}-${separationMetres}.arrow`, [0, 1].map(index => ({
        osmId: 70 + index,
        ref: 'RADIUS',
        latitude: 50 + index * separationMetres / M_PER_DEG_LAT,
        endLatitude: 50 + index * separationMetres / M_PER_DEG_LAT,
        longitude: startLongitude,
        endLongitude,
      })))
      const before = railwayBytes(path)
      const geometry = segmentGeometryReader(tableFromIPC(before))
      if (Math.abs(endLongitude - startLongitude) > 180) {
        const segment = geometry.row(0)
        assert.ok(Math.abs(segment.endLon - segment.startLon) > 350)
        assert.ok(Math.abs(Math.abs(segment.midLon) - 180) < 1e-6)
      }
      const result = await enrichRailwayParallelSquare(path)
      assert.deepEqual(result.computedHist, separationMetres < 50 ? [0, 2, 0] : [2, 0, 0])
      assert.deepEqual(readDivisors(path), separationMetres < 50 ? [2, 2] : [null, null])
      if (separationMetres > 50) assert.deepEqual(railwayBytes(path), before)
    }
  }
})

test('only values of one are raised; stored divisors above one and dry-run bytes are preserved', async () => {
  const path = writeRailwaysFixture('parallel-only-raise.arrow', [
    { osmId: 40, ref: 'M', divisor: 3, ...northSouth(14) },
    { osmId: 41, ref: 'M', divisor: 1, ...northSouth(14 + LON_STEP) },
  ], { includeDivisor: true })
  const dryBytes = railwayBytes(path)
  const dry = await enrichRailwayParallelSquare(path, { dryRun: true })
  assert.deepEqual({ written: dry.writtenHist, changed: dry.changed }, { written: [0, 1, 0], changed: false })
  assert.deepEqual(railwayBytes(path), dryBytes)

  const applied = await enrichRailwayParallelSquare(path)
  assert.deepEqual(
    { kept: applied.keptExistingAbove1, written: applied.writtenHist, changed: applied.changed },
    { kept: 1, written: [0, 1, 0], changed: true },
  )
  assert.deepEqual(readDivisors(path), [3, 2])
  const exact = railwayBytes(path)
  const rerun = await enrichRailwayParallelSquare(path)
  assert.deepEqual({ kept: rerun.keptExistingAbove1, written: rerun.writtenHist, changed: rerun.changed }, {
    kept: 2, written: [0, 0, 0], changed: false,
  })
  assert.deepEqual(railwayBytes(path), exact)

  const protectedPath = writeRailwaysFixture('parallel-preserve-two.arrow', [0, 1, 2].map(index => ({
    osmId: 50 + index, ref: 'THREE', divisor: index === 0 ? 2 : 1,
    ...northSouth(14 + index * LON_STEP),
  })), { includeDivisor: true })
  const protectedResult = await enrichRailwayParallelSquare(protectedPath)
  assert.deepEqual(protectedResult.computedHist, [0, 0, 3])
  assert.deepEqual(protectedResult.writtenHist, [0, 0, 2])
  assert.equal(protectedResult.keptExistingAbove1, 1)
  assert.deepEqual(readDivisors(protectedPath), [2, 3, 3])
})

test('registry-owned graph-walk rows neither receive nor grant a divisor', async () => {
  const path = writeRailwaysFixture('parallel-walk-owned.arrow', [
    { osmId: 200, ref: 'W', sourceId: SOURCE_ID_CZ_SZCD_GTFS, divisor: 1, ...northSouth(14) },
    { osmId: 201, ref: 'W', sourceId: SOURCE_ID_CZ_SZCD_GTFS, divisor: 1, ...northSouth(14 + LON_STEP) },
    { osmId: 202, ref: 'S', sourceId: SOURCE_ID_CZ_TIMETABLE_SILENT, divisor: 1, ...northSouth(15) },
    { osmId: 203, ref: 'S', sourceId: SOURCE_ID_CZ_TIMETABLE_SILENT, divisor: 1, ...northSouth(15 + LON_STEP) },
    { osmId: 204, ref: 'U', sourceId: SOURCE_ID_GLOBAL_GTFS_TRANSIT, divisor: 1, ...northSouth(16) },
    { osmId: 205, ref: 'U', sourceId: SOURCE_ID_GLOBAL_GTFS_TRANSIT, divisor: 1, ...northSouth(16 + LON_STEP) },
  ], { includeDivisor: true })
  const result = await enrichRailwayParallelSquare(path)
  assert.equal(result.eligibleRows, 2)
  assert.deepEqual(readDivisors(path), [1, 1, 1, 1, 2, 2])
})

function installSquare(prepared: string, x: number, y: number, source: string): string {
  const directory = join(prepared, 'z9', String(x), String(y))
  mkdirSync(directory, { recursive: true })
  const path = join(directory, 'railways.arrow')
  copyFileSync(source, path)
  return path
}

test('adjacent z9 squares across E180 supply context while preserving counts and source', async () => {
  const prepared = join(RAIL_TEST_DIRECTORY, 'parallel-e180-prepared')
  const east = installSquare(prepared, 511, 256, writeRailwaysFixture('parallel-e180-east.arrow', [{
    osmId: 300,
    ref: 'E180',
    sourceId: SOURCE_ID_GLOBAL_GTFS_TRANSIT,
    passenger: 7,
    freight: 2,
    latitude: 0,
    longitude: 179.9999,
    endLatitude: 0.0009,
    endLongitude: 179.9999,
  }], { includeTraffic: true }))
  const west = installSquare(prepared, 0, 256, writeRailwaysFixture('parallel-e180-west.arrow', [{
    osmId: 301,
    ref: 'E180',
    sourceId: SOURCE_ID_GLOBAL_GTFS_TRANSIT,
    passenger: 9,
    freight: 3,
    latitude: 0,
    longitude: -179.9999,
    endLatitude: 0.0009,
    endLongitude: -179.9999,
  }], { includeTraffic: true }))

  const result = await enrichRailwayParallelDirectory(prepared, [-1, 179.9, 1, -179.9])
  assert.deepEqual(
    { squares: result.squares, updated: result.squaresUpdated, computed: result.computedHist, written: result.writtenHist },
    { squares: 2, updated: 2, computed: [0, 2, 0], written: [0, 2, 0] },
  )
  for (const [path, passenger, freight] of [[east, 7, 2], [west, 9, 3]] as const) {
    const table = tableFromIPC(readFileSync(path))
    assert.equal(table.getChild('parallel_divisor')!.get(0), 2)
    assert.equal(table.getChild('trains_passenger')!.get(0), passenger)
    assert.equal(table.getChild('trains_freight')!.get(0), freight)
    assert.equal(table.getChild('source_id')!.get(0), SOURCE_ID_GLOBAL_GTFS_TRANSIT)
    assert.equal(table.schema.metadata.get('railways_contract'), 'country_baked_v1')
  }
  const before = [railwayBytes(east), railwayBytes(west)]
  const rerun = await enrichRailwayParallelDirectory(prepared, [-1, 179.9, 1, -179.9])
  assert.equal(rerun.squaresUpdated, 0)
  assert.deepEqual([railwayBytes(east), railwayBytes(west)], before)
})

test('invalid prepared schema fails before replacing the source file', async () => {
  const path = writeRailwaysFixture('parallel-invalid.arrow', [{
    osmId: 400, ref: 'X', ...northSouth(14),
  }], { omitRailType: true })
  const before = railwayBytes(path)
  await assert.rejects(enrichRailwayParallelSquare(path), /rail_type/)
  assert.deepEqual(railwayBytes(path), before)
})


test('nearby distinct native endpoints remain separate parallel tracks', async () => {
  const path = writeRailwaysFixture('parallel-native-gap.arrow', [0, 1].map(index => ({
    osmId: 500 + index, ref: 'DISTINCT', ...northSouth(14 + index * 0.000001),
  })))
  const result = await enrichRailwayParallelSquare(path)
  assert.deepEqual(result.computedHist, [0, 2, 0])
  assert.deepEqual(readDivisors(path), [2, 2])
})
