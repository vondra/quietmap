/** CZ source admission, pseudo-stop bridging, owned silence and native endpoint identity. */

import assert from 'node:assert/strict'
import { test, after } from 'node:test'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import { enrichCzechRailways } from './enrich-railways-cz.js'
import { readCzpttSource, czpttSequencesToStationPairs } from './lib/railway-cz-source.js'
import { writeRailwaysFixture } from './lib/rail-test-fixture.js'
import { collectZ9RailGraphSegments } from './lib/rail-walk-enrich.js'
import { buildRailGraph } from './lib/rail-graph.js'

const temporary = mkdtempSync(join(tmpdir(), 'rail-cz-'))
after(() => rmSync(temporary, { recursive: true, force: true }))

function source(directory: string, stops = [{ code: '1', name: 'A' }, { code: '2', name: 'B' }]) {
  mkdirSync(directory, { recursive: true })
  writeFileSync(join(directory, 'czptt-train-sequences.json'), JSON.stringify({ sequences: [
    { stops, isFreight: false }, { stops: [...stops].reverse(), isFreight: false },
  ] }))
  writeFileSync(join(directory, 'osm-stations.json'), JSON.stringify({
    A: { name: 'A', lat: 50, lon: 14 }, B: { name: 'B', lat: 50, lon: 14.01 },
  }))
}

test('ordered known stops bridge pseudo-locations once, retain directions and classify freight', () => {
  const coordinates = new Map([['1', { lat: 50, lon: 14 }], ['2', { lat: 50, lon: 14.01 }]])
  const a = { code: '1', name: 'A' }, b = { code: '2', name: 'B' }, pseudo = { code: '9', name: 'Km 67,500' }
  assert.deepEqual(czpttSequencesToStationPairs([
    { stops: [a, pseudo, a, b], isFreight: false },
    { stops: [b, a], isFreight: true }, { stops: [pseudo, a], isFreight: false },
  ], coordinates), [
    { fromLat: 50, fromLon: 14, toLat: 50, toLon: 14.01, pax: 1, frt: 0 },
    { fromLat: 50, fromLon: 14.01, toLat: 50, toLon: 14, pax: 0, frt: 1 },
  ])
})

test('CZ whole source admits before writes; measured, silent and foreign rows converge after re-extract', async () => {
  const directory = join(temporary, 'source'), prepared = join(temporary, 'prepared')
  source(directory)
  const square = join(prepared, 'z9', '275', '173'); mkdirSync(square, { recursive: true })
  const path = join(square, 'railways.arrow')
  const fixture = writeRailwaysFixture('cz-source.arrow', [
    { latitude: 50, longitude: 14, endLatitude: 50, endLongitude: 14.01, country: 'CZ', sourceId: 9863, passenger: 2, freight: 1 },
    { latitude: 50.03, longitude: 14, endLatitude: 50.03, endLongitude: 14.01, country: 'CZ', sourceId: 110, passenger: 70 },
    { latitude: 50.03007, longitude: 14, endLatitude: 50.03007, endLongitude: 14.01, country: 'CZ' },
    { latitude: 50.06, longitude: 14, country: 'DE', sourceId: 9864, passenger: 80 },
    { latitude: 50.07, longitude: 14, country: 'CZ', railType: 1, sourceId: 110, passenger: 90 },
    { latitude: 50.08, longitude: 14, country: 'CZ', sourceId: 9864, passenger: 95 },
    { latitude: 50.09, longitude: 14, country: 'CZ', sourceId: 100, passenger: 85 },
  ], { includeTraffic: true, includeDivisor: true })
  copyFileSync(fixture, path)
  const before = tableFromIPC(readFileSync(path))
  const result = await enrichCzechRailways(directory, prepared)
  assert.equal(result.walk.walkStamped, 1)
  assert.equal(result.walk.silentStamped, 2)
  const actual = tableFromIPC(readFileSync(path))
  assert.deepEqual([...actual.getChild('source_id')!], [110, 9863, 9863, 9864, 0, 9864, 100])
  assert.deepEqual([...actual.getChild('trains_passenger')!], [2, 2, 2, 80, 0, 95, 85])
  assert.deepEqual([...actual.getChild('trains_freight')!], [0, 1, 1, 0, 0, 0, 0])
  assert.deepEqual([...actual.getChild('parallel_divisor')!], [1, 2, 2, 1, 1, 1, 1])
  assert.deepEqual(actual.schema.metadata, before.schema.metadata)
  assert.deepEqual(actual.batches.map(batch => batch.numRows), before.batches.map(batch => batch.numRows))
  for (const field of before.schema.fields) {
    if (['source_id', 'trains_passenger', 'trains_freight', 'parallel_divisor'].includes(field.name)) continue
    assert.deepEqual([...actual.getChild(field.name)!], [...before.getChild(field.name)!], field.name)
  }
  const expected = readFileSync(path)
  await enrichCzechRailways(directory, prepared); assert.deepEqual(readFileSync(path), expected)
  copyFileSync(fixture, path)
  await enrichCzechRailways(directory, prepared); assert.deepEqual(readFileSync(path), expected)
  writeFileSync(join(directory, 'czptt-train-sequences.json'), '{"sequences":[]}')
  await assert.rejects(enrichCzechRailways(directory, prepared), /no train sequences/)
  assert.deepEqual(readFileSync(path), expected)
  source(directory)
  writeFileSync(join(directory, 'osm-stations.json'), '{"A":{"name":"A","lat":null,"lon":14}}')
  assert.throws(() => readCzpttSource(directory), /invalid station coordinates/)
  assert.deepEqual(readFileSync(path), expected)
})

test('actual z30 IPC endpoints less than one metre apart stay distinct graph components', () => {
  const prepared = join(temporary, 'endpoint'), square = 'z9/275/173'
  mkdirSync(join(prepared, square), { recursive: true })
  copyFileSync(writeRailwaysFixture('cz-exact-endpoints.arrow', [
    { latitude: 50, longitude: 14, endLatitude: 50, endLongitude: 14.01, country: 'CZ' },
    { latitude: 50, longitude: 14.010004, endLatitude: 50, endLongitude: 14.02, country: 'CZ' },
  ]), join(prepared, square, 'railways.arrow'))
  const segments = collectZ9RailGraphSegments(prepared, [square])
  assert.equal(segments[0].endLon.toFixed(5), segments[1].startLon.toFixed(5))
  assert.notEqual(segments[0].endKey, segments[1].startKey)
  const graph = buildRailGraph(segments)
  assert.equal(graph.nodeCount, 4)
  assert.notEqual(graph.componentOfNode[graph.edges[0].nodeA], graph.componentOfNode[graph.edges[1].nodeA])
})
