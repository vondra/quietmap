/** z9/z30 Arrow fixture shared by road-writer contract tests, and the test-side `qm_blocks` codec. */

import { after } from 'node:test'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import {
  Int32, Int64, RecordBatch, Schema, Table, Uint8, Uint16, Utf8,
  tableToIPC, vectorFromArray,
} from 'apache-arrow'
import { gridToLonLat, iso2Code } from './prepared-grid.js'

const WEB_MERCATOR_RADIUS_M = 6_378_137
const EARTH_CIRCUMFERENCE_M = 40_075_016.685_578_49
const MAX_MERCATOR_LAT_DEG = 85.051_128_78
const GRID_QUANTUM_M = 0.037_322_767_717_044_72
const GRID_ORIGIN = 2 ** 29
const Z14_AXIS = 1 << 14
const QM_BLOCKS_VERSION = 1
const QM_BLOCK_RECORD_LEN = 2 + 2 + 4 * 8

/** `[south, west, north, east]` degrees: envelope of one batch's complete geometries. */
export type QmEnvelope = readonly [number, number, number, number]
/** One `qm_blocks` record as engine/arrow-batching `Block` defines it. */
export interface QmBlock { cellX: number, cellY: number, bbox: QmEnvelope }

/** Global z14 cell of a point, the row-major block key of engine/arrow-batching. */
export function z14CellOf(lat: number, lon: number): [number, number] {
  const wrapped = lon >= -180 && lon < 180 ? lon : ((lon + 180) % 360 + 360) % 360 - 180
  const clamped = Math.max(-MAX_MERCATOR_LAT_DEG, Math.min(MAX_MERCATOR_LAT_DEG, lat))
  const northing = WEB_MERCATOR_RADIUS_M * Math.log(Math.tan(Math.PI / 4 + clamped * Math.PI / 360))
  const cell = (value: number) => Math.min(Z14_AXIS - 1, Math.max(0, Math.floor(value)))
  return [cell((wrapped + 180) / 360 * Z14_AXIS), cell((0.5 - northing / EARTH_CIRCUMFERENCE_M) * Z14_AXIS)]
}

/** The `qm_blocks` value for batches with these envelopes: version byte, then per
 *  batch little-endian `u16 x, u16 y, f64 south, west, north, east` (cell of the
 *  envelope midpoint), base64 — byte-identical to the Rust encoder. */
export function encodeQmBlocks(envelopes: readonly QmEnvelope[]): string {
  const bytes = Buffer.alloc(1 + QM_BLOCK_RECORD_LEN * envelopes.length)
  bytes[0] = QM_BLOCKS_VERSION
  envelopes.forEach((envelope, index) => {
    const at = 1 + QM_BLOCK_RECORD_LEN * index
    const [cellX, cellY] = z14CellOf((envelope[0] + envelope[2]) / 2, (envelope[1] + envelope[3]) / 2)
    bytes.writeUInt16LE(cellX, at)
    bytes.writeUInt16LE(cellY, at + 2)
    envelope.forEach((value, axis) => bytes.writeDoubleLE(value, at + 4 + 8 * axis))
  })
  return bytes.toString('base64')
}

export function decodeQmBlocks(value: string): QmBlock[] {
  const bytes = Buffer.from(value, 'base64')
  if (bytes[0] !== QM_BLOCKS_VERSION || (bytes.length - 1) % QM_BLOCK_RECORD_LEN !== 0) throw new Error('malformed qm_blocks')
  return Array.from({ length: (bytes.length - 1) / QM_BLOCK_RECORD_LEN }, (_, index) => {
    const at = 1 + QM_BLOCK_RECORD_LEN * index
    const bbox = [0, 1, 2, 3].map(axis => bytes.readDoubleLE(at + 4 + 8 * axis)) as [number, number, number, number]
    return { cellX: bytes.readUInt16LE(at), cellY: bytes.readUInt16LE(at + 2), bbox }
  })
}

export const ROAD_TEST_DIRECTORY = mkdtempSync(join(tmpdir(), 'roads-arrow-test-'))
after(() => rmSync(ROAD_TEST_DIRECTORY, { recursive: true, force: true }))

function lonLatToGrid(lon: number, lat: number): [number, number] {
  const x = WEB_MERCATOR_RADIUS_M * lon * Math.PI / 180
  const y = WEB_MERCATOR_RADIUS_M * Math.log(Math.tan(Math.PI / 4 + lat * Math.PI / 360))
  return [Math.floor(x / GRID_QUANTUM_M) + GRID_ORIGIN, Math.floor(y / GRID_QUANTUM_M) + GRID_ORIGIN]
}

export interface RoadFixtureOptions {
  origin?: readonly [longitude: number, latitude: number]
  speeds?: number[]
  countryCodes?: number[]
  refs?: Array<string | null>
  sourceIds?: number[]
  omitCountryColumn?: boolean
  omitCountryContract?: boolean
}

export function writeRoadsFixture(name: string, classes: number[], options: RoadFixtureOptions = {}): string {
  const indices = [...classes.keys()]
  const [longitude, latitude] = options.origin ?? [14, 50]
  const starts = indices.map(index => lonLatToGrid(longitude + index * 0.001, latitude + index * 0.001))
  const ends = indices.map(index => lonLatToGrid(longitude + 0.0005 + index * 0.001, latitude + 0.0005 + index * 0.001))
  const bounds = [...starts, ...ends].map(([gx, gy]) => gridToLonLat(gx, gy))
    .reduce<QmEnvelope>(([south, west, north, east], { lat, lon }) => [
      Math.min(south, lat), Math.min(west, lon), Math.max(north, lat), Math.max(east, lon),
    ], [90, 180, -90, -180])
  const table = new Table({
    osm_id: vectorFromArray(indices.map(index => BigInt(10_000 + index)), new Int64()),
    ref: vectorFromArray(options.refs ?? indices.map(index => `R${index}`), new Utf8()),
    name: vectorFromArray(indices.map(index => `Road ${index}`), new Utf8()),
    start_gx: vectorFromArray(starts.map(point => point[0]), new Int32()),
    start_gy: vectorFromArray(starts.map(point => point[1]), new Int32()),
    end_gx: vectorFromArray(ends.map(point => point[0]), new Int32()),
    end_gy: vectorFromArray(ends.map(point => point[1]), new Int32()),
    road_class: vectorFromArray(classes, new Uint8()),
    ...(options.speeds ? { speed_limit: vectorFromArray(options.speeds, new Uint8()) } : {}),
    aadt_light: vectorFromArray(indices.map(index => 1000 + index), new Int32()),
    aadt_medium: vectorFromArray(indices.map(index => 2000 + index), new Int32()),
    aadt_heavy: vectorFromArray(indices.map(index => 3000 + index), new Int32()),
    aadt_moto: vectorFromArray(indices.map(index => 40 + index), new Int32()),
    source_id: vectorFromArray(options.sourceIds ?? indices.map(() => 0), new Uint16()),
    ...(options.omitCountryColumn ? {} : {
      country_iso: vectorFromArray(options.countryCodes ?? indices.map(() => iso2Code('CZ')), new Uint16()),
    }),
  })
  const metadata = new Map<string, string>([
    ['grid', 'z30'],
    ...(indices.length ? [['qm_blocks', encodeQmBlocks([bounds])] as const] : []),
    ...(!options.omitCountryContract ? [['roads_contract', 'country_baked_v1'] as const] : []),
  ])
  const schema = new Schema(table.schema.fields, metadata)
  const stored = new Table(schema, table.batches.map(batch => new RecordBatch(schema, batch.data)))
  const path = join(ROAD_TEST_DIRECTORY, name)
  writeFileSync(path, Buffer.from(tableToIPC(stored, 'file')))
  return path
}

export const bytes = (path: string): Buffer => readFileSync(path)
