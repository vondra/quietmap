/** Source admission regressions for complete, immutable EU city traffic caches. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { basename, join } from 'node:path'
import {
  EUROPEAN_TRAFFIC_CITIES, latestStagedCityFile, loadEuropeanCityTraffic, parseEuropeanCityTraffic,
} from './roads-europe-source.js'

const temporary = mkdtempSync(join(tmpdir(), 'eu-traffic-source-'))
after(() => rmSync(temporary, { recursive: true, force: true }))
const feature = (properties: Record<string, unknown>, geometry: unknown = { type: 'Point', coordinates: [14, 50] }) =>
  ({ type: 'Feature', properties, geometry })
const bytes = (...features: unknown[]) => Buffer.from(JSON.stringify({ type: 'FeatureCollection', features }))

test('latest staged raw is deterministic and ignores stale normalized copies', () => {
  for (const name of ['Berlin_AADT_AAWT_2021.geojson', 'Berlin_AADT_AAWT_2023.geojson',
    'Berlin_AAWT_2023.geojson', 'Brno_AADT_2019.geojson', 'berlin.geojson']) {
    writeFileSync(join(temporary, name), name)
  }
  assert.equal(basename(latestStagedCityFile('berlin', temporary)), 'Berlin_AADT_AAWT_2023.geojson')
  assert.equal(basename(latestStagedCityFile('Brno', temporary)), 'Brno_AADT_2019.geojson')
  assert.throws(() => latestStagedCityFile('Paris', temporary), /missing staged/)
})

test('source rounding, aliases, line vertex and directional four-class split retain dev1 semantics', () => {
  const source = parseEuropeanCityTraffic('sample', 'sample.geojson', bytes(
    feature({ AADT: 1000.4, AAWT: 2000, TR_AADT: 100.4, '2W_AADT': 50.4, raw_oneway: true }),
    feature({ AADT: null, AAWT: 1000, TR_AADT: null, TR_AAWT: 100, raw_oneway: 'true' },
      { type: 'LineString', coordinates: [[14, 50], [14.001, 50.001]] }),
  ))
  assert.deepEqual(source.records[0], {
    latitude: 50, longitude: 14, light: 1660, medium: 40, heavy: 200, moto: 100, sourceId: 10,
  })
  assert.deepEqual(source.records[1], {
    latitude: 50.001, longitude: 14.001, light: 880, medium: 20, heavy: 100, moto: 0, sourceId: 10,
  })
  assert.equal(source.nonBooleanOneway, 1)
})

test('contradictory published components and rounded-zero observations cannot become measured rows', () => {
  const source = parseEuropeanCityTraffic('Toulouse', 'raw.geojson', bytes(
    feature({ AAWT: 218, TR_AAWT: 4825, raw_oneway: true }),
    feature({ AADT: 0.1 }), feature({ AADT: 1000, TR_AADT: 25 }),
  ))
  assert.equal(source.features, 3)
  assert.equal(source.records.length, 1)
  assert.deepEqual(source.rejected, [
    { feature: 0, reason: 'components_exceed_total', total: 218, truck: 4825, motorcycle: 0 },
    { feature: 1, reason: 'rounds_to_zero', total: 0.1, truck: 0, motorcycle: 0 },
  ])
  assert.equal(source.features, source.records.length + source.rejected.length)
})

test('all 36 nonempty finite city inputs are required before a load can succeed and cache identity stays unchanged', () => {
  const directory = mkdtempSync(join(temporary, 'all-cities-'))
  for (const city of EUROPEAN_TRAFFIC_CITIES) {
    writeFileSync(join(directory, `${city}_AADT_2023.geojson`), bytes(feature({ AADT: 100 })))
  }
  const identity = () => Object.fromEntries(readdirSync(directory).map(name => {
    const path = join(directory, name)
    const stat = statSync(path)
    return [name, [stat.ino, stat.size, stat.mtimeMs, stat.ctimeMs, readFileSync(path).toString('hex')]]
  }))
  const before = identity()
  const sources = loadEuropeanCityTraffic(directory)
  assert.equal(sources.length, 36)
  assert.equal(sources.reduce((sum, city) => sum + city.records.length, 0), 36)
  assert.deepEqual(identity(), before)
  const last = join(directory, 'Cardiff_AADT_2023.geojson')
  for (const invalid of [bytes(), bytes(feature({ AADT: -1 })),
    Buffer.from('{"type":"FeatureCollection","features":[{"properties":{"AADT":1e999}}]}'),
    bytes(feature({ AADT: 100 }, { type: 'LineString', coordinates: [] })),
    bytes(feature({ AADT: 2 ** 31, raw_oneway: true }))]) {
    writeFileSync(last, invalid)
    assert.throws(() => loadEuropeanCityTraffic(directory), /Cardiff/)
  }
  rmSync(last)
  assert.throws(() => loadEuropeanCityTraffic(directory), /Cardiff: missing/)
})
