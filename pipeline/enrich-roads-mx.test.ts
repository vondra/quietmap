/** Mexican SICT parser, complete line grid and z9 ownership tests. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import { buildMexicanLineGrid, enrichMexicanRoads, matchMexicanSict } from './enrich-roads-mx.js'
import { iso2Code } from './lib/prepared-grid.js'
import { mexicanAllowedRoadClassMask, parseMexicanSictSource, splitMexicanTdpa,
  type MexicanSictSegment } from './lib/roads-mx-source.js'
import { writeRoadsFixture } from './lib/road-test-fixture.js'
import { SOURCE_ID_MX_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import type { RoadRow } from './lib/roads-arrow.js'

const DIRECTORY = mkdtempSync(join(tmpdir(), 'enrich-roads-mx-test-'))
after(() => rmSync(DIRECTORY, { recursive: true, force: true }))

const segment = (overrides: Partial<MexicanSictSegment> = {}): MexicanSictSegment => ({
  sourceRow: 0, lines: [[[-100.1, 20], [-99.9, 20]]], total: 1000,
  fractions: { light: 0.8, medium: 0.07, heavy: 0.08, moto: 0.05 },
  allowedRoadClassMask: mexicanAllowedRoadClassMask('Federal', 'Libre'), ...overrides,
})
const road = (overrides: Partial<RoadRow> = {}): RoadRow => ({
  startLat: 20, startLon: -100, endLat: 20.001, endLon: -99.999,
  midLat: 20, midLon: -100, ref: null, name: null,
  osmId: 1, roadClass: 1, existingSourceId: 0, ...overrides,
})

test('Mexican parser joins composition, rejects zero TDPA and preserves exact totals', () => {
  const composition = 'segment_mongo_id,comp_A,comp_B,comp_C2,comp_C3,comp_T3S2,comp_T3S3,comp_T3S2R4,comp_M,comp_OTROS\n' +
    'a,0.7,0.02,0.05,0.03,0.03,0.01,0.01,0.05,0.1\n'
  const feature = (id: string, total: number) => ({ type: 'Feature', properties: {
    segment_mongo_id: id, tdpa_2024: total, red_ok: 'Federal', operacion: 'Libre',
  }, geometry: { type: 'LineString', coordinates: [[-100.1, 20], [-99.9, 20]] } })
  const parsed = parseMexicanSictSource(JSON.stringify({ type: 'FeatureCollection', features: [
    feature('a', 1000), feature('missing', 500), feature('zero', 0),
  ] }), composition)
  assert.deepEqual({ rows: parsed.sourceRows, composition: parsed.compositionRows,
    accepted: parsed.segments.length, unavailable: parsed.unavailableTrafficSkipped,
    missing: parsed.missingCompositionRows, fallback: parsed.fallbackCompositionRows },
  { rows: 3, composition: 1, accepted: 2, unavailable: 1, missing: 1, fallback: 1 })
  assert.deepEqual(splitMexicanTdpa(1000, parsed.segments[0].fractions),
    { light: 800, medium: 70, heavy: 80, moto: 50 })
  assert.equal(Object.values(splitMexicanTdpa(17, parsed.segments[0].fractions))
    .reduce((sum, value) => sum + value, 0), 17)
})

test('Mexican grid samples sparse edges and requires a compatible road class within 200 metres', () => {
  const measured = segment()
  const grid = buildMexicanLineGrid([measured])
  assert.equal(matchMexicanSict(road(), grid), measured)
  assert.equal(matchMexicanSict(road({ roadClass: 0 }), grid), null)
  assert.equal(matchMexicanSict(road({ midLat: 20.0017 }), grid), measured)
  assert.equal(matchMexicanSict(road({ midLat: 20.0019 }), grid), null)
})

test('z9 Mexican pass writes TDPA and enforces baked country and high-order coverage', async () => {
  const prepared = join(DIRECTORY, 'prepared')
  const square = join(prepared, 'z9', '113', '226')
  mkdirSync(square, { recursive: true })
  const fixture = writeRoadsFixture('mx-loader.arrow', [1, 1, 4], {
    origin: [-100, 20],
    countryCodes: [iso2Code('MX'), iso2Code('US'), iso2Code('MX')],
    sourceIds: [0, SOURCE_ID_MX_NATIONAL_ROADS, SOURCE_ID_MX_NATIONAL_ROADS],
  })
  const target = join(square, 'roads.arrow')
  copyFileSync(fixture, target)
  const result = await enrichMexicanRoads(prepared, [segment()])
  assert.deepEqual({ matched: result.matched, retracted: result.retracted,
    skipped: result.skipped, skippedForeign: result.skippedForeign },
  { matched: 1, retracted: 2, skipped: 1, skippedForeign: 1 })
  const table = tableFromIPC(readFileSync(target))
  assert.deepEqual([...table.getChild('source_id')!], [SOURCE_ID_MX_NATIONAL_ROADS, 0, 0])
  assert.deepEqual(['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto']
    .map(name => table.getChild(name)!.get(0)), [800, 70, 80, 50])
})
