/** Irish TII parser, ref matching and z9 ownership tests. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import { enrichIrishRoads, indexIrishTii, matchIrishTii } from './enrich-roads-ie.js'
import { iso2Code } from './lib/prepared-grid.js'
import { normalizeIrishRoadRef, parseIrishTiiSource, type IrishTiiObservation } from './lib/roads-ie-source.js'
import { writeRoadsFixture } from './lib/road-test-fixture.js'
import { SOURCE_ID_IE_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import type { RoadRow } from './lib/roads-arrow.js'

const DIRECTORY = mkdtempSync(join(tmpdir(), 'enrich-roads-ie-test-'))
after(() => rmSync(DIRECTORY, { recursive: true, force: true }))

const observation = (overrides: Partial<IrishTiiObservation> = {}): IrishTiiObservation => ({
  cosit: '1', ref: 'M1', latitude: 53.4, longitude: -8,
  light: 100, medium: 20, heavy: 50, moto: 10, ...overrides,
})
const road = (overrides: Partial<RoadRow> = {}): RoadRow => ({
  startLat: 53.4, startLon: -8, endLat: 53.401, endLon: -7.999,
  midLat: 53.4, midLon: -8, ref: 'M01', name: null,
  osmId: 1, roadClass: 0, existingSourceId: 0, ...overrides,
})

test('Irish parser joins sites to exact class counts and normalizes refs', () => {
  const sites = JSON.stringify([
    { cosit: '1', name: 'TMU M01 000.0 N', location: { lat: 53.4, lng: -8 } },
    { cosit: '2', name: 'test', location: { lat: 53.4, lng: -8 } },
  ])
  const counts = 'cosit,class,year,month,day,VehicleCount\n' +
    '1,1,2019,6,19,10\n1,2,2019,6,19,80\n1,3,2019,6,19,20\n' +
    '1,4,2019,6,19,20\n1,5,2019,6,19,10\n1,6,2019,6,19,15\n1,7,2019,6,19,25\n'
  const parsed = parseIrishTiiSource(sites, counts)
  assert.deepEqual({ siteRows: parsed.siteRows, countRows: parsed.countRows,
    usableSites: parsed.usableSites, countedSites: parsed.countedSites },
  { siteRows: 2, countRows: 7, usableSites: 1, countedSites: 1 })
  assert.deepEqual(parsed.observations[0], observation())
  assert.equal(normalizeIrishRoadRef('N-007'), 'N7')
  assert.equal(normalizeIrishRoadRef('street 12'), '')
})

test('Irish matcher requires ref and strict 25 kilometre proximity', () => {
  const measured = observation()
  const index = indexIrishTii([measured])
  assert.equal(matchIrishTii(road(), index), measured)
  assert.equal(matchIrishTii(road({ ref: 'N1' }), index), null)
  assert.equal(matchIrishTii(road({ ref: 'R132;M1' }), index), measured)
  assert.equal(matchIrishTii(road({ midLat: 53.62 }), index), measured)
  assert.equal(matchIrishTii(road({ midLat: 53.63 }), index), null)
})

test('z9 Irish pass writes exact classes and enforces baked ownership', async () => {
  const prepared = join(DIRECTORY, 'prepared')
  const square = join(prepared, 'z9', '244', '165')
  mkdirSync(square, { recursive: true })
  const fixture = writeRoadsFixture('ie-loader.arrow', [0, 0, 0], {
    origin: [-8, 53.4], refs: ['M1', 'M1', 'N1'],
    countryCodes: [iso2Code('IE'), iso2Code('GB'), iso2Code('IE')],
    sourceIds: [0, SOURCE_ID_IE_NATIONAL_ROADS, SOURCE_ID_IE_NATIONAL_ROADS],
  })
  const target = join(square, 'roads.arrow')
  copyFileSync(fixture, target)
  const result = await enrichIrishRoads(prepared, [observation()])
  assert.deepEqual({ matched: result.matched, retracted: result.retracted,
    skippedForeign: result.skippedForeign }, { matched: 1, retracted: 2, skippedForeign: 1 })
  const table = tableFromIPC(readFileSync(target))
  assert.deepEqual([...table.getChild('source_id')!], [SOURCE_ID_IE_NATIONAL_ROADS, 0, 0])
  assert.deepEqual(['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto']
    .map(name => table.getChild(name)!.get(0)), [100, 20, 50, 10])
})
