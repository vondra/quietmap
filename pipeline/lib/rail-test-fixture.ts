/** Faithful z9/z30 railways.arrow fixture shared by writer and loader tests. */

import { after } from 'node:test'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import {
  Bool, Float32, Int16, Int32, Int64, RecordBatch, Schema, Table, Uint8, Uint16, Utf8,
  tableToIPC, vectorFromArray,
} from 'apache-arrow'
import { iso2Code } from './prepared-grid.js'

const WEB_MERCATOR_RADIUS_M = 6_378_137
const GRID_QUANTUM_M = 0.037_322_767_717_044_72
const GRID_ORIGIN = 2 ** 29

export const RAIL_TEST_DIRECTORY = mkdtempSync(join(tmpdir(), 'railways-arrow-test-'))
after(() => rmSync(RAIL_TEST_DIRECTORY, { recursive: true, force: true }))

function lonLatToGrid(lon: number, lat: number): [number, number] {
  const x = WEB_MERCATOR_RADIUS_M * lon * Math.PI / 180
  const y = WEB_MERCATOR_RADIUS_M * Math.log(Math.tan(Math.PI / 4 + lat * Math.PI / 360))
  return [Math.floor(x / GRID_QUANTUM_M) + GRID_ORIGIN, Math.floor(y / GRID_QUANTUM_M) + GRID_ORIGIN]
}

export interface RailwayFixtureRow {
  osmId?: number | bigint
  latitude: number
  longitude: number
  endLatitude?: number
  endLongitude?: number
  lengthMetres?: number
  railType?: number
  usage?: number
  service?: number
  name?: string
  ref?: string
  sourceId?: number
  passenger?: number
  freight?: number
  divisor?: number
  country?: string
}

export interface RailwayFixtureOptions {
  omitContract?: boolean
  omitCountry?: boolean
  omitRailType?: boolean
  includeTraffic?: boolean
  includeDivisor?: boolean
}

export function writeRailwaysFixture(
  name: string,
  rows: readonly RailwayFixtureRow[],
  options: RailwayFixtureOptions = {},
): string {
  const starts = rows.map(row => lonLatToGrid(row.longitude, row.latitude))
  const ends = rows.map(row => lonLatToGrid(
    row.endLongitude ?? row.longitude + 0.0005,
    row.endLatitude ?? row.latitude + 0.0005,
  ))
  const indices = [...rows.keys()]
  const table = new Table({
    osm_id: vectorFromArray(rows.map((row, index) => BigInt(row.osmId ?? 50_000 + index)), new Int64()),
    segment_idx: vectorFromArray(indices, new Int16()),
    start_gx: vectorFromArray(starts.map(point => point[0]), new Int32()),
    start_gy: vectorFromArray(starts.map(point => point[1]), new Int32()),
    end_gx: vectorFromArray(ends.map(point => point[0]), new Int32()),
    end_gy: vectorFromArray(ends.map(point => point[1]), new Int32()),
    length_m: vectorFromArray(rows.map(row => row.lengthMetres ?? 66), new Float32()),
    ...(options.omitRailType ? {} : {
      rail_type: vectorFromArray(rows.map(row => row.railType ?? 0), new Uint8()),
    }),
    usage: vectorFromArray(rows.map(row => row.usage ?? 0), new Uint8()),
    maxspeed: vectorFromArray(rows.map(() => 120), new Uint16()),
    name: vectorFromArray(rows.map(row => row.name ?? ''), new Utf8()),
    ref: vectorFromArray(rows.map(row => row.ref ?? ''), new Utf8()),
    electrified: vectorFromArray(rows.map(() => 0), new Uint8()),
    gauge: vectorFromArray(rows.map(() => 1435), new Uint16()),
    bridge: vectorFromArray(rows.map(() => false), new Bool()),
    tunnel: vectorFromArray(rows.map(() => false), new Bool()),
    highspeed: vectorFromArray(rows.map(() => false), new Bool()),
    service: vectorFromArray(rows.map(row => row.service ?? 0), new Uint8()),
    source_id: vectorFromArray(rows.map(row => row.sourceId ?? 0), new Uint16()),
    ...(options.includeTraffic ? {
      trains_passenger: vectorFromArray(rows.map(row => row.passenger ?? 0), new Int32()),
      trains_freight: vectorFromArray(rows.map(row => row.freight ?? 0), new Int32()),
    } : {}),
    ...(options.includeDivisor ? {
      parallel_divisor: vectorFromArray(rows.map(row => row.divisor ?? 1), new Uint8()),
    } : {}),
    ...(options.omitCountry ? {} : {
      country_iso: vectorFromArray(rows.map(row => iso2Code(row.country ?? 'CD')), new Uint16()),
      city_id: vectorFromArray(rows.map(() => 0), new Uint16()),
      continent: vectorFromArray(rows.map(() => 2), new Uint8()),
    }),
  })
  const metadata = new Map<string, string>([
    ['grid', 'z30'],
    ['qm_batch_bboxes', '[[0,0,1,1]]'],
    ...(!options.omitContract ? [['railways_contract', 'country_baked_v1'] as const] : []),
  ])
  const schema = new Schema(table.schema.fields, metadata)
  const stored = new Table(schema, table.batches.map(batch => new RecordBatch(schema, batch.data)))
  const path = join(RAIL_TEST_DIRECTORY, name)
  writeFileSync(path, Buffer.from(tableToIPC(stored, 'file')))
  return path
}

export const railwayBytes = (path: string): Buffer => readFileSync(path)
