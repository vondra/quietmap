/** Service-tree graph bug classes and real z9 IPC preserve source priority and reruns. */

import assert from 'node:assert/strict'
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync, statSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { resolve } from 'node:path'
import { test } from 'node:test'
import { Bool, Field, makeTable, RecordBatch, Schema, Table, tableToIPC, tableFromIPC, vectorFromArray } from 'apache-arrow'
import { buildGraph, findComponents, flowAccumulate, type ServiceRoad } from './lib/service-tree-flow.js'
import { assignBuildingsGlobally } from './lib/service-tree-buildings.js'
import { iso2Code } from './lib/prepared-grid.js'
import { fleetForIso, WORLD_FLEET } from './lib/country-fleet.js'
import { SOURCE_ID_SERVICE_TREE_HEURISTIC as SELF } from './lib/source-ids.generated.js'
import { enrichServiceTreeSquare, readServiceRoads, splitAADT, SERVICE_TREE_CAP_PER_CLASS } from './enrich-roads-service-tree.js'

function road(a: number, b: number, roadClass = 5, sourceId = 0): ServiceRoad {
  return { startKey: String(a), endKey: String(b), startLat: 50, endLat: 50, midLat: 50, startLon: 14 + a * 0.001,
    endLon: 14 + b * 0.001, midLon: 14 + (a + b) * 0.0005,
    length: Math.abs(b - a) * 71, roadClass, sourceId, tunnel: false, access: 0 }
}

test('tracks do not root; measured locals, tunnels and access exclusions do root the retained graph', () => {
  const roads = [road(0, 1), road(1, 2, 7), road(2, 3, 8)]
  let graph = buildGraph(roads), components = findComponents(graph)
  assert.deepEqual([...graph.eligible], [1, 1, 0])
  assert.equal(components.length, 1); assert.equal(components[0].rootNodes.size, 0)
  assert.deepEqual([...flowAccumulate(components[0], graph.segNodeIds, { get: i => roads[i].length }, new Map(), () => WORLD_FLEET).values()], [0, 0])
  roads[2] = road(2, 3, 5, 10)
  graph = buildGraph(roads); components = findComponents(graph)
  assert.equal(components[0].rootNodes.size, 1)
  const loads = new Map([[0, { dwellings: 10, trips: 0 }], [1, { dwellings: 0, trips: 5 }]])
  const flow = flowAccumulate(components[0], graph.segNodeIds, { get: i => roads[i].length }, loads, () => WORLD_FLEET)
  assert.equal(flow.get(0), 36.800000000000004); assert.equal(flow.get(1), 41.800000000000004)
  const exclusions = [road(0, 1), { ...road(1, 2), tunnel: true }, { ...road(2, 3), access: 2 },
    { ...road(3, 4), access: 3 }, { ...road(4, 5), access: 4 }, road(5, 6, 10)]
  assert.deepEqual([...buildGraph(exclusions).eligible], [1, 0, 0, 1, 0, 0])
})

test('global assignment chooses one component, preserves ties and handles the dateline', () => {
  const roads = [road(0, 1), road(0, 1)]
  const buildings = [{ lat: 50.00001, lon: 14.0005, type: 0, floors: 2, area: 400 }]
  assert.deepEqual([...assignBuildingsGlobally(roads, [1, 0], buildings)], [[1, { dwellings: 10, trips: 0 }]])
  assert.equal(assignBuildingsGlobally(roads, [0], [{ ...buildings[0], lat: 50 + 50.001 / 110540 }]).size, 0)
  assert.equal(assignBuildingsGlobally(roads, [0], [{ ...buildings[0], lat: 50 + 49.999 / 110540 }]).size, 1)
  const seam = { ...road(0, 1), startLon: 179.999, endLon: -179.999, midLon: -180 }
  const same = { ...seam, startLon: -0.001, endLon: 0.001, midLon: 0 }
  assert.deepEqual([...assignBuildingsGlobally([seam], [0], [{ ...buildings[0], lon: -180 }])],
    [...assignBuildingsGlobally([same], [0], [{ ...buildings[0], lon: 0 }])])
})

test('inherited class caps and country splits conserve the one final rounded traffic total', () => {
  assert.equal(SERVICE_TREE_CAP_PER_CLASS[7], 400)
  for (const fleet of [WORLD_FLEET, fleetForIso('TH'), fleetForIso('CZ')]) {
    for (const trips of [0, 7, 20, 333.4, 2000]) {
      const value = splitAADT(Math.min(trips, SERVICE_TREE_CAP_PER_CLASS[7]), fleet)
      assert.equal(value.light + value.medium + value.heavy + value.moto, Math.round(Math.max(20, Math.min(trips, 400))))
      assert.ok(value.light >= 0)
    }
  }
  assert.equal(splitAADT(1000, fleetForIso('TH')).moto, 200)
  assert.equal(fleetForIso('CZ').tripsPerDwelling, 3.4)
})

function grid(lat: number, lon: number): [number, number] {
  return [Math.floor(6378137 * lon * Math.PI / 180 / 0.03732276771704472) + 2 ** 29,
    Math.floor(6378137 * Math.log(Math.tan(Math.PI / 4 + lat * Math.PI / 360)) / 0.03732276771704472) + 2 ** 29]
}
function store(path: string, table: Table, metadata: Map<string, string>) {
  const fields = table.schema.fields.map(field => new Field(field.name, field.type, false, new Map([['note', field.name]])))
  const schema = new Schema(fields, metadata)
  const parts = table.numRows > 1 ? [table.slice(0, 1), table.slice(1)] : [table]
  const stored = new Table(schema, parts.flatMap(part => part.batches.map(batch => new RecordBatch(schema, batch.data))))
  writeFileSync(path, tableToIPC(stored, 'file'))
}
function fixture(directory: string, roads: ServiceRoad[], emptyBuildings = false) {
  mkdirSync(directory, { recursive: true })
  const starts = roads.map(r => grid(r.startLat, r.startLon)), ends = roads.map(r => grid(r.endLat, r.endLon))
  const table = makeTable({
    start_gx: Int32Array.from(starts, r => r[0]), start_gy: Int32Array.from(starts, r => r[1]),
    end_gx: Int32Array.from(ends, r => r[0]), end_gy: Int32Array.from(ends, r => r[1]),
    road_class: Uint8Array.from(roads, r => r.roadClass), source_id: Uint16Array.from(roads, r => r.sourceId),
    access: Uint8Array.from(roads, r => r.access), tunnel: vectorFromArray(roads.map(r => r.tunnel), new Bool()),
    length_m: Float32Array.from(roads, r => r.length), country_iso: Uint16Array.from(roads, () => iso2Code('CZ')),
    aadt_light: Int32Array.from(roads, () => 100), aadt_medium: new Int32Array(roads.length),
    aadt_heavy: new Int32Array(roads.length), aadt_moto: new Int32Array(roads.length),
    speed_taper: Uint8Array.from(roads, () => 41), speed_limit: Uint8Array.from(roads, () => 50),
  } as never) as unknown as Table
  store(resolve(directory, 'roads.arrow'), table, new Map([['grid', 'z30'], ['roads_contract', 'country_baked_v1'], ['qm_batch_bboxes', '[1,2]']]))
  const points = emptyBuildings ? [] : [grid(50.00001, 14.0015)]
  const buildings = makeTable({ centroid_gx: Int32Array.from(points, r => r[0]), centroid_gy: Int32Array.from(points, r => r[1]),
    building_type: new Uint8Array(points.length), floors: Uint8Array.from(points, () => 2), area_m2: Float32Array.from(points, () => 400) })
  store(resolve(directory, 'buildings.arrow'), buildings, new Map([['grid', 'z30'], ['buildings_contract', 'buildings_v3']]))
}

test('real IPC preserves measured roads, all other columns and batches; retraction heals even without eligible roads', async () => {
  const work = mkdtempSync(resolve(tmpdir(), 'service-tree-ipc-'))
  try {
    const roads = [road(0, 1, 5, 10), road(1, 2), { ...road(2, 3, 5, SELF), tunnel: true }]
    fixture(work, roads)
    const path = resolve(work, 'roads.arrow'), before = tableFromIPC(readFileSync(path))
    const result = await enrichServiceTreeSquare(work)
    assert.equal(result.matched, 1); assert.equal(result.retracted, 1)
    const after = tableFromIPC(readFileSync(path))
    assert.deepEqual([...after.getChild('source_id')!], [10, SELF, 0])
    assert.deepEqual([...after.getChild('aadt_light')!], [100, 32, 0])
    assert.deepEqual([...after.getChild('speed_taper')!], [41, 0, 0])
    assert.deepEqual(after.schema.metadata, before.schema.metadata)
    for (const field of before.schema.fields) assert.deepEqual(after.schema.fields.find(f => f.name === field.name), field)
    assert.deepEqual(after.batches.map(b => b.numRows), before.batches.map(b => b.numRows))
    for (const field of before.schema.fields) {
      if (['source_id', 'aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto', 'speed_taper'].includes(field.name)) continue
      assert.deepEqual(after.getChild(field.name)!.toArray(), before.getChild(field.name)!.toArray())
    }
    const bytes = readFileSync(path), stat = statSync(path, { bigint: true })
    assert.equal((await enrichServiceTreeSquare(work)).updated, false)
    assert.deepEqual(readFileSync(path), bytes); assert.equal(statSync(path, { bigint: true }).ino, stat.ino)
    const stale = resolve(work, 'stale')
    fixture(stale, [{ ...road(0, 1, 8, SELF) }], true)
    assert.equal((await enrichServiceTreeSquare(stale)).retracted, 1)
    assert.equal((await enrichServiceTreeSquare(stale)).updated, false)
    rmSync(resolve(work, 'buildings.arrow'))
    await assert.rejects(enrichServiceTreeSquare(work), /missing original OSM/)
    assert.deepEqual(readFileSync(path), bytes)
  } finally { rmSync(work, { recursive: true, force: true }) }
})


test('valid empty buildings produce no new traffic; missing admin bake fails before a write', async () => {
  const work = mkdtempSync(resolve(tmpdir(), 'service-tree-admission-'))
  try {
    fixture(work, [road(0, 1, 5, SELF), road(1, 2, 5, 10), road(2, 3)], true)
    const path = resolve(work, 'roads.arrow'), before = tableFromIPC(readFileSync(path))
    const result = await enrichServiceTreeSquare(work)
    assert.equal(result.matched, 0); assert.equal(result.retracted, 1)
    const table = tableFromIPC(readFileSync(path))
    assert.deepEqual([...table.getChild('source_id')!], [0, 10, 0])
    assert.deepEqual([...table.getChild('aadt_light')!], [0, 100, 100])
    assert.deepEqual([...table.getChild('speed_taper')!], [0, 41, 41])
    for (const field of before.schema.fields) {
      if (['source_id', 'aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto', 'speed_taper'].includes(field.name)) continue
      assert.deepEqual(table.getChild(field.name)!.toArray(), before.getChild(field.name)!.toArray())
    }
    const bytes = readFileSync(path), stat = statSync(path, { bigint: true })
    assert.equal((await enrichServiceTreeSquare(work)).updated, false)
    assert.deepEqual(readFileSync(path), bytes); assert.equal(statSync(path, { bigint: true }).ino, stat.ino)
    store(path, table, new Map([['grid', 'z30']]))
    const unbaked = readFileSync(path)
    await assert.rejects(enrichServiceTreeSquare(work), /country_baked_v1/)
    assert.deepEqual(readFileSync(path), unbaked)
  } finally { rmSync(work, { recursive: true, force: true }) }
})


test('nearby distinct native endpoints do not create a motor exit for a disconnected service road', () => {
  const work = mkdtempSync(resolve(tmpdir(), 'service-tree-native-gap-'))
  try {
    const local = road(0, 1), exit = road(1, 2, 4, 10)
    exit.startLon += 0.000001
    fixture(work, [local, exit])
    const { roads } = readServiceRoads(tableFromIPC(readFileSync(resolve(work, 'roads.arrow'))))
    const graph = buildGraph(roads), components = findComponents(graph)
    assert.equal(components.length, 1)
    assert.equal(components[0].rootNodes.size, 0)
    assert.notEqual(graph.segNodeIds[1], graph.segNodeIds[2])
  } finally { rmSync(work, { recursive: true, force: true }) }
})
