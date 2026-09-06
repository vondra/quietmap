/** z9/z30 Arrow fixture shared by road-writer contract tests. */

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
const GRID_QUANTUM_M = 0.037_322_767_717_044_72
const GRID_ORIGIN = 2 ** 29

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
    .reduce(([south, west, north, east], { lat, lon }) => [
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
    ['qm_batch_bboxes', JSON.stringify(indices.length ? [bounds] : [])],
    ...(!options.omitCountryContract ? [['roads_contract', 'country_baked_v1'] as const] : []),
  ])
  const schema = new Schema(table.schema.fields, metadata)
  const stored = new Table(schema, table.batches.map(batch => new RecordBatch(schema, batch.data)))
  const path = join(ROAD_TEST_DIRECTORY, name)
  writeFileSync(path, Buffer.from(tableToIPC(stored, 'file')))
  return path
}

export const bytes = (path: string): Buffer => readFileSync(path)
