/** Polish GPR parser, matcher and z9 ownership tests. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import { enrichPolishRoads, indexPolishGpr, matchPolishGpr } from './enrich-roads-pl.js'
import { iso2Code } from './lib/prepared-grid.js'
import { loadPolishGprSource, parsePolishGprSource, type PolishGprSegment } from './lib/roads-pl-source.js'
import { writeRoadsFixture } from './lib/road-test-fixture.js'
import { SOURCE_ID_PL_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import type { RoadRow } from './lib/roads-arrow.js'

const DIRECTORY = mkdtempSync(join(tmpdir(), 'enrich-roads-pl-test-'))
after(() => rmSync(DIRECTORY, { recursive: true, force: true }))

const sourceRow = (overrides: Record<string, unknown> = {}) => ({
  nr2020: '10201', ref: 'DK85', isProvincial: false,
  midLat: 52.2, midLon: 21,
  coords: [[20.99, 52.2], [21.01, 52.2]], imdTot: 1000,
  aadt_light: 800, aadt_medium: 50, aadt_heavy: 130, aadt_moto: 20,
  ...overrides,
})

const segment = (overrides: Partial<PolishGprSegment> = {}): PolishGprSegment => ({
  sourceId: '10201', ref: 'DK85', isProvincial: false,
  coordinates: [[20.99, 52.2], [21.01, 52.2]],
  light: 800, medium: 50, heavy: 130, moto: 20,
  ...overrides,
})

const road = (overrides: Partial<RoadRow> = {}): RoadRow => ({
  startLat: 52.2, startLon: 21, endLat: 52.201, endLon: 21.001,
  midLat: 52.2, midLon: 21, ref: 'DK 85', name: null,
  osmId: 1, roadClass: 2, existingSourceId: 0, ...overrides,
})

test('Polish parser preserves classes and accounts for zero traffic', () => {
  const parsed = parsePolishGprSource(JSON.stringify([
    sourceRow(),
    sourceRow({ nr2020: '1', ref: 'DW943', isProvincial: true,
      midLat: null, midLon: null, coords: null }),
    sourceRow({ nr2020: 'zero', aadt_light: 0, aadt_medium: 0, aadt_heavy: 0, aadt_moto: 0 }),
  ]))
  assert.deepEqual(
    { sourceRows: parsed.sourceRows, accepted: parsed.segments.length, zero: parsed.zeroTrafficSkipped },
    { sourceRows: 3, accepted: 2, zero: 1 },
  )
  assert.deepEqual(parsed.segments[0], segment())
  assert.equal(parsed.segments[1].coordinates, null)
  assert.throws(() => parsePolishGprSource(JSON.stringify([sourceRow({ coords: [[21, 52.2]] })])),
    /invalid line geometry/)
})

test('Polish pinned loader rejects bytes outside the admitted release source', () => {
  const enrichmentDirectory = join(DIRECTORY, 'source')
  mkdirSync(join(enrichmentDirectory, 'pl'), { recursive: true })
  writeFileSync(join(enrichmentDirectory, 'pl', 'gpr-2020-parsed-v3.json'), JSON.stringify([sourceRow()]))
  assert.throws(() => loadPolishGprSource({
    preparedDirectory: join(DIRECTORY, 'unused'), enrichmentDirectory,
    enrichOnly: true, forceDownload: false,
  }), /does not match the admitted release source/)
})

test('Polish matcher prefers a nearby national line and retains provincial ref matching', () => {
  const provincial = segment({ sourceId: 'p', ref: 'DW85', isProvincial: true, coordinates: null })
  const national = segment({ ref: 'DK85;85B;85' })
  const index = indexPolishGpr([provincial, national])
  assert.equal(matchPolishGpr(road(), index), national)
  assert.equal(matchPolishGpr(road({ ref: '85' }), index), national)
  assert.equal(matchPolishGpr(road({ ref: 'other;85B' }), index), national)
  assert.equal(matchPolishGpr(road({ ref: 'DW 85', midLat: 40 }), index), provincial)
  assert.equal(matchPolishGpr(road({ ref: 'DK85', midLat: 51.9 }), index), null)
})

test('z9 Polish pass writes classes, retracts stale claims and enforces baked country', async () => {
  const prepared = join(DIRECTORY, 'prepared')
  const square = join(prepared, 'z9', '285', '168')
  mkdirSync(square, { recursive: true })
  const fixture = writeRoadsFixture('pl-loader.arrow', [2, 2, 2], {
    origin: [21, 52.2], refs: ['DK 85', 'DK 85', 'DK 86'],
    countryCodes: [iso2Code('PL'), iso2Code('CZ'), iso2Code('PL')],
    sourceIds: [0, SOURCE_ID_PL_NATIONAL_ROADS, SOURCE_ID_PL_NATIONAL_ROADS],
  })
  const target = join(square, 'roads.arrow')
  copyFileSync(fixture, target)
  const result = await enrichPolishRoads(prepared, [segment()])
  assert.deepEqual(
    { matched: result.matched, retracted: result.retracted, skippedForeign: result.skippedForeign },
    { matched: 1, retracted: 2, skippedForeign: 1 },
  )
  const table = tableFromIPC(readFileSync(target))
  assert.deepEqual([...table.getChild('source_id')!], [SOURCE_ID_PL_NATIONAL_ROADS, 0, 0])
  assert.deepEqual(['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto']
    .map(name => table.getChild(name)!.get(0)), [800, 50, 130, 20])
})
