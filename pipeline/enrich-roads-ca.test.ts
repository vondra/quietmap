/** Quebec MTQ parser, route matching and z9 ownership tests. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import { enrichCanadianRoads, indexQuebecDjma, matchQuebecDjma } from './enrich-roads-ca.js'
import { iso2Code } from './lib/prepared-grid.js'
import { normalizeQuebecOsmRef, parseQuebecDjmaSource, type QuebecDjmaSection } from './lib/roads-ca-source.js'
import { writeRoadsFixture } from './lib/road-test-fixture.js'
import { SOURCE_ID_CA_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import type { RoadRow } from './lib/roads-arrow.js'

const DIRECTORY = mkdtempSync(join(tmpdir(), 'enrich-roads-ca-test-'))
after(() => rmSync(DIRECTORY, { recursive: true, force: true }))

const feature = (overrides: Record<string, unknown> = {}) => ({
  type: 'Feature', properties: { rtss_debut: '00138-01',
    annee_en_cours: '2024 DJMA:1000 / %cam:10' },
  geometry: { type: 'LineString', coordinates: [[-73.01, 46.8], [-72.99, 46.8]] }, ...overrides,
})
const section = (overrides: Partial<QuebecDjmaSection> = {}): QuebecDjmaSection => ({
  sourceRow: 0, route: 138, rank: 1, latitude: 46.8, longitude: -73,
  total: 1000, truckPercent: 10, light: 890, medium: 20, heavy: 80, moto: 10,
  ...overrides,
})
const road = (overrides: Partial<RoadRow> = {}): RoadRow => ({
  startLat: 46.8, startLon: -73, endLat: 46.801, endLon: -72.999,
  midLat: 46.8, midLon: -73, ref: 'R-138 Est', name: null,
  osmId: 1, roadClass: 1, existingSourceId: 0, ...overrides,
})

test('Quebec parser uses latest available DJMA and exact vehicle total', () => {
  const parsed = parseQuebecDjmaSource(JSON.stringify({ type: 'FeatureCollection', features: [
    feature(), feature({ properties: { rtss_debut: '00138-01', annee_en_cours: '',
      annee2: '2023 DJMA:500 / %cam:' } }),
    feature({ properties: { rtss_debut: '09936-01', annee_en_cours: '2024 DJMA:100' } }),
    feature({ geometry: null }),
  ] }))
  assert.deepEqual({ rows: parsed.sourceRows, accepted: parsed.sections.length,
    route: parsed.invalidRouteSkipped, geometry: parsed.invalidGeometrySkipped },
  { rows: 4, accepted: 2, route: 1, geometry: 1 })
  assert.deepEqual(parsed.sections[0], section())
  for (const value of parsed.sections) {
    assert.equal(value.light + value.medium + value.heavy + value.moto, value.total)
  }
  assert.equal(normalizeQuebecOsmRef('20;132'), 20)
  assert.equal(normalizeQuebecOsmRef('Zelená 20'), null)
})

test('Quebec matcher requires route, compatible class and 25 kilometre proximity', () => {
  const measured = section()
  const index = indexQuebecDjma([measured])
  assert.equal(matchQuebecDjma(road(), index), measured)
  assert.equal(matchQuebecDjma(road({ ref: 'R-139' }), index), null)
  assert.equal(matchQuebecDjma(road({ roadClass: 4 }), index), null)
  assert.equal(matchQuebecDjma(road({ midLat: 47.02 }), index), measured)
  assert.equal(matchQuebecDjma(road({ midLat: 47.03 }), index), null)
})

test('z9 Quebec pass writes measured classes and enforces baked Canada ownership', async () => {
  const prepared = join(DIRECTORY, 'prepared')
  const square = join(prepared, 'z9', '152', '179')
  mkdirSync(square, { recursive: true })
  const fixture = writeRoadsFixture('ca-loader.arrow', [1, 1, 7], {
    origin: [-73, 46.8], refs: ['R-138', 'R-138', 'R-138'],
    countryCodes: [iso2Code('CA'), iso2Code('US'), iso2Code('CA')],
    sourceIds: [0, SOURCE_ID_CA_NATIONAL_ROADS, SOURCE_ID_CA_NATIONAL_ROADS],
  })
  const target = join(square, 'roads.arrow')
  copyFileSync(fixture, target)
  const result = await enrichCanadianRoads(prepared, [section()])
  assert.deepEqual({ matched: result.matched, retracted: result.retracted,
    skipped: result.skipped, skippedForeign: result.skippedForeign },
  { matched: 1, retracted: 2, skipped: 1, skippedForeign: 1 })
  const table = tableFromIPC(readFileSync(target))
  assert.deepEqual([...table.getChild('source_id')!], [SOURCE_ID_CA_NATIONAL_ROADS, 0, 0])
  assert.deepEqual(['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto']
    .map(name => table.getChild(name)!.get(0)), [890, 20, 80, 10])
})
