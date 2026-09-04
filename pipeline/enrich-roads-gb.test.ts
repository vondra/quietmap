/** GB DfT loader tests for quoted CSV, matching and baked ownership. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import {
  enrichGreatBritainRoads, matchDftPoint, parseDftCsv, type DftCountPoint,
} from './enrich-roads-gb.js'
import { iso2Code } from './lib/prepared-grid.js'
import { writeRoadsFixture } from './lib/road-test-fixture.js'
import type { RoadRow } from './lib/roads-arrow.js'

const TEST_DIRECTORY = mkdtempSync(join(tmpdir(), 'enrich-roads-gb-test-'))
after(() => rmSync(TEST_DIRECTORY, { recursive: true, force: true }))

const CSV_HEADER = [
  'count_point_id', 'year', 'road_name', 'road_category', 'start_junction_road_name',
  'latitude', 'longitude', 'cars_and_taxis', 'LGVs', 'buses_and_coaches',
  'all_HGVs', 'two_wheeled_motor_vehicles', 'all_motor_vehicles',
].join(',')

function csvRow(values: readonly (string | number)[]): string {
  return values.map(value => {
    const text = String(value)
    return text.includes(',') ? `"${text.replaceAll('"', '""')}"` : text
  }).join(',')
}

function road(overrides: Partial<RoadRow> = {}): RoadRow {
  return {
    startLat: 51.5, startLon: -0.1, endLat: 51.51, endLon: -0.09,
    midLat: 51.505, midLon: -0.095, ref: 'A1', name: null,
    osmId: 1, roadClass: 1, existingSourceId: 0, ...overrides,
  }
}

function point(overrides: Partial<DftCountPoint> = {}): DftCountPoint {
  return {
    ref: 'A1', latitude: 51.505, longitude: -0.095, roadCategory: 'PA',
    light: 1000, medium: 40, heavy: 100, moto: 10, total: 1150, year: 2024,
    ...overrides,
  }
}

test('DfT CSV parsing respects quoted commas, latest-point identity, age and class-split gates', () => {
  const csv = [
    CSV_HEADER,
    csvRow([1, 2023, 'A 1', 'PA', 'Old junction', 51.5, -0.1, 100, 10, 4, 20, 2, 136]),
    csvRow([1, 2024, 'A 1', 'PA', 'Pierhead, Hugh Town', 51.5, -0.1, 200, 20, 8, 40, 4, 272]),
    csvRow([2, 2014, 'A2', 'PA', 'Old road', 52, -1, 100, 10, 4, 20, 2, 136]),
    csvRow([3, 2024, 'A3', 'PA', 'No split', 52, -1, 0, 0, 0, 0, 0, 100]),
  ].join('\n')
  const parsed = parseDftCsv(csv)
  assert.equal(parsed.length, 1)
  assert.deepEqual(parsed[0], {
    ref: 'A1', latitude: 51.5, longitude: -0.1, roadCategory: 'PA',
    light: 220, medium: 8, heavy: 40, moto: 4, total: 272, year: 2024,
  })
})

test('DfT parser admits only the derived independent-rounding class excess', () => {
  const csv = [
    CSV_HEADER,
    csvRow([1, 2024, 'A1', 'PA', 'rounded', 51.5, -0.1, 91, 0, 3, 7, 1, 100]),
    csvRow([2, 2024, 'A2', 'PA', 'impossible', 51.5, -0.1, 92, 0, 3, 7, 1, 100]),
  ].join('\n')
  assert.deepEqual(parseDftCsv(csv).map(point => point.ref), ['A1'])
})

test('DfT matcher requires an exact normalized ref within fifteen kilometres', () => {
  const near = point()
  const far = point({ latitude: 54 })
  const index = new Map([['A1', [far, near]]])
  assert.equal(matchDftPoint(road({ ref: ' A 1 ' }), index), near)
  assert.equal(matchDftPoint(road({ ref: 'A2' }), index), null)
  assert.equal(matchDftPoint(road({ midLat: 50 }), index), null)
})

test('z9 GB pass writes domestic data and heals a matching foreign GB stamp', async () => {
  const prepared = join(TEST_DIRECTORY, 'prepared')
  const square = join(prepared, 'z9', '255', '170')
  mkdirSync(square, { recursive: true })
  const source = writeRoadsFixture('gb-loader.arrow', [1, 1], {
    refs: ['A1', 'A1'],
    countryCodes: [iso2Code('GB'), iso2Code('IE')],
    sourceIds: [0, 1041],
  })
  const target = join(square, 'roads.arrow')
  copyFileSync(source, target)
  const measured = point({ latitude: 50.00025, longitude: 14.00025 })

  const result = await enrichGreatBritainRoads(prepared, [measured])
  assert.deepEqual(
    { matched: result.matched, retracted: result.retracted, skippedForeign: result.skippedForeign },
    { matched: 1, retracted: 1, skippedForeign: 1 },
  )
  const table = tableFromIPC(readFileSync(target))
  assert.deepEqual([...Array(2)].map((_, index) => table.getChild('source_id')!.get(index)), [1041, 0])
  assert.equal(table.getChild('aadt_light')!.get(0), 1000)
  assert.equal(table.schema.metadata.get('roads_contract'), 'country_baked_v1')
})
