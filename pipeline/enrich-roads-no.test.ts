/** Norwegian NVDB parser, class matching and z9 ownership tests. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import { enrichNorwegianRoads } from './enrich-roads-no.js'
import { iso2Code } from './lib/prepared-grid.js'
import {
  norwegianVegrefRank, parseNorwegianNvdbSource, splitNorwegianAadt,
  type NorwegianNvdbSegment,
} from './lib/roads-no-source.js'
import { writeRoadsFixture } from './lib/road-test-fixture.js'
import { SOURCE_ID_NO_NATIONAL_ROADS } from './lib/source-ids.generated.js'

const DIRECTORY = mkdtempSync(join(tmpdir(), 'enrich-roads-no-test-'))
after(() => rmSync(DIRECTORY, { recursive: true, force: true }))

const sourceRow = (overrides: Record<string, unknown> = {}) => ({
  id: 1, vegref: 'EV6 S1D1 m0-100', midLat: 59.9, midLon: 10.75,
  aadt: 1000, heavyPct: 20, year: 2025,
  aadt_light: 790, aadt_medium: 50, aadt_heavy: 150, aadt_moto: 10,
  ...overrides,
})

const segment = (overrides: Partial<NorwegianNvdbSegment> = {}): NorwegianNvdbSegment => ({
  sourceId: 1, vegref: 'EV6 S1D1 m0-100', rank: 1,
  latitude: 59.9, longitude: 10.75, year: 2025,
  light: 790, medium: 50, heavy: 150, moto: 10,
  ...overrides,
})

test('Norwegian parser filters road category and geometry then derives an exact class total', () => {
  const parsed = parseNorwegianNvdbSource(JSON.stringify([
    sourceRow(),
    sourceRow({ id: 2, vegref: 'KV1 S1D1 m0-100' }),
    sourceRow({ id: 3, vegref: 'FV1 S1D1 m0-100', midLat: null }),
    sourceRow({ id: 4, vegref: 'RV4 S1D1 m0-100', aadt: 100, heavyPct: 100 }),
  ]))
  assert.deepEqual({ rows: parsed.sourceRows, accepted: parsed.segments.length,
    category: parsed.unsupportedRoadCategorySkipped, geometry: parsed.invalidGeometrySkipped },
  { rows: 4, accepted: 2, category: 1, geometry: 1 })
  assert.deepEqual(parsed.segments[0], segment())
  const extreme = parsed.segments[1]
  assert.equal(extreme.light + extreme.medium + extreme.heavy + extreme.moto, 100)
  assert.deepEqual(splitNorwegianAadt(100, 100), { light: 0, medium: 25, heavy: 74, moto: 1 })
  assert.equal(norwegianVegrefRank('FV7'), 2)
  assert.equal(norwegianVegrefRank('PV7'), null)
})

test('Norwegian parser fails loud on invalid traffic instead of partially admitting it', () => {
  assert.throws(() => parseNorwegianNvdbSource(JSON.stringify([
    sourceRow(), sourceRow({ id: 2, heavyPct: 101 }),
  ])), /invalid metadata or traffic values/)
})

test('z9 Norwegian pass requires class-compatible proximity and baked country', async () => {
  const prepared = join(DIRECTORY, 'prepared')
  const square = join(prepared, 'z9', '271', '148')
  mkdirSync(square, { recursive: true })
  const fixture = writeRoadsFixture('no-loader.arrow', [1, 1, 4, 7], {
    origin: [10.75, 59.9],
    countryCodes: [iso2Code('NO'), iso2Code('SE'), iso2Code('NO'), iso2Code('NO')],
    sourceIds: [0, SOURCE_ID_NO_NATIONAL_ROADS, SOURCE_ID_NO_NATIONAL_ROADS, SOURCE_ID_NO_NATIONAL_ROADS],
  })
  const target = join(square, 'roads.arrow')
  copyFileSync(fixture, target)
  const result = await enrichNorwegianRoads(prepared, [segment()])
  assert.deepEqual({ matched: result.matched, retracted: result.retracted,
    skipped: result.skipped, skippedForeign: result.skippedForeign },
  { matched: 1, retracted: 3, skipped: 1, skippedForeign: 1 })
  const table = tableFromIPC(readFileSync(target))
  assert.deepEqual([...table.getChild('source_id')!], [SOURCE_ID_NO_NATIONAL_ROADS, 0, 0, 0])
  assert.deepEqual(['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto']
    .map(name => table.getChild(name)!.get(0)), [790, 50, 150, 10])
})
