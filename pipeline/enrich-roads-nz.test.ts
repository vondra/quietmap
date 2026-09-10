/** New Zealand road-source parser and z9 ownership tests. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import { enrichNewZealandRoads } from './enrich-roads-nz.js'
import { iso2Code } from './lib/prepared-grid.js'
import { newZealandOnrcRank, parseNewZealandRoadSources, type NewZealandRoadObservation } from './lib/roads-nz-source.js'
import { writeRoadsFixture } from './lib/road-test-fixture.js'
import { SOURCE_ID_NZ_NATIONAL_ROADS } from './lib/source-ids.generated.js'

const DIRECTORY = mkdtempSync(join(tmpdir(), 'enrich-roads-nz-test-'))
after(() => rmSync(DIRECTORY, { recursive: true, force: true }))

const collection = (features: readonly unknown[]) => JSON.stringify({ type: 'FeatureCollection', features })
const nzta = (overrides: Record<string, unknown> = {}) => ({
  type: 'Feature', properties: { trafficADTEst: 1000, trafficADTCount: null,
    loadingPcHeavy: 20, ONRC: 'Regional' },
  geometry: { type: 'LineString', coordinates: [[174.79, -36.9], [174.81, -36.9]] }, ...overrides,
})
const at = (overrides: Record<string, unknown> = {}) => ({
  type: 'Feature', properties: { adt: 500, pcheavy: 10 },
  geometry: { type: 'Point', coordinates: [174.8, -36.9] }, ...overrides,
})
const observation = (overrides: Partial<NewZealandRoadObservation> = {}): NewZealandRoadObservation => ({
  source: 'nzta', sourceRow: 0, latitude: -36.9, longitude: 174.8, rank: 1,
  total: 1000, heavyPercent: 20, light: 790, medium: 40, heavy: 160, moto: 10,
  ...overrides,
})

test('NZ parser combines pages and Auckland points with exact totals and explicit rejects', () => {
  const parsed = parseNewZealandRoadSources([
    collection([nzta(), nzta({ properties: { trafficADTEst: 100, ONRC: 'Access' } }),
      nzta({ properties: { trafficADTEst: 0, ONRC: 'National' } }), nzta({ geometry: null })]),
  ], collection([at()]))
  assert.deepEqual({ nztaRows: parsed.nztaRows, atRows: parsed.atRows,
    accepted: parsed.observations.length, traffic: parsed.unavailableTrafficSkipped,
    unsupported: parsed.unsupportedClassSkipped, geometry: parsed.invalidGeometrySkipped },
  { nztaRows: 4, atRows: 1, accepted: 2, traffic: 1, unsupported: 1, geometry: 1 })
  assert.deepEqual(parsed.observations[0], observation())
  assert.equal(newZealandOnrcRank('Secondary Collector'), 4)
  assert.equal(newZealandOnrcRank('Access'), null)
  for (const value of parsed.observations) {
    assert.equal(value.light + value.medium + value.heavy + value.moto, value.total)
  }
})

test('z9 NZ pass matches compatible NZTA rows and enforces baked country', async () => {
  const prepared = join(DIRECTORY, 'prepared')
  const square = join(prepared, 'z9', '504', '312')
  mkdirSync(square, { recursive: true })
  const fixture = writeRoadsFixture('nz-loader.arrow', [1, 1, 4, 7], {
    origin: [174.8, -36.9],
    countryCodes: [iso2Code('NZ'), iso2Code('AU'), iso2Code('NZ'), iso2Code('NZ')],
    sourceIds: [0, SOURCE_ID_NZ_NATIONAL_ROADS, SOURCE_ID_NZ_NATIONAL_ROADS, SOURCE_ID_NZ_NATIONAL_ROADS],
  })
  const target = join(square, 'roads.arrow')
  copyFileSync(fixture, target)
  const result = await enrichNewZealandRoads(prepared, [observation()])
  assert.deepEqual({ matched: result.matched, retracted: result.retracted,
    skipped: result.skipped, skippedForeign: result.skippedForeign },
  { matched: 1, retracted: 3, skipped: 1, skippedForeign: 1 })
  const table = tableFromIPC(readFileSync(target))
  assert.deepEqual([...table.getChild('source_id')!], [SOURCE_ID_NZ_NATIONAL_ROADS, 0, 0, 0])
  assert.deepEqual(['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto']
    .map(name => table.getChild(name)!.get(0)), [790, 40, 160, 10])
})
