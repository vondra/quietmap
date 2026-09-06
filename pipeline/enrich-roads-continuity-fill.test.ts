/** Native IPC proofs for flow redistribution, stale retraction and final CZ ramps. */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync, statSync, writeFileSync } from 'node:fs'
import { Float32, Int16, Int32, RecordBatch, Schema, Table, Uint8, Uint16, tableFromIPC, tableToIPC, vectorFromArray } from 'apache-arrow'
import { bytes, writeRoadsFixture } from './lib/road-test-fixture.js'
import { withArrowWrite } from './lib/provenance.js'
import { iso2Code } from './lib/prepared-grid.js'
import { writeRoadAadt } from './lib/roads-arrow.js'
import { readPlanningRoads, type PlanningRoad } from './lib/road-planning-input.js'
import { buildContinuityPlan } from './lib/roads-continuity-plan.js'
import { enrichContinuitySquare } from './enrich-roads-continuity-fill.js'
import { enrichTaperSquare } from './enrich-roads-taper.js'
import { generateRoadPlanningDefaults } from './generate-road-planning-defaults.js'

const traffic = (light: number, sourceId: number) => ({ light, medium: 0, heavy: 0, moto: 0, sourceId })

async function chain(name: string, classes: number[], countries = classes.map(() => iso2Code('CZ'))) {
  const path = writeRoadsFixture(name, classes, { sourceIds: classes.map(() => 0), refs: classes.map(() => '0033'),
    speeds: classes.map(() => 0), countryCodes: countries })
  await withArrowWrite(path, table => {
    const columns = Object.fromEntries(table.schema.fields.map(field => [field.name, table.getChild(field.name)!]))
    const startX = Array.from(table.getChild('start_gx')!.toArray()) as number[]
    const startY = Array.from(table.getChild('start_gy')!.toArray()) as number[]
    columns.end_gx = vectorFromArray(startX.map((value, i) => startX[i + 1] ?? value + 1000), new Int32())
    columns.end_gy = vectorFromArray(startY.map((value, i) => startY[i + 1] ?? value + 1000), new Int32())
    columns.segment_idx = vectorFromArray(classes.map(() => 0), new Int16())
    columns.length_m = vectorFromArray(classes.map(() => 60), new Float32())
    for (const name of ['access', 'junction']) columns[name] = vectorFromArray(classes.map(() => 0), new Uint8())
    columns.built_up = vectorFromArray(classes.map(() => 2), new Uint8())
    for (const name of ['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto']) columns[name] = vectorFromArray(classes.map(() => 0), new Int32())
    return new Table(columns)
  })
  const table = tableFromIPC(bytes(path)), halves = [table.slice(0, 1), table.slice(1)]
  const schema = new Schema(table.schema.fields, new Map([...table.schema.metadata, ['fixture', 'original-metadata']]))
  const stored = new Table(schema, halves.flatMap(part => part.batches.map(batch => new RecordBatch(schema, batch.data))))
  writeFileSync(path, tableToIPC(stored, 'file'))
  return path
}

function row(i: number, a: string, b: string, changes: Partial<PlanningRoad> = {}): PlanningRoad {
  return { i, a, b, ref: '0033', cls: 4, src: 0, aadt: [0, 0, 0, 0], osmId: i, segIdx: 0,
    speedTag: 0, builtUp: 2, access: 0, roundabout: false, len: 60, ...changes }
}

test('same-ref flow loses local draw, retains four-class proportions and never seeds from estimates', () => {
  const roads = [row(0, 'a', 'b', { src: 10, aadt: [9000, 500, 400, 100] }), row(1, 'b', 'c'),
    row(2, 'b', 'd', { cls: 5, ref: '', src: 11, aadt: [999999, 0, 0, 0] })]
  const result = buildContinuityPlan(roads)
  assert.equal(result.anchors, 1)
  assert.deepEqual(result.fill.get(1), { light: 8550, medium: 475, heavy: 380, moto: 95, total: 9500, anchor: 0 })
  assert.equal(result.fill.has(2), false)
  roads[0].src = 12
  assert.equal(buildContinuityPlan(roads).fill.size, 0)
  roads[0].src = 10; roads[1].ref = 'OTHER'
  assert.equal(buildContinuityPlan(roads).fill.get(1)?.total, 10000 * 800 / 1300)
})

test('incompatible measured anchors veto the shared fill; agreeing anchors retain the lower vector', () => {
  const roads = [row(0, 'a', 'b', { src: 10, aadt: [10000, 0, 0, 0] }), row(1, 'b', 'c'),
    row(2, 'c', 'd', { src: 10, aadt: [40000, 0, 0, 0] })]
  assert.equal(buildContinuityPlan(roads).conflicts, 1)
  assert.equal(buildContinuityPlan(roads).fill.has(1), false)
  roads[2].aadt = [20000, 0, 0, 0]
  assert.equal(buildContinuityPlan(roads).fill.get(1)?.total, 10000)
})

test('native continuity retains metadata, batches and measurements; retiring all anchors clears only owned output', async () => {
  const path = await chain('continuity.arrow', [4, 4, 4])
  await writeRoadAadt(path, (_row, i) => i === 0 ? traffic(10000, 10) : null)
  const input = tableFromIPC(bytes(path))
  assert.equal((await enrichContinuitySquare(path)).matched, 2)
  let output = tableFromIPC(bytes(path))
  assert.deepEqual([...output.schema.metadata], [...input.schema.metadata])
  assert.deepEqual(output.batches.map(batch => batch.numRows), input.batches.map(batch => batch.numRows))
  for (const field of input.schema.fields.filter(field => !field.name.startsWith('aadt_') && field.name !== 'source_id')) {
    assert.deepEqual(output.getChild(field.name)!.toArray(), input.getChild(field.name)!.toArray())
  }
  const stable = bytes(path), mtime = statSync(path).mtimeMs
  assert.equal((await enrichContinuitySquare(path)).updated, false)
  assert.deepEqual(bytes(path), stable); assert.equal(statSync(path).mtimeMs, mtime)
  await writeRoadAadt(path, () => null, undefined, undefined, { sourceIds: [10], when: () => true })
  const retired = await enrichContinuitySquare(path)
  assert.equal(retired.anchors, 0); assert.equal(retired.retracted, 2)
  output = tableFromIPC(bytes(path))
  assert.deepEqual([...output.getChild('source_id')!.toArray()], [0, 0, 0])
  assert.deepEqual([...output.getChild('aadt_light')!.toArray()], [0, 0, 0])
  assert.equal((await enrichContinuitySquare(path)).updated, false)
})

test('native final taper grades only CZ default rows, preserves tags/measurements and retracts obsolete ramps', async () => {
  const path = await chain('taper.arrow', [4, 4, 4, 4], [iso2Code('CZ'), iso2Code('CZ'), iso2Code('CZ'), iso2Code('AT')])
  await writeRoadAadt(path, (_row, i) => i === 0 ? traffic(10000, 10) : null)
  const input = tableFromIPC(bytes(path))
  assert.equal((await enrichTaperSquare(path)).matched, 2)
  let table = tableFromIPC(bytes(path))
  assert.deepEqual([...table.getChild('source_id')!.toArray()], [10, 9862, 9862, 0])
  assert.deepEqual(table.getChild('speed_limit')!.toArray(), input.getChild('speed_limit')!.toArray())
  assert.deepEqual([...table.schema.metadata], [...input.schema.metadata])
  assert.deepEqual(table.batches.map(batch => batch.numRows), input.batches.map(batch => batch.numRows))
  const stable = bytes(path), mtime = statSync(path).mtimeMs
  assert.equal((await enrichTaperSquare(path)).updated, false)
  assert.deepEqual(bytes(path), stable); assert.equal(statSync(path).mtimeMs, mtime)
  await writeRoadAadt(path, () => null, undefined, undefined, { sourceIds: [10], when: () => true })
  assert.equal((await enrichTaperSquare(path)).retracted, 2)
  table = tableFromIPC(bytes(path))
  assert.deepEqual([...table.getChild('source_id')!.toArray()], [0, 0, 0, 0])
})

test('fresh unstamped native tables may omit all AADT columns; partial or stamped absence fails closed', async () => {
  const path = await chain('no-aadt.arrow', [4, 4]), original = tableFromIPC(bytes(path))
  const strip = (names: string[]) => {
    const table = new Table(Object.fromEntries(original.schema.fields.filter(field => !names.includes(field.name))
      .map(field => [field.name, original.getChild(field.name)!])))
    return new Table(original.schema.select(table.schema.fields.map(field => field.name)), table.batches)
  }
  assert.equal(readPlanningRoads(strip(['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto'])).length, 2)
  assert.throws(() => readPlanningRoads(strip(['aadt_medium'])), /road planning column/)
  const absent = strip(['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto'])
  const stamped = new Table({ ...Object.fromEntries(absent.schema.fields.map(field => [field.name, absent.getChild(field.name)!])),
    source_id: vectorFromArray([10, 0], new Uint16()) })
  const stampedSchema = new Schema(stamped.schema.fields, absent.schema.metadata)
  assert.throws(() => readPlanningRoads(new Table(stampedSchema, stamped.batches.map(batch => new RecordBatch(stampedSchema, batch.data)))), /missing stamped traffic/)
  assert.throws(() => readPlanningRoads(strip(['built_up'])), /built_up/)
})

test('planning defaults remain an exact derivation of the canonical engine tables', () => {
  assert.equal(readFileSync(new URL('./lib/road-planning-defaults.generated.ts', import.meta.url), 'utf8'), generateRoadPlanningDefaults())
})

test('new measured anchors replace older baseline taper through canonical priority on a complete chain rerun', async () => {
  const path = await chain('taper-before-new-anchor.arrow', [4, 4, 4])
  await withArrowWrite(path, table => new Table({
    ...Object.fromEntries(table.schema.fields.map(field => [field.name, table.getChild(field.name)!])),
    speed_limit: vectorFromArray([100, 0, 0], new Uint8()),
  }))
  assert.equal((await enrichTaperSquare(path)).matched, 2)
  assert.deepEqual([...tableFromIPC(bytes(path)).getChild('source_id')!.toArray()], [0, 9862, 9862])
  await writeRoadAadt(path, (_row, i) => i === 0 ? traffic(10000, 10) : null)
  assert.equal((await enrichContinuitySquare(path)).matched, 2)
  const table = tableFromIPC(bytes(path))
  assert.deepEqual([...table.getChild('source_id')!.toArray()], [10, 12, 12])
  assert.deepEqual([...table.getChild('speed_taper')!.toArray()], [0, 0, 0])
  assert.equal((await enrichTaperSquare(path)).updated, false)
  assert.equal((await enrichContinuitySquare(path)).updated, false)
})


test('native endpoints one grid cell apart do not transfer measured traffic or transition ramps', async () => {
  const path = await chain('native-endpoint-gap.arrow', [4, 4])
  await withArrowWrite(path, table => new Table({
    ...Object.fromEntries(table.schema.fields.map(field => [field.name, table.getChild(field.name)!])),
    start_gx: vectorFromArray([...table.getChild('start_gx')!].map((value, index) => Number(value) + index), new Int32()),
  }))
  await writeRoadAadt(path, (_row, index) => index === 0 ? traffic(10000, 10) : null)
  const original = bytes(path)
  assert.equal((await enrichContinuitySquare(path)).matched, 0)
  assert.equal((await enrichTaperSquare(path)).matched, 0)
  assert.deepEqual(bytes(path), original)
})
