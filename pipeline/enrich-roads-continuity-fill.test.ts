/** Native IPC proofs for partition-independent road flow and owned-output retraction. */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdirSync, readFileSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { Float32, Float64, Int16, Int32, Int64, RecordBatch, Schema, Table, Uint8, Uint16, Utf8, tableFromIPC, tableToIPC, vectorFromArray } from 'apache-arrow'
import { bytes, writeRoadsFixture } from './lib/road-test-fixture.js'
import { withArrowWrite } from './lib/provenance.js'
import { gridToLonLat, iso2Code, lonLatToGrid, segmentGeometryReader, z9AxesOfGridCell } from './lib/prepared-grid.js'
import { writeRoadAadt } from './lib/roads-arrow.js'
import { readPlanningRoads, type PlanningRoad } from './lib/road-planning-input.js'
import { roadContinuityComponent, planContinuityComponent, type ContinuityRoad, type Flow } from './lib/roads-continuity-plan.js'
import { planSquareContinuity } from './lib/roads-continuity-square.js'
import { SquarePieces } from './lib/transport-topology.js'
import { enrichContinuityDirectory, writeContinuitySquares } from './enrich-roads-continuity-fill.js'
import { writeTransportFixture, type FixtureSourceWay, type FixtureSourcePiece } from './lib/transport-test-fixture.js'
import { generateRoadPlanningDefaults } from './generate-road-planning-defaults.js'

const traffic = (light: number, sourceId: number) => ({ light, medium: 0, heavy: 0, moto: 0, sourceId,
  countBasis: 'unknown' as const, observationId: 'fixture-observation', observationSourceId: sourceId, estimatedClasses: 0 })

function writeRoadTopology(prepared: string, squares: string[],
  alter?: (ways: FixtureSourceWay[], pieces: FixtureSourcePiece[]) => FixtureSourcePiece[]): void {
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
  writeTransportFixture(prepared, ways, alter ? alter(ways, pieces) : pieces)
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
  return { i, a, b, ref: '0033', name: '', cls: 4, src: 0, aadt: [0, 0, 0, 0], osmId: i, segIdx: 0,
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

test('pieces without a ref continue under one name; another name, a missing name or another ref ends the chain', () => {
  const chainFills = (first: Partial<ContinuityRoad>, second: Partial<ContinuityRoad>) => buildContinuityPlan([
    row(0, 'a', 'b', { src: 10, aadt: [90, 5, 4, 1], ...first }), row(1, 'b', 'c', second)]).fill.has(1)
  const inner = { ref: '', name: 'Boulevard Périphérique Intérieur' }
  assert.equal(chainFills(inner, inner), true)
  assert.equal(chainFills(inner, { ref: '', name: 'Boulevard Périphérique Extérieur' }), false)
  assert.equal(chainFills({ ref: '', name: '' }, { ref: '', name: '' }), false)
  assert.equal(chainFills(inner, { ...inner, ref: 'A1' }), false)
  assert.equal(chainFills({ ...inner, ref: 'A1' }, { ...inner, ref: 'A3' }), false)
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

test('parallel square workers preserve cross-owner counts, exact bytes, retraction and missing-owner failures', async t => {
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
      writeRoadTopology(prepared, squares, (ways, pieces) => {
        ways.find(way => way.id === '10001')!.nodes[0][0] = '900'
        return pieces
      })
      const separated = await enrichContinuityDirectory(prepared)
      assert.equal(separated.matched, 0); assert.equal(separated.retracted, 4)
    }
    if (squares.length > 1) {
      writeRoadTopology(prepared, squares, (_ways, pieces) => pieces.filter(piece => piece.way !== '10002'))
      await assert.rejects(enrichContinuityDirectory(prepared), /source topology missing or repeated road piece/)
      for (const [index, square] of squares.entries()) assert.deepEqual(bytes(resolve(prepared, square, 'roads.arrow')), stable[index])
    }
  }
})

test('a writeback child failure rejects without replacing corrupt input', async () => {
  const path = await chain('writeback-failure.arrow', [4])
  writeFileSync(path, 'invalid Arrow input')
  const before = bytes(path)
  await assert.rejects(writeContinuitySquares(resolve(dirname(path), '../../..'), new Map([['z9/275/173', []]])))
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

type RegionNode = [id: string, lon: number, lat: number]
interface RegionPiece { way: number; segment?: number; from: RegionNode; to: RegionNode; cut?: [from: number, to: number]
  cls?: number; ref?: string; name?: string; oneway?: number; anchorLight?: number }

/** Roads and source pieces stored where the extractor stores them: in the square of each piece's midpoint. */
async function writeRegion(name: string, regionPieces: RegionPiece[]) {
  const prepared = resolve(dirname(writeRoadsFixture(`${name}.unused`, [])), `${name}-prepared`)
  const placed = regionPieces.map(piece => {
    const [from, to] = piece.cut ?? [0, 1]
    const at = (fraction: number) => lonLatToGrid(piece.from[1] + (piece.to[1] - piece.from[1]) * fraction, piece.from[2] + (piece.to[2] - piece.from[2]) * fraction)
    const [x, y] = z9AxesOfGridCell(...at((from + to) / 2))
    return { piece, square: `z9/${x}/${y}`, start: at(from), end: at(to) }
  })
  const squares = [...new Set(placed.map(member => member.square))].sort()
  for (const square of squares) {
    const members = placed.filter(member => member.square === square), constant = (value: number) => members.map(() => value)
    const path = resolve(prepared, square, 'roads.arrow')
    mkdirSync(dirname(path), { recursive: true })
    renameSync(writeRoadsFixture(`${name}-${square.replaceAll('/', '-')}.arrow`, members.map(member => member.piece.cls ?? 4), { sourceIds: constant(0), speeds: constant(0) }), path)
    await withArrowWrite(path, table => new Table({
      ...Object.fromEntries(table.schema.fields.map(field => [field.name, table.getChild(field.name)!])),
      osm_id: vectorFromArray(members.map(member => BigInt(member.piece.way)), new Int64()),
      segment_idx: vectorFromArray(members.map(member => member.piece.segment ?? 0), new Int16()),
      ref: vectorFromArray(members.map(member => member.piece.ref ?? 'R1'), new Utf8()),
      name: vectorFromArray(members.map(member => member.piece.name ?? ''), new Utf8()),
      ...Object.fromEntries((['start', 'end'] as const).flatMap(side => [0, 1].map(axis =>
        [`${side}_g${'xy'[axis]}`, vectorFromArray(members.map(member => member[side][axis]), new Int32())]))),
      length_m: vectorFromArray(constant(60), new Float32()),
      access: vectorFromArray(constant(0), new Uint8()), junction: vectorFromArray(constant(0), new Uint8()),
      oneway: vectorFromArray(members.map(member => member.piece.oneway ?? 0), new Uint8()), built_up: vectorFromArray(constant(2), new Uint8()),
      ...Object.fromEntries(['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto'].map(column => [column, vectorFromArray(constant(0), new Float64())])),
    }))
    await writeRoadAadt(path, (_row, index) => members[index].piece.anchorLight === undefined ? null :
      { ...traffic(members[index].piece.anchorLight!, 10), observationId: `count-${members[index].piece.anchorLight}` })
  }
  const ways = new Map<number, FixtureSourceWay>()
  for (const { piece } of placed) ways.set(piece.way, { id: String(piece.way), family: 'roads',
    nodes: [[piece.from[0], [piece.from[2], piece.from[1]]], [piece.to[0], [piece.to[2], piece.to[1]]]] })
  writeTransportFixture(prepared, [...ways.values()], placed.map(({ piece, square }) => {
    const [from, to] = piece.cut ?? [0, 1]
    return { way: String(piece.way), segment: piece.segment ?? 0, square, start: [0, from] as [number, number], end: to === 1 ? [1, 0] as [number, number] : [0, to] as [number, number] }
  }))
  return { prepared, squares }
}

test('chains crossing square borders fill exactly as one undivided walk; a chain inside one square never reaches the parent', async t => {
  const originalWorkers = process.env.QM_ROAD_WORKERS
  t.after(() => { if (originalWorkers === undefined) delete process.env.QM_ROAD_WORKERS; else process.env.QM_ROAD_WORKERS = originalWorkers })
  process.env.QM_ROAD_WORKERS = '2'
  // The corner shared by z9/275/173, z9/276/173, z9/275/174 and z9/276/174.
  const corner = gridToLonLat(276 << 21, (511 - 173) << 21)
  let nextWay = 1
  const line = (label: string, lat: number, lons: number[], pieces: Array<Partial<RegionPiece>>): RegionPiece[] =>
    pieces.map((piece, index) => ({ way: nextWay++, from: [`${label}${index}`, corner.lon + lons[index], lat], to: [`${label}${index + 1}`, corner.lon + lons[index + 1], lat], ...piece }))
  const crossing = [-0.0020, -0.0010, 0.0015, 0.0025, 0.0035], north = (step: number) => corner.lat + 0.01 * step
  const diagonalWay = nextWay++, thirds = [0, 1 / 3, 2 / 3, 1]
  const region: RegionPiece[] = [
    ...line('straight', north(1), crossing, [{ anchorLight: 100 }, {}, {}, {}]),
    ...line('agreeing', north(2), crossing, [{ anchorLight: 200 }, {}, {}, { anchorLight: 200 }]),
    ...line('conflicting', north(3), crossing, [{ anchorLight: 300 }, {}, {}, { anchorLight: 301 }]),
    ...line('named', north(4), crossing, [{ anchorLight: 400, ref: '', name: 'Ring' }, { ref: '', name: 'Ring' }, { ref: '', name: 'Other' }, {}]),
    ...line('oneway', north(5), crossing, [{ anchorLight: 500, oneway: 1 }, { oneway: 1 }, { oneway: 2 }, { oneway: 2 }]),
    ...line('branched', north(6), crossing, [{ anchorLight: 600 }, {}, {}, {}]),
    // A side arm stored west of the border ends on the node where the branched line crosses into the eastern square.
    { way: nextWay++, from: ['arm', corner.lon - 0.0005, north(6) + 0.0002], to: ['branched1', corner.lon - 0.0010, north(6)], cls: 6, ref: '' },
    ...line('inside', north(7), [0.0100, 0.0110, 0.0120, 0.0130], [{}, { anchorLight: 700 }, {}]),
    // One long hop cut at fractions crosses three squares diagonally through the corner.
    ...thirds.slice(1).map((to, segment): RegionPiece => ({ way: diagonalWay, segment, cut: [thirds[segment], to], anchorLight: segment === 0 ? 800 : undefined,
      from: ['diagonal0', corner.lon - 0.0020, corner.lat + 0.0028], to: ['diagonal1', corner.lon + 0.0040, corner.lat - 0.0014] })),
  ]
  const { prepared, squares } = await writeRegion('border-region', region)
  assert.deepEqual(squares, ['z9/275/173', 'z9/276/173', 'z9/276/174'])

  const stored = squares.flatMap(square => {
    const table = tableFromIPC(bytes(resolve(prepared, square, 'roads.arrow'))), pieces = SquarePieces.read(prepared, square, 'roads')
    return readPlanningRoads(table).map(road => {
      const { startKey, endKey } = pieces.identity(pieces.row(String(road.osmId), road.segIdx))
      return { ...road, square, row: road.i, a: startKey, b: endKey, direction: Number(table.getChild('oneway')!.get(road.i)) }
    })
  }).map((road, i) => ({ ...road, i }))
  const undivided = buildContinuityPlan(stored)
  assert.deepEqual([undivided.anchors, undivided.conflicts, undivided.fill.size], [10, 1, 11])

  const inside = planSquareContinuity(prepared, 'z9/276/173', [])
  assert.deepEqual(inside.fills.map(fill => [[...fill.rows].length, fill.flow.light]), [[2, 700]])
  assert.equal(inside.anchors, 1)

  const result = await enrichContinuityDirectory(prepared)
  assert.deepEqual([result.rows, result.anchors, result.conflicts, result.matched], [stored.length, undivided.anchors, undivided.conflicts, undivided.fill.size])
  for (const square of squares) {
    const table = tableFromIPC(bytes(resolve(prepared, square, 'roads.arrow')))
    for (const road of stored.filter(road => road.square === square)) {
      const flow = undivided.fill.get(road.i), label = `${square} way ${road.osmId}:${road.segIdx}`
      assert.equal(table.getChild('source_id')!.get(road.row), flow ? 12 : road.src, label)
      assert.equal(table.getChild('aadt_light')!.get(road.row), flow ? flow.light : road.aadt[0], label)
      assert.equal(table.getChild('traffic_observation_id')!.get(road.row), flow ? flow.observationId : road.observationId, label)
    }
  }
  assert.equal((await enrichContinuityDirectory(prepared)).updated, false)
})
