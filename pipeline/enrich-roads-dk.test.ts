/** Danish Mastra parser and z9 ownership tests. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import proj4 from 'proj4'
import { enrichDanishRoads } from './enrich-roads-dk.js'
import { iso2Code } from './lib/prepared-grid.js'
import { parseDanishMastraPages, type DanishMastraObservation } from './lib/roads-dk-source.js'
import { writeRoadsFixture } from './lib/road-test-fixture.js'
import { SOURCE_ID_DK_NATIONAL_ROADS } from './lib/source-ids.generated.js'

const DIRECTORY = mkdtempSync(join(tmpdir(), 'enrich-roads-dk-test-'))
after(() => rmSync(DIRECTORY, { recursive: true, force: true }))

const observation = (overrides: Partial<DanishMastraObservation> = {}): DanishMastraObservation => ({
  sourceRow: 0, roadNumber: 10, kilometre: 1, year: 2025,
  latitude: 55.7, longitude: 12.5, rank: 1, total: 1000,
  light: 890, medium: 5, heavy: 95, moto: 10, ...overrides,
})
const feature = (properties: Record<string, unknown>, longitude = 12.5, latitude = 55.7) => {
  const [x, y] = proj4('WGS84', 'EPSG:25832', [longitude, latitude])
  return { type: 'Feature', properties, geometry: { type: 'Point', coordinates: [x, y] } }
}
const collection = (features: readonly unknown[]) => JSON.stringify({ type: 'FeatureCollection', features })

proj4.defs('EPSG:25832', '+proj=utm +zone=32 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs')

test('Mastra parser keeps the latest station year and preserves exact totals', () => {
  const base = { KOERETOEJSART: 'MOTORKTJ', VEJNR: 10, KILOMETER: 1,
    AAR: 2024, AADT: 900, LBIL_AADT: 90, VEJBESTYRER: 0 }
  const parsed = parseDanishMastraPages([collection([
    feature(base), feature({ ...base, AAR: 2025, AADT: 1000, LBIL_AADT: 100 }),
    feature({ ...base, VEJNR: 11, KOERETOEJSART: 'CYKEL' }),
    feature({ ...base, VEJNR: 12, AADT: 0 }),
  ])])
  assert.deepEqual({ sourceRows: parsed.sourceRows, admitted: parsed.admittedRecords,
    observations: parsed.observations.length, superseded: parsed.supersededRecords,
    nonMotor: parsed.nonMotorSkipped, invalid: parsed.invalidRowsSkipped },
  { sourceRows: 4, admitted: 2, observations: 1, superseded: 1, nonMotor: 1, invalid: 1 })
  assert.deepEqual({ ...parsed.observations[0], latitude: 55.7, longitude: 12.5 },
    observation({ sourceRow: 1 }))
  assert.ok(Math.abs(parsed.observations[0].latitude - 55.7) < 1e-9)
  assert.ok(Math.abs(parsed.observations[0].longitude - 12.5) < 1e-9)
  assert.equal(parsed.observations[0].light + parsed.observations[0].medium +
    parsed.observations[0].heavy + parsed.observations[0].moto, 1000)
})

test('z9 Danish pass matches class-compatible points and enforces baked country', async () => {
  const prepared = join(DIRECTORY, 'prepared')
  const square = join(prepared, 'z9', '273', '160')
  mkdirSync(square, { recursive: true })
  const fixture = writeRoadsFixture('dk-loader.arrow', [1, 1, 4, 7], {
    origin: [12.5, 55.7],
    countryCodes: [iso2Code('DK'), iso2Code('SE'), iso2Code('DK'), iso2Code('DK')],
    sourceIds: [0, SOURCE_ID_DK_NATIONAL_ROADS, SOURCE_ID_DK_NATIONAL_ROADS,
      SOURCE_ID_DK_NATIONAL_ROADS],
  })
  const target = join(square, 'roads.arrow')
  copyFileSync(fixture, target)
  const result = await enrichDanishRoads(prepared, [observation()])
  assert.deepEqual({ matched: result.matched, retracted: result.retracted,
    skipped: result.skipped, skippedForeign: result.skippedForeign },
  { matched: 1, retracted: 3, skipped: 1, skippedForeign: 1 })
  const table = tableFromIPC(readFileSync(target))
  assert.deepEqual([...table.getChild('source_id')!], [SOURCE_ID_DK_NATIONAL_ROADS, 0, 0, 0])
  assert.deepEqual(['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto']
    .map(name => table.getChild(name)!.get(0)), [890, 5, 95, 10])
})
