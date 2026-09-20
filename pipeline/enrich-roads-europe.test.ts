/** Actual IPC regression for cross-owner, high-latitude city traffic and source priority. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { Int16, Int32, Int64, Uint8, RecordBatch, Schema, Table, tableFromIPC, tableToIPC, vectorFromArray } from 'apache-arrow'
import { encodeQmBlocks, writeRoadsFixture } from './lib/road-test-fixture.js'
import { writeRoadAadt } from './lib/roads-arrow.js'
import { SOURCE_ID_CZ_RSD_SCITANI, SOURCE_ID_CITY_PRAHA_TSK } from './lib/sources.js'
import { segmentGeometryReader } from './lib/prepared-grid.js'
import { parseEuropeanCityTraffic } from './lib/roads-europe-source.js'
import { buildOneHundredthDegreePointGrid, flatDist, nearestCompatiblePointWithin200Metres } from './lib/spatial.js'
import { enrichEuropeanRoads, indexEuropeanTraffic, nearestEuropeanTraffic } from './enrich-roads-europe.js'

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

/** A northbound road piece of the given length centred on a point, optionally east of it. */
function roadPiece(midLat: number, midLon: number, osmId: number | null = null, lengthMetres = 20, offsetEastMetres = 0) {
  const longitude = midLon + offsetEastMetres / (111_320 * Math.cos(midLat * Math.PI / 180))
  const halfLength = lengthMetres / 2 / 110_540
  return { startLat: midLat - halfLength, startLon: longitude, endLat: midLat + halfLength, endLon: longitude,
    midLat, midLon: longitude, osmId, roadClass: 2 }
}

test('the exact 50 metre flat-distance cap rejects the dev1 50-to-51 metre leak', () => {
  const row = { midLat: 50, midLon: 14 }
  for (const [offset, accepted] of [[49.9, true], [50.5, false]] as const) {
    const source = city(traffic(row, offset))
    assert.equal(nearestEuropeanTraffic(roadPiece(50, 14), indexEuropeanTraffic(source.records)) !== null, accepted)
  }
})

test('a road piece along a long line matches far from its middle vertex, the published way wins, and the nearest line wins whatever its direction', () => {
  const line = (osmid: number, offsetEastMetres: number, northward: boolean) => {
    const longitude = 14 + offsetEastMetres / (111_320 * Math.cos(50 * Math.PI / 180))
    const coordinates = [[longitude, 50], [longitude, 50.005], [longitude, 50.01]]
    return { type: 'Feature', properties: { AADT: 1000, raw_oneway: true, osmid },
      geometry: { type: 'LineString', coordinates: northward ? coordinates : coordinates.reverse() } }
  }
  const source = city(line(100, 30, true), line(200, 10, true), line(300, -5, false))
  const [publishedWay, , nearestSouthbound] = source.records
  const index = indexEuropeanTraffic(source.records)
  // 50.001 is 440 m from every middle vertex, yet lies along all three lines.
  assert.equal(nearestEuropeanTraffic(roadPiece(50.001, 14, 100), index), publishedWay)
  assert.equal(nearestEuropeanTraffic(roadPiece(50.001, 14, 999), index), nearestSouthbound)
  assert.equal(nearestEuropeanTraffic(roadPiece(50.001, 14, 999, 20, -61), index), null)
  assert.equal(nearestEuropeanTraffic(roadPiece(50.02, 14, 999), index), null)
  // Proximity alone is not the counted street: a service lane beside it and a street crossing it stay unmatched.
  assert.equal(nearestEuropeanTraffic({ ...roadPiece(50.001, 14, 999), roadClass: 7 }, index), null)
  const crossing = roadPiece(50.001, 14, 999)
  const halfLengthDegrees = 10 / (111_320 * Math.cos(50 * Math.PI / 180))
  assert.equal(nearestEuropeanTraffic({ ...crossing, startLat: 50.001, endLat: 50.001,
    startLon: crossing.midLon - halfLengthDegrees, endLon: crossing.midLon + halfLengthDegrees }, index), null)
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
    const matched = nearestEuropeanTraffic(roadPiece(lat, lon), indexEuropeanTraffic(source.records))
    assert.equal(matched, source.records[1])
    assert.ok(flatDist(lat, lon, matched!.coordinates[0][1], matched!.coordinates[0][0]) < 41)
    const ranked = { latitude: lat, longitude: lon + 190 / (111_320 * Math.cos(lat * Math.PI / 180)), rank: 1 }
    ranked.longitude = ((ranked.longitude + 180) % 360) - 180
    assert.equal(nearestCompatiblePointWithin200Metres(lat, lon,
      buildOneHundredthDegreePointGrid([ranked]), () => true), ranked)
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
  assert.ok(source.records[3].coordinates[0][0] > 0)
  const owner = join(temporary, 'z9/255/173')
  mkdirSync(owner, { recursive: true })
  const path = join(owner, 'roads.arrow')
  copyFileSync(inputPath, path)
  const schema = new Schema(original.schema.fields, new Map([...original.schema.metadata,
    ['test_metadata', 'unchanged'], ['qm_blocks', encodeQmBlocks([[49, -1, 51, 1], [49, -1, 51, 1]])]]))
  const batches = [original.slice(0, 1).batches[0], original.slice(1, 4).batches[0]]
  const beforeTable = new Table(schema, batches.map(batch => new RecordBatch(schema, batch.data)))
  writeFileSync(path, Buffer.from(tableToIPC(beforeTable, 'file')))
  const result = await enrichEuropeanRoads(temporary, [source])
  assert.equal(result.matched, 2)
  const after = tableFromIPC(readFileSync(path))
  assert.deepEqual(after.batches.map(batch => batch.numRows), [1, 3])
  assert.deepEqual(after.schema.metadata, new Map([...beforeTable.schema.metadata, ['road_traffic_contract', '0']]))
  for (const field of beforeTable.schema.fields) {
    if (['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto', 'source_id', 'traffic_count_basis', 'traffic_observation_id', 'traffic_observation_source', 'traffic_estimated'].includes(field.name)) continue
    assert.deepEqual(after.schema.fields.find(candidate => candidate.name === field.name), field)
    assert.deepEqual(after.getChild(field.name)!.toArray(), beforeTable.getChild(field.name)!.toArray())
  }
  for (const index of [2, 3]) {
    assert.deepEqual(['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto', 'source_id']
      .map(name => after.getChild(name)!.get(index)), [830, 20, 100, 50, 10])
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

test('a directional line stamps both one-way carriageways lying along it', async () => {
  const prepared = join(temporary, 'directional-line'), path = join(prepared, 'z9/255/173/roads.arrow')
  let table = tableFromIPC(readFileSync(writeRoadsFixture('eu-directional-line.arrow', [3, 3], { sourceIds: [0, 0] })))
  const y = Number(table.getChild('start_gy')!.get(0)), x = 2 ** 29 - 400
  for (const [name, values] of Object.entries({ start_gx: [x, x], end_gx: [x, x], start_gy: [y, y], end_gy: [y + 400, y + 400] })) {
    table = table.setChild(name, vectorFromArray(values, new Int32()))
  }
  table = table.setChild('osm_id', vectorFromArray([100n, 200n], new Int64()))
  const columns = Object.fromEntries(table.schema.fields.map(field => [field.name, table.getChild(field.name)!]))
  columns.oneway = vectorFromArray([1, 2], new Uint8())
  const shape = new Table(columns)
  mkdirSync(join(path, '..'), { recursive: true })
  writeFileSync(path, tableToIPC(new Table(new Schema(shape.schema.fields, table.schema.metadata), shape.batches), 'file'))
  const row = segmentGeometryReader(table).row(0)
  const line = city({ type: 'Feature', properties: { AADT: 1000, raw_oneway: true },
    geometry: { type: 'LineString', coordinates: [[row.startLon, row.startLat], [row.endLon, row.endLat]] } })
  assert.equal(line.records[0].countBasis, 'directional')
  assert.equal((await enrichEuropeanRoads(prepared, [line])).matched, 2)
  assert.deepEqual([...tableFromIPC(readFileSync(path)).getChild('source_id')!.toArray()], [10, 10])
})

test('one directional observation chooses one current way across owners; two-way counts remain shared', async () => {
  for (const scenario of [
    { name: 'missing-id', sourceOsmId: null, directional: true, expected: [10, 10, 0] },
    { name: 'stale-id', sourceOsmId: 999, directional: true, expected: [10, 10, 0] },
    { name: 'retained-id', sourceOsmId: 200, directional: true, expected: [0, 0, 10] },
    { name: 'both-directions', sourceOsmId: null, directional: false, expected: [10, 10, 10] },
  ]) {
    const prepared = join(temporary, scenario.name)
    const input = writeRoadsFixture(`eu-${scenario.name}.arrow`, [3, 3, 3], { sourceIds: [0, 0, 0] })
    let table = tableFromIPC(readFileSync(input))
    const y = Number(table.getChild('start_gy')!.get(0))
    for (const [name, values] of Object.entries({
      start_gx: [2 ** 29 - 400, 2 ** 29 - 400, 2 ** 29 + 400],
      end_gx: [2 ** 29 - 400, 2 ** 29 - 400, 2 ** 29 + 400],
      start_gy: [y, y + 400, y + 800], end_gy: [y + 400, y + 800, y],
    })) table = table.setChild(name, vectorFromArray(values, new Int32()))
    table = table.setChild('osm_id', vectorFromArray([100n, 100n, 200n], new Int64()))
    const columns = Object.fromEntries(table.schema.fields.map(field => [field.name, table.getChild(field.name)!]))
    columns.segment_idx = vectorFromArray([0, 1, 0], new Int16())
    columns.oneway = vectorFromArray([1, 1, 1], new Uint8())
    const shape = new Table(columns)
    table = new Table(new Schema(shape.schema.fields, table.schema.metadata), shape.batches)
    const paths = ['z9/255/173', 'z9/256/173'].map(square => join(prepared, square, 'roads.arrow'))
    for (const [index, path] of paths.entries()) {
      mkdirSync(join(path, '..'), { recursive: true })
      writeFileSync(path, tableToIPC(index === 0 ? table.slice(0, 2) : table.slice(2), 'file'))
    }
    const feature = traffic(segmentGeometryReader(table).row(0))
    const observation = city({ ...feature, properties: { ...feature.properties,
      AADT: 10000, TR_AADT: 1000, '2W_AADT': 500, raw_oneway: scenario.directional,
      ...(scenario.sourceOsmId === null ? {} : { osmid: scenario.sourceOsmId }),
    } })
    if (scenario.name === 'missing-id') {
      // A prior snapshot stamped both ways under a different observation identity.
      const previous = city({ ...feature, properties: { ...feature.properties, raw_oneway: false } })
      assert.equal((await enrichEuropeanRoads(prepared, [previous])).matched, 3)
    }
    assert.equal((await enrichEuropeanRoads(prepared, [observation])).squares, 2)
    const output = paths.flatMap(path => {
      const result = tableFromIPC(readFileSync(path))
      return Array.from({ length: result.numRows }, (_, i) => ({
        source: Number(result.getChild('source_id')!.get(i)),
        id: result.getChild('traffic_observation_id')!.get(i),
        basis: result.getChild('traffic_count_basis')!.get(i),
        counts: ['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto'].map(name => result.getChild(name)!.get(i)),
      }))
    })
    assert.deepEqual(output.map(row => row.source), scenario.expected, scenario.name)
    for (const row of output.filter(row => row.source === 10)) {
      assert.deepEqual(row.counts, [8300, 200, 1000, 500], scenario.name)
      assert.equal(row.id, observation.records[0].observationId)
      assert.equal(row.basis, scenario.directional ? 1 : 4)
    }
    const before = paths.map(path => readFileSync(path))
    assert.equal((await enrichEuropeanRoads(prepared, [observation])).squaresUpdated, 0)
    paths.forEach((path, index) => assert.deepEqual(readFileSync(path), before[index]))
  }
})


test('a directional point stays on its road after a higher-priority count and retracts an earlier displaced stamp', async () => {
  for (const sourceId of [SOURCE_ID_CZ_RSD_SCITANI, SOURCE_ID_CITY_PRAHA_TSK]) {
    const prepared = join(temporary, `priority-rerun-${sourceId}`), path = join(prepared, 'z9/275/173/roads.arrow')
    let table = tableFromIPC(readFileSync(writeRoadsFixture(`eu-priority-${sourceId}.arrow`, [3, 3], { sourceIds: [0, 0] })))
    for (const name of ['start_gx', 'end_gx', 'start_gy', 'end_gy']) {
      const first = Number(table.getChild(name)!.get(0))
      table = table.setChild(name, vectorFromArray([first, first + (name.endsWith('gx') ? 400 : 0)], new Int32()))
    }
    mkdirSync(join(path, '..'), { recursive: true })
    writeFileSync(path, tableToIPC(table, 'file'))
    const observation = city(traffic(segmentGeometryReader(table).row(0)))
    const sources = () => [...tableFromIPC(readFileSync(path)).getChild('source_id')!.toArray()]
    await enrichEuropeanRoads(prepared, [observation])
    assert.deepEqual(sources(), [10, 0])
    const originalWay = Number(table.getChild('osm_id')!.get(0))
    await writeRoadAadt(path, row => row.osmId === originalWay ? {
      sourceId, countBasis: 'street-cross-section', observationId: 'measured-original-road',
      light: 5000, medium: 100, heavy: 200, moto: 50,
    } : null)
    const measured = readFileSync(path)
    assert.equal((await enrichEuropeanRoads(prepared, [observation])).squaresUpdated, 0)
    assert.deepEqual(readFileSync(path), measured)

    // The old rerun moved this same point to the neighbouring road after the original got a better source.
    await writeRoadAadt(path, row => row.osmId !== originalWay ? observation.records[0] : null)
    assert.deepEqual(sources(), [sourceId, 10])
    await enrichEuropeanRoads(prepared, [observation])
    assert.deepEqual(sources(), [sourceId, 0])
    const healed = tableFromIPC(readFileSync(path)), before = tableFromIPC(measured)
    for (const field of before.schema.fields) assert.deepEqual(healed.getChild(field.name)!.get(0), before.getChild(field.name)!.get(0))
    for (const name of ['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto', 'traffic_observation_source']) {
      assert.equal(healed.getChild(name)!.get(1), 0)
    }
    assert.equal(healed.getChild('traffic_observation_id')!.get(1), '')
    const stable = readFileSync(path)
    assert.equal((await enrichEuropeanRoads(prepared, [observation])).squaresUpdated, 0)
    assert.deepEqual(readFileSync(path), stable)
  }
})
