/** Contract tests for canonical z9 paths, z30 geometry and baked ownership. */

import { after, test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import {
  Float64, Int32, RecordBatch, Schema, Table, Uint16, makeTable, vectorFromArray,
} from 'apache-arrow'
import {
  bakedRailwayCountryReader, bakedRoadCountryReader, gridToLonLat, iso2Code, listPreparedSquares,
  segmentGeometryReader,
} from './prepared-grid.js'

const TMP = mkdtempSync(join(tmpdir(), 'prepared-grid-test-'))
after(() => rmSync(TMP, { recursive: true, force: true }))

function withMetadata(table: Table, metadata: Record<string, string>): Table {
  const schema = new Schema(table.schema.fields, new Map(Object.entries(metadata)))
  return new Table(schema, table.batches.map(batch => new RecordBatch(schema, batch.data)))
}

function segmentTable(options: { grid?: string; floatCoordinates?: boolean; nullable?: boolean; country?: boolean } = {}): Table {
  const coordinateType = options.floatCoordinates ? new Float64() : new Int32()
  const value = options.nullable ? null : 579_373_192
  const table = new Table({
    start_gx: vectorFromArray([value], coordinateType),
    start_gy: vectorFromArray([709_587_895], coordinateType),
    end_gx: vectorFromArray([579_376_174], coordinateType),
    end_gy: vectorFromArray([709_592_537], coordinateType),
    ...(options.country === false ? {} : { country_iso: vectorFromArray([iso2Code('CZ')], new Uint16()) }),
  })
  return withMetadata(table, {
    ...(options.grid === '' ? {} : { grid: options.grid ?? 'z30' }),
    roads_contract: 'country_baked_v1',
  })
}

test('grid decoder matches the independently computed Python Prague golden', () => {
  const point = gridToLonLat(579_373_192, 709_587_895)
  assert.ok(Math.abs(point.lon - 14.249_999_821_061_41) < 1e-12)
  assert.ok(Math.abs(point.lat - 49.999_999_978_523_235) < 1e-12)
})

test('segment midpoint follows the short arc across the antimeridian', () => {
  const raw = new Table({
    start_gx: vectorFromArray([1_073_443_562], new Int32()),
    start_gy: vectorFromArray([536_870_911], new Int32()),
    end_gx: vectorFromArray([298_261], new Int32()),
    end_gy: vectorFromArray([536_870_911], new Int32()),
  })
  const geometry = segmentGeometryReader(withMetadata(raw, { grid: 'z30' })).row(0)
  assert.ok(Math.abs(Math.abs(geometry.midLon) - 180) < 1e-6, `midpoint ${geometry.midLon}`)
  assert.ok(Math.abs(geometry.endLon - geometry.startLon) > 350, 'fixture crosses the stored longitude seam')
})

test('geometry rejects absent contract, wrong coordinate type and null coordinates', () => {
  assert.throws(() => segmentGeometryReader(segmentTable({ grid: '' })), /grid contract/)
  assert.throws(() => segmentGeometryReader(segmentTable({ floatCoordinates: true })), /non-null Int32/)
  assert.throws(() => segmentGeometryReader(segmentTable({ nullable: true })), /non-null Int32/)
})

test('baked road ownership is little-endian ISO2 and fail-closed', () => {
  assert.equal(iso2Code('CZ'), 23_107)
  assert.equal(iso2Code('RU'), 21_842)
  const table = segmentTable()
  assert.equal(bakedRoadCountryReader(table).codeAt(0), iso2Code('CZ'))
  const noContract = withMetadata(table, { grid: 'z30' })
  assert.throws(() => bakedRoadCountryReader(noContract), /country_baked_v1/)
  assert.throws(() => bakedRoadCountryReader(segmentTable({ country: false })), /country_iso/)
})

test('railway ownership uses its own admin-bake contract key', () => {
  const table = withMetadata(segmentTable(), {
    grid: 'z30',
    railways_contract: 'country_baked_v1',
  })
  assert.equal(bakedRailwayCountryReader(table).codeAt(0), iso2Code('CZ'))
  assert.throws(() => bakedRailwayCountryReader(segmentTable()), /railways Arrow contract/)
})

function addSquare(x: string, y: string): void {
  const directory = join(TMP, 'z9', x, y)
  mkdirSync(directory, { recursive: true })
  writeFileSync(join(directory, 'roads.arrow'), '')
}

test('square listing is canonical, numeric and intersects ordinary and seam bboxes', () => {
  addSquare('276', '173')
  addSquare('0', '256')
  addSquare('511', '256')
  addSquare('0276', '173')
  addSquare('600', '100')
  assert.deepEqual(listPreparedSquares(TMP, [-90, -180, 90, 180]), ['z9/0/256', 'z9/276/173', 'z9/511/256'])
  assert.deepEqual(listPreparedSquares(TMP, [49.9, 14.1, 50.1, 14.4]), ['z9/276/173'])
  assert.deepEqual(listPreparedSquares(TMP, [-1, 179.8, 1, -179.8]), ['z9/0/256', 'z9/511/256'])
})

test('square listing rejects malformed bboxes and absent trees are empty', () => {
  assert.throws(() => listPreparedSquares(TMP, [20, 0, 10, 1]), /invalid prepared bbox/)
  assert.deepEqual(listPreparedSquares(join(TMP, 'absent'), [-90, -180, 90, 180]), [])
})
