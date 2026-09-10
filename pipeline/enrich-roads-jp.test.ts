/** Japanese MLIT census parser, identity matching and z9 ownership tests. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import { buildJapaneseRoadMatcher, enrichJapaneseRoads } from './enrich-roads-jp.js'
import { iso2Code } from './lib/prepared-grid.js'
import { normalizeJapaneseRoadIdentity, parseJapaneseRoadCensus, type JapaneseRoadCensus } from './lib/roads-jp-source.js'
import { writeRoadsFixture } from './lib/road-test-fixture.js'
import { SOURCE_ID_JP_CLASS_MEDIAN_FALLBACK, SOURCE_ID_JP_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import type { RoadRow } from './lib/roads-arrow.js'

const DIRECTORY = mkdtempSync(join(tmpdir(), 'enrich-roads-jp-test-'))
after(() => rmSync(DIRECTORY, { recursive: true, force: true }))

const csvRow = (roadType: string, route: string, name: string, small: string, large: string) => {
  const fields = Array<string>(61).fill('')
  fields[3] = roadType
  fields[4] = route
  fields[5] = name
  fields[59] = small
  fields[60] = large
  return fields.join(',')
}
const census = (): JapaneseRoadCensus => ({
  nationalByRef: new Map([['1', { small: 1000, large: 200 }]]),
  expresswayByName: new Map([['TestExpressway', { small: 2000, large: 400 }]]),
  expresswayNames: ['TestExpressway'],
  classMedian: new Map([
    [0, { small: 1800, large: 360 }], [1, { small: 900, large: 180 }],
    [2, { small: 800, large: 160 }], [3, { small: 500, large: 100 }],
    [4, { small: 300, large: 60 }], [10, { small: 1800, large: 360 }],
    [11, { small: 900, large: 180 }], [12, { small: 800, large: 160 }],
  ]),
  sourceRows: 47, admittedSections: 47, unsupportedTypeRows: 0,
  unavailableTrafficRows: 0, invalidRows: 0,
})
const road = (overrides: Partial<RoadRow> = {}): RoadRow => ({
  startLat: 35.7, startLon: 139.7, endLat: 35.701, endLon: 139.701,
  midLat: 35.7, midLon: 139.7, ref: '1', name: null,
  osmId: 1, roadClass: 1, existingSourceId: 0, ...overrides,
})

test('Japanese parser hard-gates 47 prefectures and computes route and class medians', () => {
  const header = Array<string>(61).fill('header').join(',')
  const measured = csvRow('3', '1', 'route', '1000', '200')
  const files = Array.from({ length: 47 }, () => new TextEncoder().encode(`${header}\n${measured}\n`))
  files[0] = new TextEncoder().encode(`${header}\n${measured}\n` +
    `${csvRow('9', '', '', '10', '2')}\n${csvRow('3', '2', '', '0', '0')}\n`)
  const parsed = parseJapaneseRoadCensus(files)
  assert.deepEqual({ rows: parsed.sourceRows, admitted: parsed.admittedSections,
    unsupported: parsed.unsupportedTypeRows, unavailable: parsed.unavailableTrafficRows,
    invalid: parsed.invalidRows },
  { rows: 49, admitted: 47, unsupported: 1, unavailable: 1, invalid: 0 })
  assert.equal(normalizeJapaneseRoadIdentity('国道 １２号'), '国道12号')
  assert.deepEqual(parsed.nationalByRef.get('1'), { small: 1000, large: 200 })
  assert.deepEqual(parsed.classMedian.get(1), { small: 1000, large: 200 })
  assert.deepEqual(parsed.classMedian.get(11), { small: 1000, large: 200 })
  assert.throws(() => parseJapaneseRoadCensus(files.slice(1)), /requires 47 prefectures/)
})

test('Japanese matcher distinguishes measured identity from census-derived fallback', () => {
  const match = buildJapaneseRoadMatcher(census())
  assert.deepEqual(match(road()), { light: 1000, medium: 50, heavy: 150, moto: 0,
    sourceId: SOURCE_ID_JP_NATIONAL_ROADS })
  assert.deepEqual(match(road({ roadClass: 3, ref: null })),
    { light: 500, medium: 25, heavy: 75, moto: 0,
      sourceId: SOURCE_ID_JP_CLASS_MEDIAN_FALLBACK })
  assert.deepEqual(match(road({ roadClass: 10, ref: null, name: 'TestExpressway ramp' })),
    { light: 1000, medium: 50, heavy: 150, moto: 0,
      sourceId: SOURCE_ID_JP_NATIONAL_ROADS })
  assert.equal(match(road({ roadClass: 7 })), null)
})

test('z9 Japanese pass writes both tiers and enforces baked country', async () => {
  const prepared = join(DIRECTORY, 'prepared')
  const square = join(prepared, 'z9', '454', '201')
  mkdirSync(square, { recursive: true })
  const fixture = writeRoadsFixture('jp-loader.arrow', [1, 3, 3, 7], {
    origin: [139.7, 35.7], refs: ['1', null, null, null],
    countryCodes: [iso2Code('JP'), iso2Code('JP'), iso2Code('KR'), iso2Code('JP')],
    sourceIds: [0, 0, SOURCE_ID_JP_CLASS_MEDIAN_FALLBACK, SOURCE_ID_JP_NATIONAL_ROADS],
  })
  const target = join(square, 'roads.arrow')
  copyFileSync(fixture, target)
  const result = await enrichJapaneseRoads(prepared, census())
  assert.deepEqual({ matched: result.matched, retracted: result.retracted,
    skipped: result.skipped, skippedForeign: result.skippedForeign },
  { matched: 2, retracted: 2, skipped: 1, skippedForeign: 1 })
  const table = tableFromIPC(readFileSync(target))
  assert.deepEqual([...table.getChild('source_id')!],
    [SOURCE_ID_JP_NATIONAL_ROADS, SOURCE_ID_JP_CLASS_MEDIAN_FALLBACK, 0, 0])
})
