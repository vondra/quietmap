/** Registry, policy semantics and baked-country tests for the shared national proxy runner. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import { enrichRoadsFromNationalPolicy, policySourceId } from './enrich-roads-national-policy.js'
import { DATASETS } from './lib/enrichment-datasets.js'
import { NATIONAL_ROAD_POLICIES } from './lib/road-national-policies/index.js'
import { iso2Code } from './lib/prepared-grid.js'
import { writeRoadsFixture } from './lib/road-test-fixture.js'
import type { RoadRow } from './lib/roads-arrow.js'

const DIRECTORY = mkdtempSync(join(tmpdir(), 'road-national-policy-test-'))
after(() => rmSync(DIRECTORY, { recursive: true, force: true }))

const road = (latitude: number, longitude: number, roadClass: number): RoadRow => ({
  startLat: latitude, startLon: longitude, endLat: latitude, endLon: longitude,
  midLat: latitude, midLon: longitude, roadClass, ref: null, name: null,
  osmId: 1, existingSourceId: 0,
})

test('all dev1 class-policy countries have one proxy registry entry with identical coverage', () => {
  assert.deepEqual([...NATIONAL_ROAD_POLICIES.keys()], [
    'CD', 'DZ', 'EG', 'ET', 'IQ', 'IR', 'KE', 'KZ',
    'MA', 'NG', 'RU', 'SD', 'TR', 'TZ', 'UA', 'UZ',
  ])
  for (const policy of NATIONAL_ROAD_POLICIES.values()) {
    const dataset = DATASETS.find(candidate => candidate.id === policySourceId(policy))!
    assert.equal(dataset.key, `${policy.country.toLowerCase()}-national-roads`)
    assert.equal(dataset.measurement, 'proxy')
    assert.deepEqual([...policy.coverage].sort((a, b) => a - b),
      [...dataset.roadCoverage!].sort((a, b) => a - b))
    for (const roadClass of policy.coverage) {
      const traffic = policy.traffic(road(
        (policy.bbox[0] + policy.bbox[2]) / 2,
        (policy.bbox[1] + policy.bbox[3]) / 2,
        roadClass,
      ))
      assert.ok(traffic)
      assert.ok(Object.values(traffic).every(value => Number.isSafeInteger(value) && value >= 0))
      assert.ok(Object.values(traffic).some(value => value > 0))
    }
  }
})

test('city and freight policies retain the dev1 regional vehicle mixes', () => {
  const cd = NATIONAL_ROAD_POLICIES.get('CD')!
  assert.deepEqual(cd.traffic(road(-4.325, 15.322, 1)),
    { light: 6250, medium: 2125, heavy: 1000, moto: 3125 })
  assert.deepEqual(cd.traffic(road(-5.56, 14.44, 1)),
    { light: 2100, medium: 400, heavy: 2000, moto: 500 })

  const kz = NATIONAL_ROAD_POLICIES.get('KZ')!
  assert.deepEqual(kz.traffic(road(52, 76, 1)),
    { light: 2800, medium: 160, heavy: 4960, moto: 80 })

  const ng = NATIONAL_ROAD_POLICIES.get('NG')!
  assert.deepEqual(ng.traffic(road(6.5, 3.38, 1)),
    { light: 10500, medium: 1500, heavy: 13500, moto: 4500 })
})

test('shared runner writes proxy values and retracts stale or foreign national stamps', async () => {
  const prepared = join(DIRECTORY, 'prepared')
  const square = join(prepared, 'z9', '277', '261')
  mkdirSync(square, { recursive: true })
  const policy = NATIONAL_ROAD_POLICIES.get('CD')!
  const sourceId = policySourceId(policy)
  const fixture = writeRoadsFixture('cd-policy.arrow', [1, 7, 1], {
    origin: [15.322, -4.325],
    countryCodes: [iso2Code('CD'), iso2Code('CD'), iso2Code('CG')],
    sourceIds: [0, sourceId, sourceId],
  })
  const target = join(square, 'roads.arrow')
  copyFileSync(fixture, target)

  const result = await enrichRoadsFromNationalPolicy(prepared, policy)
  assert.deepEqual({ matched: result.matched, retracted: result.retracted,
    skipped: result.skipped, skippedForeign: result.skippedForeign },
  { matched: 1, retracted: 2, skipped: 1, skippedForeign: 1 })
  const table = tableFromIPC(readFileSync(target))
  assert.deepEqual([...table.getChild('source_id')!], [sourceId, 0, 0])
  assert.deepEqual(['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto']
    .map(name => table.getChild(name)!.get(0)), [6250, 2125, 1000, 3125])
})
