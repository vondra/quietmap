/** Swiss SASVZ parser, join and z9 ownership tests. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import { enrichSwissRoads } from './enrich-roads-ch.js'
import { iso2Code } from './lib/prepared-grid.js'
import {
  joinSwissSasvzSource,
  loadSwissSasvzSource,
  parseSwissSasvzResults,
  parseSwissSasvzStations,
} from './lib/roads-ch-source.js'
import { writeRoadsFixture } from './lib/road-test-fixture.js'
import { SOURCE_ID_CH_NATIONAL_ROADS } from './lib/source-ids.generated.js'

const DIRECTORY = mkdtempSync(join(tmpdir(), 'enrich-roads-ch-test-'))
after(() => rmSync(DIRECTORY, { recursive: true, force: true }))

const emptyRow = (): unknown[] => []
const headerRows = (): unknown[][] => Array.from({ length: 7 }, emptyRow)

const stationBlock = (number: string, road: string, annual: readonly (number | null)[]): unknown[][] => {
  const labels = ['DTV', 'DTV SV', 'DTV SGF', 'DWV', 'DWV SV', 'DWV SGF']
  return labels.map((label, index) => {
    const row: unknown[] = Array.from({ length: 19 }, () => null)
    if (index === 0) {
      row[0] = number
      row[1] = `Station ${number}`
      row[3] = 'VD'
      row[4] = road
    }
    row[5] = label
    row[18] = annual[index]
    return row
  })
}

const stationList = (entries: readonly (readonly [string, number, number])[]): unknown[][] => [
  ...Array.from({ length: 11 }, emptyRow),
  ...entries.map(([number, east, north]) => {
    const row: unknown[] = Array.from({ length: 8 }, () => null)
    row[0] = number
    row[1] = `Station ${number}`
    row[5] = 'H1'
    row[6] = east
    row[7] = north
    return row
  }),
]

test('Swiss parser reads station blocks and converts LV95 to WGS 84', () => {
  const results = parseSwissSasvzResults([
    ...headerRows(),
    ...stationBlock('002', 'H 1', [16143.28, 484.39, 373.36, 17882.15, 638.4, 515.52]),
    ...stationBlock('005', 'H 17', [null, null, null, null, null, null]),
  ])
  assert.deepEqual({ stations: results.stations.size, unusable: results.unusableSkipped }, { stations: 1, unusable: 1 })
  // Chalet-à-Gobet LV95: E 2545269, N 1158463 is 6.725 E, 46.575 N.
  const stations = parseSwissSasvzStations(stationList([['2', 2545269, 1158463]]))
  const joined = joinSwissSasvzSource(results, stations)
  assert.equal(joined.observations.length, 1)
  const [observation] = joined.observations
  assert.ok(Math.abs(observation.latitude - 46.575) < 0.01)
  assert.ok(Math.abs(observation.longitude - 6.725) < 0.01)
  assert.deepEqual(
    {
      light: observation.light,
      medium: observation.medium,
      heavy: observation.heavy,
      moto: observation.moto,
      estimated: observation.estimatedClasses,
      rank: observation.rank,
    },
    { light: 15498, medium: 111, heavy: 373, moto: 161, estimated: 9, rank: 1 },
  )
})

test('Swiss join keeps totals without classes and skips the unlocatable', () => {
  const results = parseSwissSasvzResults([
    ...headerRows(),
    ...stationBlock('002', 'A 1', [16143.28, null, null, 17882.15, null, null]),
    ...stationBlock('003', 'H 13', [8837.39, 74.87, 38.19, 9356.12, 85.77, 49.32]),
    ...stationBlock('004', '', [5000.5, 100.2, 80.1, 5100.0, 110.0, 90.0]),
  ])
  const stations = parseSwissSasvzStations(
    stationList([
      ['2', 2600000, 1200000],
      ['4', 2600000, 1200000],
    ]),
  )
  const joined = joinSwissSasvzSource(results, stations)
  assert.deepEqual(
    {
      unlocated: joined.unlocatedSkipped,
      unranked: joined.unrankedSkipped,
      observations: joined.observations.length,
    },
    { unlocated: 1, unranked: 1, observations: 1 },
  )
  assert.deepEqual(
    {
      light: joined.observations[0].light,
      estimated: joined.observations[0].estimatedClasses,
    },
    { light: 16143, estimated: 15 },
  )
  assert.throws(
    () =>
      parseSwissSasvzResults([
        ...headerRows(),
        ...stationBlock('002', 'A 1', [16143.28, 484.39, 373.36, 17882.15, 638.4, 515.52]).slice(0, 5),
      ]),
    /six-row block layout/,
  )
})

test('Swiss pinned loader rejects bytes outside the admitted release source', async () => {
  const enrichmentDirectory = join(DIRECTORY, 'source')
  mkdirSync(join(enrichmentDirectory, 'ch'), { recursive: true })
  writeFileSync(join(enrichmentDirectory, 'ch', 'sasvz-2024-jahresergebnisse.xlsx'), 'not a workbook')
  writeFileSync(join(enrichmentDirectory, 'ch', 'sasvz-messstellen-2026-04.xlsx'), 'not a workbook')
  await assert.rejects(
    () =>
      loadSwissSasvzSource({
        preparedDirectory: join(DIRECTORY, 'unused'),
        enrichmentDirectory,
        enrichOnly: true,
        forceDownload: false,
      }),
    /does not match the admitted release source/,
  )
})

test('z9 Swiss pass writes classes, retracts stale claims and enforces baked country', async () => {
  const prepared = join(DIRECTORY, 'prepared')
  const square = join(prepared, 'z9', '266', '178')
  mkdirSync(square, { recursive: true })
  // LV95 E 2600000, N 1200000 is Bern 7.43863 E, 46.95108 N after the datum shift.
  const fixture = writeRoadsFixture('ch-loader.arrow', [0, 0, 0], {
    origin: [7.43863, 46.95108],
    refs: ['A1', 'A1', 'A1'],
    countryCodes: [iso2Code('CH'), iso2Code('DE'), iso2Code('CH')],
    sourceIds: [0, SOURCE_ID_CH_NATIONAL_ROADS, SOURCE_ID_CH_NATIONAL_ROADS],
  })
  const target = join(square, 'roads.arrow')
  copyFileSync(fixture, target)
  const joined = joinSwissSasvzSource(
    parseSwissSasvzResults([
      ...headerRows(),
      ...stationBlock('002', 'A 1', [16143.28, 484.39, 373.36, 17882.15, 638.4, 515.52]),
    ]),
    parseSwissSasvzStations(stationList([['2', 2600000, 1200000]])),
  )
  const result = await enrichSwissRoads(prepared, joined.observations)
  assert.deepEqual(
    {
      matched: result.matched,
      retracted: result.retracted,
      skippedForeign: result.skippedForeign,
    },
    { matched: 1, retracted: 2, skippedForeign: 1 },
  )
  const table = tableFromIPC(readFileSync(target))
  assert.deepEqual([...table.getChild('source_id')!], [SOURCE_ID_CH_NATIONAL_ROADS, 0, 0])
  assert.deepEqual(
    ['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto'].map(name => table.getChild(name)!.get(0)),
    [15498, 111, 373, 161],
  )
})
