/**
 * Guard rails for the shared wind-registry enrichment: a registry value may only
 * fill a missing spec, the searched cell block must follow from the radius
 * rather than a fixed 3x3 ring, and the hex is patched through the shared
 * atomic Arrow write.
 *
 * Run: `cd pipeline && npx tsx --test lib/wind-registry-match.test.ts`
 */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { promises as fs } from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import {
  Field,
  Float32,
  Float64,
  RecordBatch,
  Schema,
  Table,
  Uint8,
  makeTable,
  tableFromIPC,
  tableToIPC,
  vectorFromArray,
} from 'apache-arrow'
import { haversineM } from './spatial.js'
import {
  buildRegistryGrid,
  fillMissingTurbineSpecs,
  findNearestRegistryRecord,
  writeTurbineSpecs,
} from './wind-registry-match.js'

test('a registry zero never erases a spec the row already carries', () => {
  // Swedish node 10695841749 lost its measured 45 kW to a register record with
  // no power, and the engine then used its 2000 kW default (+7 dB).
  const hub = new Float32Array([80, 0])
  const power = new Float32Array([0, 45])

  assert.equal(fillMissingTurbineSpecs(hub, power, 0, 0, 1500), true)
  assert.deepEqual([hub[0], power[0]], [80, 1500])

  assert.equal(fillMissingTurbineSpecs(hub, power, 1, 90, 0), true)
  assert.deepEqual([hub[1], power[1]], [90, 45])
})

/** One hex: a complete turbine, a turbine missing both specs, and a row that is
 *  not a turbine at all — under a schema that carries metadata, which the
 *  writers' old bare `writeFileSync` would have dropped. */
async function writeHexFixture(arrowPath: string): Promise<void> {
  const schema = new Schema(
    [
      new Field('source_type', new Uint8(), true),
      new Field('centroid_lat', new Float64(), true),
      new Field('centroid_lon', new Float64(), true),
      new Field('hub_height', new Float32(), false),
      new Field('rated_power_kw', new Float32(), false),
    ],
    new Map([['test_contract', 'v1']]),
  )
  const bare = makeTable({
    source_type: vectorFromArray([10, 10, 1], new Uint8()),
    centroid_lat: vectorFromArray([55, 55.5, 56], new Float64()),
    centroid_lon: vectorFromArray([12, 12.5, 13], new Float64()),
    hub_height: vectorFromArray([80, 0, 0], new Float32()),
    rated_power_kw: vectorFromArray([45, 0, 0], new Float32()),
  })
  const table = new Table(schema, bare.batches.map((b) => new RecordBatch(schema, b.data)))
  await fs.writeFile(arrowPath, Buffer.from(tableToIPC(table, 'file')))
}

test('a hex where nothing fills keeps its exact bytes and its mtime', async () => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'wind-arrow-'))
  const arrowPath = path.join(dir, 'industrial.arrow')
  try {
    await writeHexFixture(arrowPath)
    const before = await fs.readFile(arrowPath)
    const mtimeBefore = (await fs.stat(arrowPath)).mtimeMs

    // The registry answers for the row that already carries both specs only.
    const result = await writeTurbineSpecs(arrowPath, (lat) =>
      lat === 55 ? { hubHeightM: 100, ratedPowerKw: 2000 } : null)

    assert.deepEqual(result, { turbineRows: 2, matched: 1, filled: 0 })
    assert.ok(before.equals(await fs.readFile(arrowPath)))
    // Bytes alone would still pass if the write path serialized and compared:
    // the contract is that a no-op hex is never opened for writing at all, and
    // the chain's own ledgers read mtime.
    assert.equal((await fs.stat(arrowPath)).mtimeMs, mtimeBefore)
  } finally {
    await fs.rm(dir, { recursive: true })
  }
})

test('a filled hex keeps its schema metadata and every value the registry does not carry', async () => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'wind-arrow-'))
  const arrowPath = path.join(dir, 'industrial.arrow')
  try {
    await writeHexFixture(arrowPath)

    const result = await writeTurbineSpecs(arrowPath, () => ({ hubHeightM: 0, ratedPowerKw: 2300 }))
    assert.deepEqual(result, { turbineRows: 2, matched: 2, filled: 1 })

    const reread = tableFromIPC(await fs.readFile(arrowPath))
    // The bare-makeTable rebuild inside the callback drops metadata; the shared
    // write path puts it back — a raw writeFileSync here would lose it.
    assert.equal(reread.schema.metadata.get('test_contract'), 'v1')
    assert.deepEqual(Array.from(reread.getChild('hub_height')!.toArray() as Float32Array), [80, 0, 0])
    assert.deepEqual(Array.from(reread.getChild('rated_power_kw')!.toArray() as Float32Array), [45, 2300, 0])
  } finally {
    await fs.rm(dir, { recursive: true })
  }
})

test('NaN counts as missing — these Arrow columns carry no null bitmap', () => {
  const hub = new Float32Array([NaN])
  const power = new Float32Array([NaN])
  assert.equal(fillMissingTurbineSpecs(hub, power, 0, 90, 2300), true)
  assert.deepEqual([hub[0], power[0]], [90, 2300])
})

test('a record two grid cells east is found inside a 500 m radius at 70 deg north', () => {
  // 0.01 deg of longitude is 380 m at 70 deg, so a fixed 3x3 ring could not
  // reach this pair; the cells are 1000 and 1002.
  const grid = buildRegistryGrid([{ lat: 70, lon: 10.021 }])
  assert.equal(Math.round(haversineM(70, 10.009, 70, 10.021)), 456)
  assert.deepEqual(findNearestRegistryRecord(grid, 70, 10.009, 500), { lat: 70, lon: 10.021 })
})

test('the nearest record wins and anything beyond the radius is null', () => {
  const near = { lat: 55.001, lon: 12 }
  const far = { lat: 55.0015, lon: 12 }
  const grid = buildRegistryGrid([far, near])
  assert.equal(findNearestRegistryRecord(grid, 55, 12, 200), near)
  assert.equal(findNearestRegistryRecord(grid, 55, 12, 50), null)
})
