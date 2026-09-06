/** Municipal targeting and actual Arrow preservation across city, country and class boundaries. */

import { test, after } from 'node:test'
import assert from 'node:assert/strict'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { tmpdir } from 'node:os'
import { RecordBatch, Schema, Table, Uint8, tableFromIPC, tableToIPC, vectorFromArray } from 'apache-arrow'
import { municipalRoadMatcher, cityRecordDistance } from './lib/city-roads-match.js'
import { MUNICIPAL_ROAD_SOURCES, type CityRoadRecord } from './lib/city-roads-source.js'
import { municipalityFromGeoJson } from './lib/city-polygon.js'
import { iso2Code } from './lib/prepared-grid.js'
import { writeRoadsFixture } from './lib/road-test-fixture.js'
import type { RoadRow } from './lib/roads-arrow.js'
import { enrichMunicipalRoads } from './enrich-cities-roads.js'

const directory = mkdtempSync(resolve(tmpdir(), 'municipal-road-tests-'))
after(() => rmSync(directory, { recursive: true, force: true }))
const coverage = new Set([1, 2, 3, 4, 5, 9, 11, 12])
const record = (street: string, line?: CityRoadRecord['line']): CityRoadRecord => ({ street, light: 9000, medium: 10, heavy: 500, moto: 0, ...(line ? { line } : {}) })
function row(name: string, roadClass: number, offsetM: number, vertical = false): RoadRow {
  const lat = 48.2 + offsetM / 110540, lon = 16.37
  return { name, roadClass, startLat: lat, startLon: lon, endLat: vertical ? lat + 0.0008 : lat,
    endLon: vertical ? lon : lon + 0.001, midLat: vertical ? lat + 0.0004 : lat,
    midLon: vertical ? lon : lon + 0.0005, ref: null, osmId: null, existingSourceId: 0 }
}

test('point counters target the nearby arterial; excluded motorways compete without feeding side streets', () => {
  const station = record('Side street', [[16.3705, 48.2]])
  const arterial = row('Arterial', 2, 10), side = row('Side street', 5, 9), far = row('Far trunk', 1, 30)
  const matcher = municipalRoadMatcher([station], coverage)
  for (const r of [arterial, side, far]) matcher.observe(r)
  matcher.finish()
  assert.equal(matcher.match(arterial), station)
  assert.equal(matcher.match(side), null)
  assert.equal(matcher.match(far), null)
  const motorway = row('D1', 0, 0), next = municipalRoadMatcher([station], coverage)
  for (const r of [motorway, side]) next.observe(r)
  next.finish()
  assert.equal(next.match(side), null)
})

test('section targeting requires along-line endpoints, 28m name proximity and the exact40m final cap', () => {
  const section = record('section', [[16.369, 48.2], [16.373, 48.2]])
  const primary = row('Main', 2, 0), parallel = row('Parallel', 5, 29), perpendicular = row('Crossing', 5, 0, true)
  const matcher = municipalRoadMatcher([section], coverage)
  for (const r of [primary, parallel, perpendicular]) matcher.observe(r)
  matcher.finish()
  assert.equal(matcher.match(primary), section)
  assert.equal(matcher.match(parallel), null)
  assert.equal(matcher.match(perpendicular), null)
  assert.ok(cityRecordDistance(section, row('Main', 2, 39.99)) < 40)
  assert.equal(matcher.match(row('Main', 2, 39.99)), section)
  assert.equal(matcher.match(row('Main', 2, 40.01)), null)
})

function city(records: CityRoadRecord[]) {
  const ring = [[14.42, 50.07], [14.8, 50.07], [14.8, 50.10], [14.42, 50.10], [14.42, 50.07]]
  const hole = [[14.4309, 50.0809], [14.4317, 50.0809], [14.4317, 50.0817], [14.4309, 50.0817], [14.4309, 50.0809]]
  return { ...MUNICIPAL_ROAD_SOURCES[0], year: 2025, admission: { year: 2025, sections: 953 }, records, zeroSplitSkipped: 0, coverage,
    municipality: municipalityFromGeoJson(JSON.stringify({ type: 'FeatureCollection', features: [{ properties: { shapeName: 'city' }, geometry: { type: 'Polygon', coordinates: [ring, hole] } }] }), 'city') }
}

test('city/native priorities, hole and foreign ownership preserve IPC geometry, metadata, batches and exact reruns', async () => {
  const original = writeRoadsFixture('municipal.arrow', [2, 2, 2, 0, 2, 2], {
    origin: [14.43, 50.08], countryCodes: [iso2Code('CZ'), iso2Code('CZ'), iso2Code('AT'), iso2Code('CZ'), iso2Code('CZ'), iso2Code('CZ')],
    sourceIds: [20, 20, 20, 20, 9004, 9003], speeds: [50, 50, 50, 130, 50, 50],
  })
  const root = resolve(directory, 'ipc'), path = resolve(root, 'z9/276/173/roads.arrow')
  mkdirSync(resolve(path, '..'), { recursive: true }); copyFileSync(original, path)
  const input = tableFromIPC(readFileSync(path)), columns = Object.fromEntries(input.schema.fields.map(f => [f.name, input.getChild(f.name)!]))
  const stock = new Table({ ...columns, built_up: vectorFromArray([1, 0, 1, 0, 1, 0], new Uint8()) })
  const schema = new Schema(stock.schema.fields, new Map([...input.schema.metadata, ['test-preserve', 'municipal'], ['qm_batch_bboxes', '[[50.07,14.42,50.1,14.45],[50.07,14.42,50.1,14.45]]']]))
  const split = new Table(schema, [stock.slice(0, 3), stock.slice(3, 6)].map(t => new RecordBatch(schema, t.batches[0].data)))
  writeFileSync(path, tableToIPC(split, 'file'))
  const selected = city(Array.from({ length: 5 }, (_, i) => record(`ROAD ${i}`)))
  const result = await enrichMunicipalRoads(root, [selected])
  assert.equal(result[0].matched, 1)
  const output = tableFromIPC(readFileSync(path))
  assert.deepEqual(output.getChild('source_id')!.toArray(), new Uint16Array([9003, 20, 20, 20, 9004, 0]))
  assert.deepEqual(output.schema.metadata, split.schema.metadata)
  assert.deepEqual(output.batches.map(b => b.numRows), [3, 3])
  for (const field of split.schema.fields) if (!['source_id', 'aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto'].includes(field.name)) assert.deepEqual(output.getChild(field.name)!.toArray(), split.getChild(field.name)!.toArray())
  const before = readFileSync(path), stat = statSync(path, { bigint: true })
  assert.equal((await enrichMunicipalRoads(root, [selected]))[0].updated, 0)
  assert.deepEqual(readFileSync(path), before)
  assert.equal(statSync(path, { bigint: true }).mtimeNs, stat.mtimeNs)
  const corrupt = writeRoadsFixture('missing-admin-city.arrow', [2], { origin: [14.43, 50.08], omitCountryContract: true })
  const badPath = resolve(root, 'z9/277/173/roads.arrow'); mkdirSync(resolve(badPath, '..'), { recursive: true }); copyFileSync(corrupt, badPath)
  await assert.rejects(enrichMunicipalRoads(root, [selected]), /country_baked_v1/)
  assert.deepEqual(readFileSync(path), before)
})


test('zero-match failure leaves existing city stamps untouched', async () => {
  const root = resolve(directory, 'zero-match-failure'), path = resolve(root, 'z9/276/173/roads.arrow')
  mkdirSync(resolve(path, '..'), { recursive: true })
  const original = writeRoadsFixture('zero-match.arrow', [2], {
    origin: [14.43, 50.08], countryCodes: [iso2Code('CZ')], sourceIds: [9003],
  })
  copyFileSync(original, path)
  assert.deepEqual(tableFromIPC(readFileSync(path)).getChild('source_id')!.toArray(), new Uint16Array([9003]))
  const before = readFileSync(path)
  const selected = city([record('Road 0')]), failing = city([record('Admitted but absent street')])
  await assert.rejects(enrichMunicipalRoads(root, [selected, failing]), /no rows matched admitted municipal traffic/)
  assert.deepEqual(readFileSync(path), before)
})


test('higher-priority existing rows are applicable without forcing a city write', async () => {
  const root = resolve(directory, 'higher-priority-block'), path = resolve(root, 'z9/276/173/roads.arrow')
  mkdirSync(resolve(path, '..'), { recursive: true })
  const original = writeRoadsFixture('higher-priority.arrow', [2], {
    origin: [14.43, 50.08], countryCodes: [iso2Code('CZ')], sourceIds: [9004],
  })
  copyFileSync(original, path)
  const before = readFileSync(path)
  const selected = city([record('Road 0')])
  const result = await enrichMunicipalRoads(root, [selected])
  assert.equal(result[0].matched, 0)
  assert.equal(result[0].updated, 0)
  assert.equal(result[0].retracted, 0)
  assert.deepEqual(readFileSync(path), before)
})
