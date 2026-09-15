/** Native IPC proofs for partition-independent road flow and owned-output retraction. */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdirSync, readFileSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import { Float32, Float64, Int16, Int32, RecordBatch, Schema, Table, Uint8, Uint16, Utf8, tableFromIPC, tableToIPC, vectorFromArray } from 'apache-arrow'
import { bytes, writeRoadsFixture } from './lib/road-test-fixture.js'
import { withArrowWrite } from './lib/provenance.js'
import { iso2Code, segmentGeometryReader } from './lib/prepared-grid.js'
import { writeRoadAadt } from './lib/roads-arrow.js'
import { readPlanningRoads, type PlanningRoad } from './lib/road-planning-input.js'
import { roadContinuityComponent, planContinuityComponent, type ContinuityRoad, type Flow } from './lib/roads-continuity-plan.js'
import { enrichContinuityDirectory, writeContinuitySquares } from './enrich-roads-continuity-fill.js'
import { transportTopologyPath } from './lib/transport-topology.js'
import { writeTransportFixture, type FixtureSourceWay, type FixtureSourcePiece } from './lib/transport-test-fixture.js'
import { generateRoadPlanningDefaults } from './generate-road-planning-defaults.js'

const traffic = (light: number, sourceId: number) => ({ light, medium: 0, heavy: 0, moto: 0, sourceId,
  countBasis: 'unknown' as const, observationId: 'fixture-observation', observationSourceId: sourceId, estimatedClasses: 0 })

function writeRoadTopology(prepared: string, squares: string[]): void {
  rmSync(transportTopologyPath(prepared), { force: true })
  const nodes = new Map<string, string>(), ways: FixtureSourceWay[] = [], pieces: FixtureSourcePiece[] = []
  const node = (key: string) => {
    if (!nodes.has(key)) nodes.set(key, String(nodes.size + 1))
    return nodes.get(key)!
  }
  for (const square of squares) {
    const table = tableFromIPC(bytes(resolve(prepared, square, 'roads.arrow'))), geometry = segmentGeometryReader(table)
    for (let i = 0; i < table.numRows; i++) {
      const id = String(table.getChild('osm_id')!.get(i)), endpoints = geometry.endpointKeys(i), row = geometry.row(i)
      ways.push({ id, family: 'roads', nodes: [
        [node(endpoints.startKey), [row.startLat, row.startLon]], [node(endpoints.endKey), [row.endLat, row.endLon]],
      ] })
      pieces.push({ way: id, segment: Number(table.getChild('segment_idx')!.get(i)), square, start: [0, 0], end: [1, 0] })
    }
  }
  writeTransportFixture(prepared, ways, pieces)
}

async function enrichContinuitySquare(path: string) {
  const prepared = resolve(dirname(path), '../../..')
  writeRoadTopology(prepared, ['z9/275/173'])
  return enrichContinuityDirectory(prepared)
}

async function chain(name: string, classes: number[], countries = classes.map(() => iso2Code('CZ'))) {
  const originalPath = writeRoadsFixture(name, classes, { sourceIds: classes.map(() => 0), refs: classes.map(() => '0033'),
    speeds: classes.map(() => 0), countryCodes: countries })
  const path = resolve(dirname(originalPath), name + '-prepared/z9/275/173/roads.arrow')
  mkdirSync(dirname(path), { recursive: true }); renameSync(originalPath, path)
  await withArrowWrite(path, table => {
    const columns = Object.fromEntries(table.schema.fields.map(field => [field.name, table.getChild(field.name)!]))
    const startX = Array.from(table.getChild('start_gx')!.toArray()) as number[]
    const startY = Array.from(table.getChild('start_gy')!.toArray()) as number[]
    columns.end_gx = vectorFromArray(startX.map((value, i) => startX[i + 1] ?? value + 1000), new Int32())
    columns.end_gy = vectorFromArray(startY.map((value, i) => startY[i + 1] ?? value + 1000), new Int32())
    columns.segment_idx = vectorFromArray(classes.map(() => 0), new Int16())
    columns.length_m = vectorFromArray(classes.map(() => 60), new Float32())
    for (const name of ['access', 'junction', 'oneway']) columns[name] = vectorFromArray(classes.map(() => 0), new Uint8())
    columns.built_up = vectorFromArray(classes.map(() => 2), new Uint8())
    columns.traffic_count_basis = vectorFromArray(classes.map(() => 0), new Uint8())
    columns.traffic_observation_source = vectorFromArray(classes.map(() => 0), new Uint16())
    columns.traffic_observation_id = vectorFromArray(classes.map(() => ''), new Utf8())
    columns.traffic_estimated = vectorFromArray(classes.map(() => 15), new Uint8())
    for (const name of ['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto']) columns[name] = vectorFromArray(classes.map(() => 0), new Float64())
    return new Table(columns)
  })
  const table = tableFromIPC(bytes(path)), halves = [table.slice(0, 1), table.slice(1)]
  const schema = new Schema(table.schema.fields, new Map([...table.schema.metadata, ['fixture', 'original-metadata'], ['road_traffic_contract', '0']]))
  const stored = new Table(schema, halves.flatMap(part => part.batches.map(batch => new RecordBatch(schema, batch.data))))
  writeFileSync(path, tableToIPC(stored, 'file'))
  return path
}

function row(i: number, a: string, b: string, changes: Partial<PlanningRoad & ContinuityRoad> = {}): PlanningRoad & ContinuityRoad {
  return { i, a, b, ref: '0033', cls: 4, src: 0, aadt: [0, 0, 0, 0], osmId: i, segIdx: 0,
    speedTag: 0, builtUp: 2, access: 0, roundabout: false, len: 60, direction: 0, countBasis: 'unknown', observationId: 'fixture-observation', observationSourceId: 10, ...changes }
}

function buildContinuityPlan(roads: ContinuityRoad[]) {
  const endpoint = new Map<string, ContinuityRoad[]>()
  for (const road of roads) for (const key of [road.a, road.b]) {
    const incident = endpoint.get(key)
    if (incident) incident.push(road)
    else endpoint.set(key, [road])
  }
  const fill = new Map<number, Flow>(), seen = new Set<number>()
  let anchors = 0, conflicts = 0
  for (const road of roads) {
    if (seen.has(road.i)) continue
    const component = roadContinuityComponent(road, key => endpoint.get(key) ?? [])
    for (const member of component) seen.add(member.i)
    const plan = planContinuityComponent(component)
    for (const [index, flow] of plan.fill) fill.set(index, flow)
    anchors += plan.anchors; conflicts += plan.conflicts
  }
  return { fill, anchors, conflicts }
}

test('unobserved branches stop propagation; pure subdivisions retain even below-default observed flow', () => {
  const roads = [row(0, 'a', 'b', { src: 10, aadt: [90, 5, 4, 1] }), row(1, 'b', 'c'),
    row(2, 'b', 'd', { cls: 5, ref: '', src: 11, aadt: [999999, 0, 0, 0] })]
  assert.equal(buildContinuityPlan(roads).fill.size, 0)
  roads.pop()
  assert.deepEqual(buildContinuityPlan(roads).fill.get(1), { light: 90, medium: 5, heavy: 4, moto: 1, countBasis: 'unknown', observationId: 'fixture-observation', observationSourceId: 10 })
  roads[0].src = 12
  assert.equal(buildContinuityPlan(roads).fill.size, 0)
  roads[0].src = 10; roads[1].direction = 1
  assert.equal(buildContinuityPlan(roads).fill.size, 0)
  roads[0].direction = 2
  assert.equal(buildContinuityPlan(roads).fill.size, 0)
  roads[0].direction = 1
  assert.equal(buildContinuityPlan(roads).fill.get(1)?.light, 90)
  roads[0].aadt = [0, 0, 0, 0]
  assert.deepEqual(buildContinuityPlan(roads).fill.get(1), { light: 0, medium: 0, heavy: 0, moto: 0, countBasis: 'unknown', observationId: 'fixture-observation', observationSourceId: 10 })
})

test('conflicting measured vectors remain unresolved; agreeing observations preserve their exact counts', () => {
  const roads = [row(0, 'a', 'b', { src: 10, aadt: [10000, 0, 0, 0] }), row(1, 'b', 'c'),
    row(2, 'c', 'd', { src: 10, aadt: [20000, 0, 0, 0] })]
  assert.equal(buildContinuityPlan(roads).conflicts, 1)
  assert.equal(buildContinuityPlan(roads).fill.has(1), false)
  roads[2].aadt = [10000, 0, 0, 0]
  assert.equal(buildContinuityPlan(roads).fill.get(1)?.light, 10000)
  roads[2].observationId = 'different-observation'
  assert.equal(buildContinuityPlan(roads).fill.size, 0)
})

test('native continuity retains metadata, batches and measurements; retiring all anchors clears only owned output', async () => {
  const path = await chain('continuity.arrow', [4, 4, 4])
  await writeRoadAadt(path, (_row, i) => i === 0 ? traffic(10000, 10) : null)
  const input = tableFromIPC(bytes(path))
  assert.equal((await enrichContinuitySquare(path)).matched, 2)
  let output = tableFromIPC(bytes(path))
  assert.deepEqual([...output.schema.metadata], [...input.schema.metadata])
  assert.deepEqual(output.batches.map(batch => batch.numRows), input.batches.map(batch => batch.numRows))
  for (const field of input.schema.fields.filter(field => !field.name.startsWith('aadt_') && field.name !== 'source_id' && !field.name.startsWith('traffic_'))) {
    assert.deepEqual(output.getChild(field.name)!.toArray(), input.getChild(field.name)!.toArray())
  }
  const stable = bytes(path), mtime = statSync(path).mtimeMs
  assert.equal((await enrichContinuitySquare(path)).updated, false)
  assert.deepEqual(bytes(path), stable); assert.equal(statSync(path).mtimeMs, mtime)
  await writeRoadAadt(path, () => null, undefined, undefined, { sourceIds: [10], when: () => true })
  await withArrowWrite(path, table => new Table({
    ...Object.fromEntries(table.schema.fields.map(field => [field.name, table.getChild(field.name)!])),
    road_class: vectorFromArray([4, 5, 4], new Uint8()),
  }))
  const retired = await enrichContinuitySquare(path)
  assert.equal(retired.anchors, 0); assert.equal(retired.retracted, 2)
  output = tableFromIPC(bytes(path))
  assert.deepEqual([...output.getChild('source_id')!.toArray()], [0, 0, 0])
  assert.deepEqual([...output.getChild('aadt_light')!.toArray()], [0, 0, 0])
  assert.equal(output.schema.metadata.get('road_traffic_contract'), '0')
  assert.equal((await enrichContinuitySquare(path)).updated, false)
})

test('observed traffic requires complete Float64 categories and observation provenance', async () => {
  const path = await chain('no-aadt.arrow', [4, 4]), original = tableFromIPC(bytes(path))
  const strip = (names: string[]) => {
    const table = new Table(Object.fromEntries(original.schema.fields.filter(field => !names.includes(field.name))
      .map(field => [field.name, original.getChild(field.name)!])))
    return new Table(original.schema.select(table.schema.fields.map(field => field.name)), table.batches)
  }
  assert.equal(readPlanningRoads(strip(['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto'])).length, 2)
  assert.throws(() => readPlanningRoads(strip(['aadt_medium'])), /road planning column/)
  assert.throws(() => readPlanningRoads(strip(['built_up'])), /built_up/)
  assert.throws(() => readPlanningRoads(strip(['traffic_observation_source'])), /observation columns/)
  await writeRoadAadt(path, (_row, i) => i === 0 ? { ...traffic(100, 11), countBasis: 'allocated', observationId: '', estimatedClasses: 15 } : null)
  const allocated = readPlanningRoads(tableFromIPC(bytes(path)))[0]
  assert.equal(allocated.countBasis, 'allocated'); assert.equal(allocated.observationId, '')
  assert.equal((await enrichContinuitySquare(path)).anchors, 0)
  await withArrowWrite(path, table => new Table({
    ...Object.fromEntries(table.schema.fields.map(field => [field.name, table.getChild(field.name)!])),
    aadt_light: vectorFromArray([0, 0], new Int32()),
  }))
  assert.throws(() => readPlanningRoads(tableFromIPC(bytes(path))), /aadt_light/)
})

test('planning defaults remain an exact derivation of the canonical engine tables', () => {
  assert.equal(readFileSync(new URL('./lib/road-planning-defaults.generated.ts', import.meta.url), 'utf8'), generateRoadPlanningDefaults())
})

test('ordered preparation workers preserve cross-owner counts, exact bytes, retraction and missing-owner failures', async t => {
  const originalWorkers = process.env.QM_ROAD_WORKERS
  t.after(() => { if (originalWorkers === undefined) delete process.env.QM_ROAD_WORKERS; else process.env.QM_ROAD_WORKERS = originalWorkers })
  const outputs = new Map<number, { result: Awaited<ReturnType<typeof enrichContinuityDirectory>>; bytes: Buffer[] }>()
  const sourcePath = await chain('partition-source.arrow', [4, 4, 4, 4, 4],
    ['CZ', 'CZ', 'AT', 'AT', 'AT'].map(iso2Code))
  await writeRoadAadt(sourcePath, (_row, index) => index === 0 ?
    { light: 89999, medium: 5000, heavy: 4000, moto: 1001, sourceId: 10,
      countBasis: 'both-directions' as const, observationId: 'cross-country-count', observationSourceId: 10, estimatedClasses: 0 } : null)
  const original = tableFromIPC(bytes(sourcePath))
  const layouts = [
    [['z9/275/173', 0, 5]],
    [['z9/275/173', 0, 1], ['z9/276/173', 1, 3], ['z9/277/173', 3, 5]],
  ] as const
  for (const [number, layout] of layouts.entries()) for (const workers of [1, 3]) {
    process.env.QM_ROAD_WORKERS = String(workers)
    const prepared = resolve(dirname(sourcePath), `../../../../partition-${number}-${workers}`)
    const squares = layout.map(([square]) => square)
    for (const [square, from, to] of layout) {
      const path = resolve(prepared, square, 'roads.arrow')
      mkdirSync(dirname(path), { recursive: true })
      writeFileSync(path, tableToIPC(original.slice(from, to), 'file'))
    }
    writeRoadTopology(prepared, squares)
    if (squares.length > 1) {
      // The anchor-only square must not enter withArrowWrite at all (reading this lock directory fails).
      const lock = resolve(prepared, squares[0], 'roads.arrow.lock')
      mkdirSync(lock); t.after(() => rmSync(lock, { recursive: true, force: true }))
    }
    const result = await enrichContinuityDirectory(prepared)
    assert.equal(result.matched, 4)
    console.log(JSON.stringify({ continuityFixture: number, graphBytes: result.graphBytes }))
    for (const [square, from, to] of layout) {
      const path = resolve(prepared, square, 'roads.arrow'), result = tableFromIPC(bytes(path))
      for (let row = 0; row < result.numRows; row++) {
        assert.deepEqual(['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto'].map(name => result.getChild(name)!.get(row)),
          [89999, 5000, 4000, 1001])
        assert.equal(result.getChild('traffic_count_basis')!.get(row), 2)
        assert.equal(result.getChild('traffic_observation_id')!.get(row), 'cross-country-count')
        assert.equal(result.getChild('traffic_observation_source')!.get(row), 10)
        assert.equal(result.getChild('source_id')!.get(row), from + row === 0 ? 10 : 12)
        assert.equal(result.getChild('traffic_estimated')!.get(row), from + row === 0 ? 0 : 15)
      }
      for (const field of original.schema.fields.filter(field => !field.name.startsWith('aadt_') && field.name !== 'source_id' && !field.name.startsWith('traffic_'))) {
        assert.deepEqual(result.getChild(field.name)!.toArray(), original.slice(from, to).getChild(field.name)!.toArray())
      }
    }
    const stable = squares.map(square => bytes(resolve(prepared, square, 'roads.arrow')))
    const previous = outputs.get(number)
    if (previous) { assert.deepEqual(result, previous.result); assert.deepEqual(stable, previous.bytes) }
    else outputs.set(number, { result, bytes: stable })
    assert.equal((await enrichContinuityDirectory(prepared)).updated, false)
    for (const [index, square] of squares.entries()) assert.deepEqual(bytes(resolve(prepared, square, 'roads.arrow')), stable[index])
    if (squares.length === 1) {
      // Equal snapped coordinates do not connect distinct original OSM nodes, such as separated grades.
      using database = new DatabaseSync(transportTopologyPath(prepared))
      const row = database.prepare('SELECT nodes_json FROM source_ways WHERE osm_id=10001').get()!
      const nodes = JSON.parse(String(row.nodes_json)); nodes[0][0] = '900'
      database.prepare('UPDATE source_ways SET nodes_json=? WHERE osm_id=10001').run(JSON.stringify(nodes))
      const separated = await enrichContinuityDirectory(prepared)
      assert.equal(separated.matched, 0); assert.equal(separated.retracted, 4)
    }
    if (squares.length > 1) {
      {
        using database = new DatabaseSync(transportTopologyPath(prepared))
        database.prepare('DELETE FROM source_pieces WHERE way_id=10002').run()
      }
      await assert.rejects(enrichContinuityDirectory(prepared), /source topology missing or repeated road piece/)
      for (const [index, square] of squares.entries()) assert.deepEqual(bytes(resolve(prepared, square, 'roads.arrow')), stable[index])
      writeRoadTopology(prepared, squares)
      rmSync(resolve(prepared, squares[1], 'roads.arrow'))
      await assert.rejects(enrichContinuityDirectory(prepared), /source topology road owner is missing/)
      assert.deepEqual(bytes(resolve(prepared, squares[0], 'roads.arrow')), stable[0])
    }
  }
})

test('writeback child failures reject without replacing corrupt input or hiding worker startup errors', async () => {
  const path = await chain('writeback-failure.arrow', [4])
  const prepared = resolve(dirname(path), '../../..'), graphPath = resolve(prepared, 'fills.sqlite')
  {
    using database = new DatabaseSync(graphPath)
    database.exec('CREATE TABLE fills (square TEXT, row_index INTEGER, PRIMARY KEY(square,row_index)) WITHOUT ROWID')
  }
  writeFileSync(path, 'invalid Arrow input')
  const before = bytes(path)
  await assert.rejects(writeContinuitySquares(prepared, graphPath, ['z9/275/173']))
  assert.deepEqual(bytes(path), before)
  await assert.rejects(writeContinuitySquares(prepared, graphPath + '.absent', ['z9/275/173']), /worker exited/)
  assert.deepEqual(bytes(path), before)
})

test('compact nonfillable branches and selfloops keep incidence and retract their owned payload', async () => {
  for (const selfloop of [false, true]) {
    const path = await chain(`compact-incidence-${selfloop}.arrow`, [4, 4, 5])
    await withArrowWrite(path, table => {
      const columns = Object.fromEntries(table.schema.fields.map(field => [field.name, table.getChild(field.name)!]))
      for (const axis of ['gx', 'gy']) {
        const starts = [...table.getChild(`start_${axis}`)!.toArray()] as number[]
        const ends = [...table.getChild(`end_${axis}`)!.toArray()] as number[]
        starts[2] = starts[1]
        if (selfloop) ends[2] = starts[1]
        columns[`start_${axis}`] = vectorFromArray(starts, new Int32())
        columns[`end_${axis}`] = vectorFromArray(ends, new Int32())
      }
      return new Table(columns)
    })
    await writeRoadAadt(path, (_row, i) => i === 0 ? traffic(100, 10) : i === 2 ? traffic(99, 12) : null)
    const result = await enrichContinuitySquare(path)
    assert.equal(result.anchors, 1); assert.equal(result.matched, 0); assert.equal(result.retracted, 1)
    const output = tableFromIPC(bytes(path))
    assert.deepEqual([...output.getChild('source_id')!.toArray()], [10, 0, 0])
    assert.deepEqual([...output.getChild('aadt_light')!.toArray()], [100, 0, 0])
  }
})
