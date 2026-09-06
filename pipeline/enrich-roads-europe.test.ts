/** Actual IPC regression for cross-owner, high-latitude city traffic and source priority. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { Int32, RecordBatch, Schema, Table, tableFromIPC, tableToIPC, vectorFromArray } from 'apache-arrow'
import { writeRoadsFixture } from './lib/road-test-fixture.js'
import { segmentGeometryReader } from './lib/prepared-grid.js'
import { parseEuropeanCityTraffic } from './lib/roads-europe-source.js'
import { buildOneHundredthDegreePointGrid, flatDist, nearestCompatiblePointWithin200Metres } from './lib/spatial.js'
import { enrichEuropeanRoads, nearestEuropeanTraffic } from './enrich-roads-europe.js'

const temporary = mkdtempSync(join(tmpdir(), 'eu-traffic-ipc-'))
after(() => rmSync(temporary, { recursive: true, force: true }))

function traffic(point: { midLat: number; midLon: number }, offsetNorthMetres = 0, offsetEastMetres = 0) {
  return { type: 'Feature', properties: { AADT: 1000, TR_AADT: 100, '2W_AADT': 50, raw_oneway: true },
    geometry: { type: 'Point', coordinates: [
      point.midLon + offsetEastMetres / (111_320 * Math.cos(point.midLat * Math.PI / 180)),
      point.midLat + offsetNorthMetres / 110_540,
    ] } }
}

function city(...features: unknown[]) {
  return parseEuropeanCityTraffic('fixture', 'raw.geojson',
    Buffer.from(JSON.stringify({ type: 'FeatureCollection', features })))
}

test('the exact 50 metre flat-distance cap rejects the dev1 50-to-51 metre leak', () => {
  const row = { midLat: 50, midLon: 14 }
  for (const [offset, accepted] of [[49.9, true], [50.5, false]] as const) {
    const source = city(traffic(row, offset))
    const grid = buildOneHundredthDegreePointGrid(source.records)
    assert.equal(nearestEuropeanTraffic(50, 14, grid) !== null, accepted)
  }
})

test('high-latitude and dateline candidates survive the shared index and nearest wins', () => {
  for (const [lat, lon] of [[72, 14], [89, 14], [0, 179.9999]]) {
    const row = { midLat: lat, midLon: lon }
    const far = traffic(row, 0, 49)
    const close = traffic(row, 0, 40)
    for (const feature of [far, close]) {
      feature.geometry.coordinates[0] = ((feature.geometry.coordinates[0] + 180) % 360) - 180
    }
    const source = city(far, close)
    const matched = nearestEuropeanTraffic(lat, lon, buildOneHundredthDegreePointGrid(source.records))
    assert.equal(matched, source.records[1])
    assert.ok(flatDist(lat, lon, matched!.latitude, matched!.longitude) < 41)
    const ranked = { latitude: lat, longitude: lon + 190 / (111_320 * Math.cos(lat * Math.PI / 180)), rank: 1 }
    ranked.longitude = ((ranked.longitude + 180) % 360) - 180
    assert.equal(nearestCompatiblePointWithin200Metres(lat, lon, 1, 1,
      buildOneHundredthDegreePointGrid([ranked])), ranked)
  }
})

test('whole road rows across a z9 boundary receive four-class totals without changing other columns or batches', async () => {
  const inputPath = writeRoadsFixture('eu-owner-crossing.arrow', [3, 3, 5, 3], {
    sourceIds: [0, 20, 11, 0], speeds: [0, 90, 30, 0],
  })
  let original = tableFromIPC(readFileSync(inputPath))
  // Translate the existing z30 fixture so all midpoint owners are just west of x=256.
  const offset = 2 ** 29 - Math.round((original.getChild('start_gx')!.get(3) +
    original.getChild('end_gx')!.get(3)) / 2) - 600
  for (const name of ['start_gx', 'end_gx']) {
    original = original.setChild(name, vectorFromArray(
      [...original.getChild(name)!.toArray()].map(value => Number(value) + offset), new Int32()))
  }
  const geometry = segmentGeometryReader(original)
  assert.ok([...Array(4).keys()].every(index => geometry.row(index).midLon < 0))
  const source = city(traffic(geometry.row(0), 50.5), traffic(geometry.row(1)),
    traffic(geometry.row(2)), traffic(geometry.row(3), 0, 40))
  assert.ok(source.records[3].longitude > 0)
  const owner = join(temporary, 'z9/255/173')
  mkdirSync(owner, { recursive: true })
  const path = join(owner, 'roads.arrow')
  copyFileSync(inputPath, path)
  const schema = new Schema(original.schema.fields, new Map([...original.schema.metadata,
    ['test_metadata', 'unchanged'], ['qm_batch_bboxes', '[[49,-1,51,1],[49,-1,51,1]]']]))
  const batches = [original.slice(0, 1).batches[0], original.slice(1, 4).batches[0]]
  const beforeTable = new Table(schema, batches.map(batch => new RecordBatch(schema, batch.data)))
  writeFileSync(path, Buffer.from(tableToIPC(beforeTable, 'file')))
  const result = await enrichEuropeanRoads(temporary, [source])
  assert.equal(result.matched, 2)
  const after = tableFromIPC(readFileSync(path))
  assert.deepEqual(after.batches.map(batch => batch.numRows), [1, 3])
  assert.deepEqual(after.schema.metadata, beforeTable.schema.metadata)
  for (const field of beforeTable.schema.fields) {
    if (['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto', 'source_id'].includes(field.name)) continue
    assert.deepEqual(after.schema.fields.find(candidate => candidate.name === field.name), field)
    assert.deepEqual(after.getChild(field.name)!.toArray(), beforeTable.getChild(field.name)!.toArray())
  }
  for (const index of [2, 3]) {
    assert.deepEqual(['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto', 'source_id']
      .map(name => after.getChild(name)!.get(index)), [1660, 40, 200, 100, 10])
  }
  for (const index of [0, 1]) {
    for (const field of beforeTable.schema.fields) {
      assert.deepEqual(after.getChild(field.name)!.get(index), beforeTable.getChild(field.name)!.get(index))
    }
  }
  const beforeRerun = readFileSync(path)
  const stat = statSync(path)
  assert.equal((await enrichEuropeanRoads(temporary, [source])).squaresUpdated, 0)
  assert.deepEqual(readFileSync(path), beforeRerun)
  assert.equal(statSync(path).ino, stat.ino)
  assert.equal(statSync(path).mtimeMs, stat.mtimeMs)
})
