/** Italian Anas TGM parser, matching and z9 ownership tests. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import { enrichItalianRoads, indexItalianTgm, matchItalianTgm, splitItalianTgm } from './enrich-roads-it.js'
import { iso2Code } from './lib/prepared-grid.js'
import { normalizeAnasRef, normalizeItalianOsmRef, parseItalianTgmSource, type ItalianTgmStation } from './lib/roads-it-source.js'
import { writeRoadsFixture } from './lib/road-test-fixture.js'
import { SOURCE_ID_IT_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import type { RoadRow } from './lib/roads-arrow.js'

const DIRECTORY = mkdtempSync(join(tmpdir(), 'enrich-roads-it-test-'))
after(() => rmSync(DIRECTORY, { recursive: true, force: true }))

const feature = (overrides: Record<string, unknown> = {}) => ({
  type: 'Feature', properties: { Strada: 'A01', TGM: 1000 },
  geometry: { type: 'Point', coordinates: [12, 42, 10] }, ...overrides,
})
const station = (overrides: Partial<ItalianTgmStation> = {}): ItalianTgmStation => ({
  sourceRow: 0, ref: 'A1', latitude: 42, longitude: 12, total: 1000, ...overrides,
})
const road = (overrides: Partial<RoadRow> = {}): RoadRow => ({
  startLat: 42, startLon: 12, endLat: 42.001, endLon: 12.001,
  midLat: 42, midLon: 12, ref: 'A1', name: null,
  osmId: 1, roadClass: 0, existingSourceId: 0, ...overrides,
})

test('Italian parser normalizes refs, preserves total and accounts for malformed features', () => {
  const parsed = parseItalianTgmSource(JSON.stringify({ type: 'FeatureCollection', features: [
    feature(), feature({ geometry: null }), feature({ properties: { Strada: 'A2', TGM: 0 } }),
    feature({ properties: { Strada: 'E45', TGM: 100 } }),
  ] }))
  assert.deepEqual({ rows: parsed.sourceRows, accepted: parsed.stations.length,
    geometry: parsed.invalidGeometrySkipped, traffic: parsed.invalidTrafficSkipped,
    metadata: parsed.invalidMetadataSkipped },
  { rows: 4, accepted: 1, geometry: 1, traffic: 1, metadata: 1 })
  assert.deepEqual(parsed.stations[0], station())
  assert.equal(normalizeAnasRef('RA05bis'), 'RA 5')
  assert.equal(normalizeItalianOsmRef('E45; SR 106'), 'SS 106')
  for (const roadClass of [0, 2, 3, 5]) {
    assert.equal(Object.values(splitItalianTgm(1000, roadClass)).reduce((a, b) => a + b, 0), 1000)
  }
})

test('Italian matcher requires the normalized ref and strict 30 kilometre cap', () => {
  const measured = station()
  const index = indexItalianTgm([measured])
  assert.equal(matchItalianTgm(road({ ref: 'E45; A01' }), index), measured)
  assert.equal(matchItalianTgm(road({ ref: 'A2' }), index), null)
  assert.equal(matchItalianTgm(road({ midLat: 42.269 }), index), measured)
  assert.equal(matchItalianTgm(road({ midLat: 42.273 }), index), null)
})

test('z9 Italian pass writes split classes, retracts stale claims and enforces baked country', async () => {
  const prepared = join(DIRECTORY, 'prepared')
  const square = join(prepared, 'z9', '273', '190')
  mkdirSync(square, { recursive: true })
  const fixture = writeRoadsFixture('it-loader.arrow', [0, 0, 0], {
    origin: [12, 42], refs: ['A1', 'A1', 'A2'],
    countryCodes: [iso2Code('IT'), iso2Code('CH'), iso2Code('IT')],
    sourceIds: [0, SOURCE_ID_IT_NATIONAL_ROADS, SOURCE_ID_IT_NATIONAL_ROADS],
  })
  const target = join(square, 'roads.arrow')
  copyFileSync(fixture, target)
  const result = await enrichItalianRoads(prepared, [station()])
  assert.deepEqual({ matched: result.matched, retracted: result.retracted,
    skippedForeign: result.skippedForeign }, { matched: 1, retracted: 2, skippedForeign: 1 })
  const table = tableFromIPC(readFileSync(target))
  assert.deepEqual([...table.getChild('source_id')!], [SOURCE_ID_IT_NATIONAL_ROADS, 0, 0])
  assert.deepEqual(['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto']
    .map(name => table.getChild(name)!.get(0)), [780, 36, 144, 40])
})
