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

test('source rounding, aliases and the whole line preserve published directional counts', () => {
  const source = parseEuropeanCityTraffic('sample', 'sample.geojson', bytes(
    feature({ AADT: 1000.4, AAWT: 2000, TR_AADT: 100.4, '2W_AADT': 50.4, raw_oneway: true }),
    feature({ AADT: null, AAWT: 1000, TR_AADT: null, TR_AAWT: 100, raw_oneway: 'true' },
      { type: 'LineString', coordinates: [[14, 50], [14, 50], [14.001, 50.001]] }),
  ))
  // Source direction remains separate from the matched OSM road direction.
  assert.deepEqual(source.records[0], {
    coordinates: [[14, 50]], light: 830, medium: 20, heavy: 100, moto: 50, sourceId: 10,
    observationId: source.records[0].observationId, sourceOsmId: null, estimatedClasses: 3,
    countBasis: 'directional', rawOneway: true, rawDirection: null, osmOneway: null, rawTechnology: null,
  })
  assert.deepEqual(source.records[1], {
    coordinates: [[14, 50], [14.001, 50.001]], light: 880, medium: 20, heavy: 100, moto: 0, sourceId: 10,
    observationId: source.records[1].observationId, sourceOsmId: null, estimatedClasses: 11,
    countBasis: 'unknown', rawOneway: 'true', rawDirection: null, osmOneway: null, rawTechnology: null,
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
    bytes(feature({ AADT: 2 ** 32, raw_oneway: true }))]) {
    writeFileSync(last, invalid)
    assert.throws(() => loadEuropeanCityTraffic(directory), /Cardiff/)
  }
  rmSync(last)
  assert.throws(() => loadEuropeanCityTraffic(directory), /Cardiff: missing/)
})

test('source basis survives OSM disagreement and heavy-only counts gain no invented vehicles', () => {
  const source = parseEuropeanCityTraffic('fixture', 'raw.geojson', bytes(
    feature({ AADT: 10000, raw_oneway: true, raw_direction: 'A-->B', osm_oneway: 'False',
      raw_techno: 'Estimated using previous year' }),
    feature({ AADT: 10000, raw_oneway: false, osm_oneway: 'True' }),
    feature({ AADT: 500, TR_AADT: 500, raw_oneway: true }),
  ))
  assert.equal(source.records[0].countBasis, 'directional')
  assert.equal(source.records[0].rawDirection, 'A-->B')
  assert.equal(source.records[0].osmOneway, 'False')
  assert.equal(source.records[0].rawTechnology, 'Estimated using previous year')
  assert.equal(source.records[1].countBasis, 'both-directions')
  assert.equal(source.records[1].osmOneway, 'True')
  assert.deepEqual(['light', 'medium', 'heavy', 'moto'].map(key =>
    source.records[2][key as 'light' | 'medium' | 'heavy' | 'moto']), [0, 0, 500, 0])
})
