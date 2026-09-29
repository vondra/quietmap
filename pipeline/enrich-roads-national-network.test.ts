/** Source registry and a run of the shared national network runner. */

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

test('network proxy registry coverage agrees with the source registry', () => {
  for (const policy of NATIONAL_ROAD_NETWORK_POLICIES.values()) {
    const dataset = DATASETS.find(candidate => candidate.key === `${policy.country.toLowerCase()}-national-roads`)!
    assert.equal(dataset.measurement, 'proxy')
    assert.deepEqual([...policy.coverage], dataset.roadCoverage)
  }
})

test('a network run stamps matched rows, retracts stale and foreign stamps, and rejects an empty scope', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'road-network-run-'))
  after(() => rmSync(directory, { recursive: true, force: true }))
  const sourceId = nationalRoadProxySourceId('PY')
  const origins = [-57.6, -56.9]
  const squares = ['z9/174/293', 'z9/175/293']
  const source = JSON.stringify({ type: 'FeatureCollection', features: origins.map((longitude, id) => ({
    type: 'Feature', id, properties: { TIPO_SUP: 'PCA' },
    geometry: { type: 'LineString', coordinates: [[longitude, -25.3], [longitude + 0.01, -25.29]] },
  })) })
  writeFileSync(join(directory, 'network.geojson'), source)
  const policy = { ...NATIONAL_ROAD_NETWORK_POLICIES.get('PY')!, files: [{
    relativePath: 'network.geojson', sha256: createHash('sha256').update(source).digest('hex'),
  }] }
  const options = (scope: string) => ({ preparedDirectory: join(directory, scope),
    enrichmentDirectory: directory, enrichOnly: true, forceDownload: false })
  squares.forEach((square, index) => {
    mkdirSync(join(options('prepared').preparedDirectory, square), { recursive: true })
    copyFileSync(writeRoadsFixture(`network-${index}.arrow`, [1, 7, 1], {
      origin: [origins[index], -25.3], sourceIds: [0, sourceId, sourceId],
      countryCodes: [iso2Code('PY'), iso2Code('PY'), iso2Code('AR')],
    }), join(options('prepared').preparedDirectory, square, 'roads.arrow'))
  })
  const whole = await runNationalRoadNetworkPolicy(options('prepared'), policy)
  assert.deepEqual({ matched: whole.matched, retracted: whole.retracted, squares: whole.squares }, { matched: 2, retracted: 4, squares: 2 })
  for (const square of squares) {
    const bytes = readFileSync(join(options('prepared').preparedDirectory, square, 'roads.arrow'))
    assert.deepEqual([...tableFromIPC(bytes).getChild('source_id')!], [sourceId, 0, 0])
  }
  assert.equal((await runNationalRoadNetworkPolicy(options('prepared'), policy)).squaresUpdated, 0)
  await assert.rejects(runNationalRoadNetworkPolicy(options('missing'), policy), /no PY roads.arrow squares/)
})
