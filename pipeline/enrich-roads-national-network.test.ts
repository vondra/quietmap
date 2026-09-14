/** Source registry and Brazil policy regression for the shared national network runner. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { createHash } from 'node:crypto'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import { runNationalRoadNetworkPolicy } from './enrich-roads-national-network.js'
import { nationalRoadProxySourceId } from './enrich-roads-national-policy.js'
import { iso2Code } from './lib/prepared-grid.js'
import { writeRoadsFixture } from './lib/road-test-fixture.js'
import { DATASETS } from './lib/enrichment-datasets.js'
import { NATIONAL_ROAD_NETWORK_POLICIES } from './lib/road-national-network-policies/index.js'
import type { PinnedRoadLine } from './lib/pinned-road-lines.js'
import type { RoadRow } from './lib/roads-arrow.js'

const road = (latitude: number, longitude: number): RoadRow => ({
  startLat: latitude, startLon: longitude, endLat: latitude, endLon: longitude,
  midLat: latitude, midLon: longitude, roadClass: 1, ref: null, name: null,
  osmId: 1, existingSourceId: 0,
})
const line = (properties: Record<string, unknown>): PinnedRoadLine => ({
  observationId: 'fixture', coordinates: [[-46.7, -23.6], [-46.6, -23.5]], properties, relativePath: 'br/test',
})

test('network proxy registry coverage agrees with the source registry', () => {
  for (const policy of NATIONAL_ROAD_NETWORK_POLICIES.values()) {
    const dataset = DATASETS.find(candidate => candidate.key === `${policy.country.toLowerCase()}-national-roads`)!
    assert.equal(dataset.measurement, 'proxy')
    assert.deepEqual([...policy.coverage], dataset.roadCoverage)
  }
})

test('Brazil retains DNIT surface, concession, city multiplier and vehicle mix', () => {
  const policy = NATIONAL_ROAD_NETWORK_POLICIES.get('BR')!
  assert.deepEqual(policy.traffic(road(-23.6, -46.7), line({ Superficie: 'PAV', Administra: 'Concession' })),
    { light: 49000, medium: 7000, heavy: 10500, moto: 3500 })
  assert.deepEqual(policy.traffic(road(-10, -50), line({ Superficie: 'TER', Administra: '' })),
    { light: 1800, medium: 300, heavy: 750, moto: 150 })
})

test('disjoint network shards preserve whole-run bytes, border retractions and empty-shard admission', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'road-network-shards-'))
  after(() => rmSync(directory, { recursive: true, force: true }))
  const sourceId = nationalRoadProxySourceId('BR')
  const origins = [-46.7, -46.0]
  const squares = ['z9/189/290', 'z9/190/290']
  const source = JSON.stringify({ type: 'FeatureCollection', features: origins.map((longitude, id) => ({
    type: 'Feature', id, properties: { Superficie: 'PAV', Administra: 'Concession' },
    geometry: { type: 'LineString', coordinates: [[longitude, -23.6], [longitude + 0.01, -23.59]] },
  })) })
  writeFileSync(join(directory, 'network.geojson'), source)
  const policy = { ...NATIONAL_ROAD_NETWORK_POLICIES.get('BR')!, files: [{
    relativePath: 'network.geojson', sha256: createHash('sha256').update(source).digest('hex'),
  }] }
  const options = (scope: string) => ({ preparedDirectory: join(directory, scope),
    enrichmentDirectory: directory, enrichOnly: true, forceDownload: false })
  for (const scope of ['single', 'sharded']) {
    squares.forEach((square, index) => {
      mkdirSync(join(options(scope).preparedDirectory, square), { recursive: true })
      copyFileSync(writeRoadsFixture(`network-${scope}-${index}.arrow`, [1, 7, 1], {
        origin: [origins[index], -23.6], sourceIds: [0, sourceId, sourceId],
        countryCodes: [iso2Code('BR'), iso2Code('BR'), iso2Code('UY')],
      }), join(options(scope).preparedDirectory, square, 'roads.arrow'))
    })
  }
  const whole = await runNationalRoadNetworkPolicy(options('single'), policy)
  const shards = await Promise.all([0, 1].map(index =>
    runNationalRoadNetworkPolicy(options('sharded'), policy, { index, count: 2 })))
  for (const key of ['rows', 'matched', 'retracted', 'skipped', 'skippedForeign', 'squares', 'squaresUpdated'] as const) {
    assert.equal(shards.reduce((sum, part) => sum + part[key], 0), whole[key], key)
  }
  assert.equal(whole.matched, 2)
  assert.equal(whole.retracted, 4)
  for (const part of shards) {
    for (const key of ['sourceRows', 'sourceLines', 'invalidGeometrySkipped'] as const) assert.equal(part[key], whole[key])
  }
  const expected = squares.map(square => readFileSync(join(options('single').preparedDirectory, square, 'roads.arrow')))
  const assertBytes = () => squares.forEach((square, index) => {
    assert.deepEqual(readFileSync(join(options('sharded').preparedDirectory, square, 'roads.arrow')), expected[index])
    assert.deepEqual([...tableFromIPC(expected[index]).getChild('source_id')!], [sourceId, 0, 0])
  })
  assertBytes()
  const rerun = await Promise.all([0, 1, 2].map(index =>
    runNationalRoadNetworkPolicy(options('sharded'), policy, { index, count: 3 })))
  assert.ok(rerun.every(part => part.squaresUpdated === 0 && part.retracted === 0))
  assert.equal(rerun[2].rows, 0)
  assert.equal(rerun[2].squares, 0)
  assertBytes()
  await assert.rejects(runNationalRoadNetworkPolicy(options('missing'), policy, { index: 2, count: 3 }), /no BR roads.arrow squares/)
})
