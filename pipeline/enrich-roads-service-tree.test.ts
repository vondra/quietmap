/** Service-tree graph bug classes and real z9 IPC preserve source priority and reruns. */

import assert from 'node:assert/strict'
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync, statSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { resolve } from 'node:path'
import { test } from 'node:test'
import { Bool, Field, Float32, Int32, Int64, Uint8, makeTable, RecordBatch, Schema, Table, Utf8, tableToIPC, tableFromIPC, vectorFromArray } from 'apache-arrow'
import { buildGraph, findComponents, flowAccumulate, serviceStreetDemands, type ServiceRoad } from './lib/service-tree-flow.js'
import { localStreetAadt, LOCAL_STREET_PARAMETERS as PARAMETERS } from './lib/local-street-demand.js'
import { assignBuildingsGlobally, readServiceBuildings } from './lib/service-tree-buildings.js'
import { iso2Code } from './lib/prepared-grid.js'
import { encodeQmBlocks } from './lib/road-test-fixture.js'
import { fleetForIso, WORLD_FLEET } from './lib/country-fleet.js'
import { SOURCE_ID_SERVICE_TREE_HEURISTIC as SELF } from './lib/source-ids.generated.js'
import { enrichServiceTreeSquare, readServiceRoads, splitAADT } from './enrich-roads-service-tree.js'

function road(a: number, b: number, roadClass = 5, sourceId = 0): ServiceRoad {
  return { startNode: a, endNode: b, startLat: 50, endLat: 50, startLon: 14 + a * 0.001, endLon: 14 + b * 0.001,
    name: '', osmId: BigInt(a * 100 + b), builtUp: 2, length: Math.abs(b - a) * 71, roadClass, sourceId, tunnel: false, access: 0, lanes: 0 }
}

test('tracks do not root; measured locals, tunnels and access exclusions do root the retained graph', () => {
  const roads = [road(0, 1), road(1, 2, 7), road(2, 3, 8)]
  let graph = buildGraph(roads), components = findComponents(graph)
  assert.deepEqual([...graph.eligible], [1, 1, 0])
  assert.equal(components.length, 1); assert.equal(components[0].rootNodes.size, 0)
  assert.deepEqual([...flowAccumulate(components[0], graph.segNodeIds, { get: i => roads[i].length }, new Map(), () => WORLD_FLEET).trips.values()], [0, 0])
  roads[2] = road(2, 3, 5, 10)
  graph = buildGraph(roads); components = findComponents(graph)
  assert.equal(components[0].rootNodes.size, 1)
  const loads = new Map([[0, { dwellings: 10, trips: 0 }], [1, { dwellings: 0, trips: 5 }]])
  const flow = flowAccumulate(components[0], graph.segNodeIds, { get: i => roads[i].length }, loads, () => WORLD_FLEET)
  assert.equal(flow.trips.get(0), 36.800000000000004); assert.equal(flow.trips.get(1), 41.800000000000004)
  const exclusions = [road(0, 1), { ...road(1, 2), tunnel: true }, { ...road(2, 3), access: 2 },
    { ...road(3, 4), access: 3 }, { ...road(4, 5), access: 4 }, road(5, 6, 10)]
  assert.deepEqual([...buildGraph(exclusions).eligible], [1, 0, 0, 1, 0, 0])
})

test('global assignment chooses one component, preserves ties and handles the dateline', () => {
  const roads = [road(0, 1), road(0, 1)]
  const buildings = [{ lat: 50.00001, lon: 14.0005, type: 0, storeys: 2, area: 400 }]
  assert.deepEqual([...assignBuildingsGlobally(roads, [1, 0], buildings)], [[1, { dwellings: 10, trips: 0 }]])
  assert.equal(assignBuildingsGlobally(roads, [0], [{ ...buildings[0], lat: 50 + 50.001 / 110540 }]).size, 0)
  assert.equal(assignBuildingsGlobally(roads, [0], [{ ...buildings[0], lat: 50 + 49.999 / 110540 }]).size, 1)
  const seam = { ...road(0, 1), startLon: 179.999, endLon: -179.999, midLon: -180 }
  const same = { ...seam, startLon: -0.001, endLon: 0.001, midLon: 0 }
  assert.deepEqual([...assignBuildingsGlobally([seam], [0], [{ ...buildings[0], lon: -180 }])],
    [...assignBuildingsGlobally([same], [0], [{ ...buildings[0], lon: 0 }])])
})

test('country splits conserve the one final rounded traffic total without local floors or caps', () => {
  for (const fleet of [WORLD_FLEET, fleetForIso('TH'), fleetForIso('CZ')]) {
    for (const trips of [0, 7, 20, 333.4, 2000]) {
      const value = splitAADT(trips, fleet)
      assert.equal(value.light + value.medium + value.heavy + value.moto, Math.round(trips))
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
function fixture(directory: string, roads: ServiceRoad[], emptyBuildings = false, storeys = 2) {
  mkdirSync(directory, { recursive: true })
  const starts = roads.map(r => grid(r.startLat, r.startLon)), ends = roads.map(r => grid(r.endLat, r.endLon))
  const table = makeTable({
    start_gx: Int32Array.from(starts, r => r[0]), start_gy: Int32Array.from(starts, r => r[1]),
    end_gx: Int32Array.from(ends, r => r[0]), end_gy: Int32Array.from(ends, r => r[1]),
    osm_id: BigInt64Array.from(roads, r => r.osmId), built_up: Uint8Array.from(roads, r => r.builtUp),
    name: vectorFromArray(roads.map(r => r.name), new Utf8()),
    road_class: Uint8Array.from(roads, r => r.roadClass), source_id: Uint16Array.from(roads, r => r.sourceId),
    access: Uint8Array.from(roads, r => r.access), lanes: Uint8Array.from(roads, r => r.lanes),
    tunnel: vectorFromArray(roads.map(r => r.tunnel), new Bool()),
    length_m: Float32Array.from(roads, r => r.length), country_iso: Uint16Array.from(roads, () => iso2Code('CZ')),
    aadt_light: Float64Array.from(roads, () => 100), aadt_medium: new Float64Array(roads.length),
    aadt_heavy: new Float64Array(roads.length), aadt_moto: new Float64Array(roads.length),
    traffic_count_basis: Uint8Array.from(roads, r => r.sourceId === SELF ? 3 : 0),
    traffic_observation_id: vectorFromArray(roads.map((r, i) => r.sourceId && r.sourceId !== SELF ? `fixture:${i}` : ''), new Utf8()),
    traffic_observation_source: Uint16Array.from(roads, r => r.sourceId),
    traffic_estimated: Uint8Array.from(roads, () => 15),
    speed_taper: Uint8Array.from(roads, () => 41), speed_limit: Uint8Array.from(roads, () => 50),
  } as never) as unknown as Table
  store(resolve(directory, 'roads.arrow'), table, new Map([['grid', 'z30'], ['roads_contract', 'country_baked_v1'], ['road_traffic_contract', '0'], ['qm_blocks', encodeQmBlocks([[50, 14, 50.01, 14.01]])]]))
  // The structures_v5 producer contract (scripts/structures/structure_contract.py): OSM emission
  // rows carry demand storeys; an Overture-only footprint and a wall carry no demand.
  const points = emptyBuildings ? [] : [grid(50.00001, 14.0015)]
  const [overture, wall] = [grid(50.00001, 14.0005), grid(50.00001, 14.0025)]
  const rows = [...points.map(point => ({ kind: 0, osmId: 7n, type: 0, storeys, centroid: overture, emission: point })),
    { kind: 0, osmId: null, type: null, storeys: 9, centroid: overture, emission: null },
    { kind: 1, osmId: 8n, type: null, storeys: null, centroid: wall, emission: null }]
  const structures = makeTable({
    kind: Uint8Array.from(rows, r => r.kind), centroid_gx: Int32Array.from(rows, r => r.centroid[0]),
    centroid_gy: Int32Array.from(rows, r => r.centroid[1]),
    osm_id: vectorFromArray(rows.map(r => r.osmId), new Int64()),
    building_type: vectorFromArray(rows.map(r => r.type), new Uint8()),
    storeys: vectorFromArray(rows.map(r => r.storeys), new Uint8()),
    emission_centroid_gx: vectorFromArray(rows.map(r => r.emission?.[0] ?? null), new Int32()),
    emission_centroid_gy: vectorFromArray(rows.map(r => r.emission?.[1] ?? null), new Int32()),
    area_m2: vectorFromArray(rows.map(r => r.kind === 0 ? 400 : null), new Float32()),
  } as never) as unknown as Table
  store(resolve(directory, 'structures.arrow'), structures, new Map([['grid', 'z30'], ['structures_contract', 'structures_v5']]))
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
    assert.deepEqual([...after.getChild('aadt_light')!], [100, splitAADT(PARAMETERS.residentialUrban * PARAMETERS.throughFactor + PARAMETERS.demandScale * 34, fleetForIso('CZ')).light, 0])
    assert.deepEqual([...after.getChild('speed_taper')!], [41, 0, 0])
    assert.deepEqual(after.schema.metadata, before.schema.metadata)
    for (const field of before.schema.fields) assert.deepEqual(after.schema.fields.find(f => f.name === field.name), field)
    assert.deepEqual(after.batches.map(b => b.numRows), before.batches.map(b => b.numRows))
    for (const field of before.schema.fields) {
      if (['source_id', 'aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto', 'speed_taper', 'traffic_count_basis', 'traffic_observation_id', 'traffic_observation_source', 'traffic_estimated'].includes(field.name)) continue
      assert.deepEqual(after.getChild(field.name)!.toArray(), before.getChild(field.name)!.toArray())
    }
    const bytes = readFileSync(path), stat = statSync(path, { bigint: true })
    assert.equal((await enrichServiceTreeSquare(work)).updated, false)
    assert.deepEqual(readFileSync(path), bytes); assert.equal(statSync(path, { bigint: true }).ino, stat.ino)
    const stale = resolve(work, 'stale')
    fixture(stale, [{ ...road(0, 1, 8, SELF) }], true)
    assert.equal((await enrichServiceTreeSquare(stale)).retracted, 1)
    assert.equal((await enrichServiceTreeSquare(stale)).updated, false)
    const structuresPath = resolve(work, 'structures.arrow'), structures = tableFromIPC(readFileSync(structuresPath))
    // Floors before the storey fill (structures_v4) are not the demand truth.
    store(structuresPath, structures, new Map([['grid', 'z30'], ['structures_contract', 'structures_v4']]))
    await assert.rejects(enrichServiceTreeSquare(work), /structures_v5/)
    rmSync(structuresPath)
    assert.equal((await enrichServiceTreeSquare(work)).retracted, 0)
    assert.deepEqual([...tableFromIPC(readFileSync(path)).getChild('source_id')!], [10, SELF, 0])
    assert.equal((await enrichServiceTreeSquare(work)).updated, false)
  } finally { rmSync(work, { recursive: true, force: true }) }
})


test('empty buildings keep local background and retract service rows; missing country bake fails before write', async () => {
  const work = mkdtempSync(resolve(tmpdir(), 'service-tree-admission-'))
  try {
    fixture(work, [road(0, 1, 7, SELF), road(1, 2, 5, 10), road(2, 3)], true)
    const path = resolve(work, 'roads.arrow'), before = tableFromIPC(readFileSync(path))
    const result = await enrichServiceTreeSquare(work)
    assert.equal(result.matched, 1); assert.equal(result.retracted, 1)
    const table = tableFromIPC(readFileSync(path))
    assert.deepEqual([...table.getChild('source_id')!], [0, 10, SELF])
    assert.deepEqual([...table.getChild('aadt_light')!], [0, 100, splitAADT(PARAMETERS.residentialUrban, fleetForIso('CZ')).light])
    assert.deepEqual([...table.getChild('speed_taper')!], [0, 41, 0])
    for (const field of before.schema.fields) {
      if (['source_id', 'aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto', 'speed_taper', 'traffic_count_basis', 'traffic_observation_id', 'traffic_observation_source', 'traffic_estimated'].includes(field.name)) continue
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


test('a through connector without frontage gets background times c; named pieces share maximum demand', () => {
  const roads = [road(-1, 0, 4), road(0, 1), road(1, 2), road(2, 3), road(3, 4, 4)]
  for (const i of [1, 2, 3]) roads[i].name = 'Connector'
  const graph = buildGraph(roads), components = findComponents(graph), fleets = roads.map(() => WORLD_FLEET)
  let result = serviceStreetDemands(roads, graph, components, new Map(), fleets)
  for (const i of [1, 2, 3]) {
    assert.equal(result.streets.get(i)!.through, true)
    assert.equal(localStreetAadt(5, 2, result.streets.get(i)!), PARAMETERS.residentialUrban * PARAMETERS.throughFactor)
  }
  result = serviceStreetDemands(roads, graph, components, new Map([[1, { dwellings: 0, trips: 3000 }]]), fleets)
  assert.equal(new Set([...result.streets.values()].map(s => localStreetAadt(5, 2, s))).size, 1)
  assert.ok(localStreetAadt(5, 2, result.streets.get(2)!) > 1200)
})

test('a rural through connector keeps the rural background without the urban premium', () => {
  const roads = [road(-1, 0, 4), road(0, 1), road(1, 2), road(2, 3), road(3, 4, 4)]
  for (const i of [1, 2, 3]) { roads[i].name = 'Farm track'; roads[i].builtUp = 1 }
  const graph = buildGraph(roads), components = findComponents(graph)
  const result = serviceStreetDemands(roads, graph, components, new Map(), roads.map(() => WORLD_FLEET))
  assert.equal(result.streets.get(2)!.through, true)
  assert.equal(localStreetAadt(5, 1, result.streets.get(2)!), PARAMETERS.rural)
})

test('a single-track street keeps only a share of the background; mixed or unknown lanes keep it all', () => {
  const track = [road(-1, 0, 4), road(0, 1), road(1, 2), road(2, 3, 4)]
  for (const i of [1, 2]) { track[i].name = 'Lane'; track[i].lanes = 1 }
  const graph = buildGraph(track), components = findComponents(graph)
  const single = serviceStreetDemands(track, graph, components, new Map(), track.map(() => WORLD_FLEET))
  assert.equal(single.streets.get(1)!.singleTrack, true)
  assert.equal(localStreetAadt(5, 2, single.streets.get(1)!),
    PARAMETERS.residentialUrban * PARAMETERS.throughFactor * PARAMETERS.singleTrackFactor)
  track[2].lanes = 2
  const mixedGraph = buildGraph(track)
  const mixed = serviceStreetDemands(track, mixedGraph, findComponents(mixedGraph), new Map(), track.map(() => WORLD_FLEET))
  assert.equal(mixed.streets.get(1)!.singleTrack, false)
  track[1].lanes = 0; track[2].lanes = 0
  const unknownGraph = buildGraph(track)
  const unknown = serviceStreetDemands(track, unknownGraph, findComponents(unknownGraph), new Map(), track.map(() => WORLD_FLEET))
  assert.equal(unknown.streets.get(1)!.singleTrack, false)
})

test('an access branch has T plus kG and never inherits another street exit boundary', () => {
  const roads = [road(-1, 0, 4), road(0, 1), road(1, 2), road(2, 3, 4), road(0, 4)]
  const graph = buildGraph(roads), components = findComponents(graph)
  const result = serviceStreetDemands(roads, graph, components, new Map([[4, { dwellings: 0, trips: 500 }]]), roads.map(() => WORLD_FLEET))
  assert.deepEqual(result.streets.get(4), { trips: 500, through: false, singleTrack: false })
  assert.equal(localStreetAadt(5, 2, result.streets.get(4)!), PARAMETERS.residentialUrban + PARAMETERS.demandScale * 500)
})

test('same names in different components stay separate; unnamed pieces of one way share demand', () => {
  const roads = [road(0, 1), road(2, 3), road(4, 5), road(6, 7)]
  roads[0].name = roads[1].name = 'Common name'
  roads[2].osmId = roads[3].osmId = 123n
  const graph = buildGraph(roads), components = findComponents(graph)
  const result = serviceStreetDemands(roads, graph, components, new Map([[0, { dwellings: 0, trips: 50 }], [2, { dwellings: 0, trips: 75 }]]), roads.map(() => WORLD_FLEET))
  assert.equal(result.streets.get(0)!.trips, 50); assert.equal(result.streets.get(1)!.trips, 0)
  assert.equal(result.streets.get(2)!.trips, 75); assert.equal(result.streets.get(3)!.trips, 75)
})

test('filled structure storeys feed demand; service traffic retains its historical row rule', async () => {
  const work = mkdtempSync(resolve(tmpdir(), 'service-storeys-'))
  try {
    fixture(work, [road(0, 1, 4, 10), road(1, 2, 7), road(2, 3, 7)])
    const buildings = readServiceBuildings(tableFromIPC(readFileSync(resolve(work, 'structures.arrow'))))
    assert.equal(buildings.length, 1); assert.equal(buildings[0].storeys, 2)
    await enrichServiceTreeSquare(work)
    const table = tableFromIPC(readFileSync(resolve(work, 'roads.arrow')))
    const totals = [0, 1, 2].map(i => ['light', 'medium', 'heavy', 'moto'].reduce((sum, c) => sum + Number(table.getChild(`aadt_${c}`)!.get(i)), 0))
    assert.deepEqual(totals, [100, 34, 20])
    fixture(work, [road(0, 1, 4, 10), road(1, 2, 7)], false, 100)
    await enrichServiceTreeSquare(work)
    const capped = tableFromIPC(readFileSync(resolve(work, 'roads.arrow')))
    assert.equal(['light', 'medium', 'heavy', 'moto'].reduce((sum, c) => sum + Number(capped.getChild(`aadt_${c}`)!.get(1)), 0), 400)
  } finally { rmSync(work, { recursive: true, force: true }) }
})
