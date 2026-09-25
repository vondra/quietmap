/** Accumulate building-generated local-road traffic per z9 owner before speed taper. */

import { existsSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { DataType, tableFromIPC, type Table } from 'apache-arrow'
import { withArrowWrite } from './lib/provenance.js'
import { applyRoadAadt, type RoadAadt } from './lib/roads-arrow.js'
import {bakedRoadCountryReader, segmentGeometryReader} from './lib/prepared-grid.js'
import { SOURCE_ID_SERVICE_TREE_HEURISTIC } from './lib/source-ids.generated.js'
import { fleetForIso, LOCAL_MEDIUM_SHARE, LOCAL_HEAVY_SHARE, type CountryFleet } from './lib/country-fleet.js'
import { buildGraph, findComponents, serviceStreetDemands, type ServiceRoad } from './lib/service-tree-flow.js'
import { assignBuildingsGlobally, readServiceBuildings } from './lib/service-tree-buildings.js'
import { localStreetAadt } from './lib/local-street-demand.js'
import { runSquareSteps } from './lib/square-pool.js'

export function splitAADT(trips: number, fleet: CountryFleet): RoadAadt {
  const total = Math.round(trips)
  const medium = Math.round(total * LOCAL_MEDIUM_SHARE), heavy = Math.round(total * LOCAL_HEAVY_SHARE)
  const moto = Math.round(total * fleet.motoTrafficShare)
  return { light: total - medium - heavy - moto, medium, heavy, moto, sourceId: SOURCE_ID_SERVICE_TREE_HEURISTIC,
    countBasis: 'allocated', observationId: '' }
}

export function readServiceRoads(table: Table): { roads: ServiceRoad[]; fleets: CountryFleet[]; unknownCountryRows: number } {
  const geometry = segmentGeometryReader(table), countries = bakedRoadCountryReader(table)
  for (const [name, bits] of [['road_class', 8], ['source_id', 16], ['access', 8], ['built_up', 8], ['lanes', 8]] as const) {
    const vector = table.getChild(name)
    if (!vector || !DataType.isInt(vector.type) || vector.type.isSigned || vector.type.bitWidth !== bits || vector.nullCount) {
      throw new Error(`invalid service-tree road column ${name}`)
    }
  }
  const names = table.getChild('name'), osmIds = table.getChild('osm_id')
  if (!names || !osmIds || !DataType.isInt(osmIds.type) || osmIds.type.bitWidth !== 64 ||
      !osmIds.type.isSigned || osmIds.nullCount) throw new Error('invalid service-tree name/osm_id columns')
  const tunnel = table.getChild('tunnel'), length = table.getChild('length_m')
  if (!tunnel || !DataType.isBool(tunnel.type) || tunnel.nullCount || !length || !DataType.isFloat(length.type) || length.nullCount) {
    throw new Error('invalid service-tree tunnel/length_m columns')
  }
  // One flat array per column: `getChild` and `get` on a 1024-batch table cost more than the whole graph walk.
  const roadClasses = table.getChild('road_class')!.toArray() as Uint8Array, lengths = length.toArray() as Float32Array
  const sourceIds = table.getChild('source_id')!.toArray() as Uint16Array, accesses = table.getChild('access')!.toArray() as Uint8Array
  const builtUp = table.getChild('built_up')!.toArray() as Uint8Array, lanes = table.getChild('lanes')!.toArray() as Uint8Array
  const ways = osmIds.toArray() as BigInt64Array, streetNames = Array.from(names) as (string | null)[]
  const tunnels = Array.from(tunnel) as boolean[], endpoints = geometry.tableLocalEndpointNumbers()
  let unknownCountryRows = 0
  const fleets: CountryFleet[] = []
  const roads = Array.from({ length: table.numRows }, (_, index) => {
    const code = countries.codeAt(index), iso = code === 0 ? undefined : String.fromCharCode(code & 255, code >> 8)
    if (iso !== undefined && !/^[A-Z]{2}$/.test(iso)) throw new Error(`invalid baked country at road ${index}`)
    if (iso === undefined) unknownCountryRows++
    fleets.push(fleetForIso(iso))
    const roadClass = roadClasses[index], metres = lengths[index]
    if ((streetNames[index] !== null && typeof streetNames[index] !== 'string') || roadClass > 12 || builtUp[index] > 2 || !Number.isFinite(metres) || metres < 0) throw new Error(`invalid service-tree road ${index}`)
    const { startLat, startLon, endLat, endLon } = geometry.row(index)
    return { startLat, startLon, endLat, endLon, startNode: endpoints.start[index], endNode: endpoints.end[index],
      name: streetNames[index] ?? '', osmId: ways[index], builtUp: builtUp[index],
      roadClass, length: metres, sourceId: sourceIds[index], access: accesses[index], tunnel: tunnels[index],
      lanes: lanes[index] }
  })
  return { roads, fleets, unknownCountryRows }
}

export async function enrichServiceTreeSquare(directory: string) {
  const roadsPath = resolve(directory, 'roads.arrow'), structuresPath = resolve(directory, 'structures.arrow')
  let counts = { rows: 0, matched: 0, retracted: 0, updated: false, unknownCountryRows: 0 }
  await withArrowWrite(roadsPath, table => {
    const { roads, fleets, unknownCountryRows } = readServiceRoads(table)
    const buildings = existsSync(structuresPath)
      ? readServiceBuildings(tableFromIPC(readFileSync(structuresPath))) : []
    const graph = buildGraph(roads), components = findComponents(graph)
    const eligible: number[] = []
    for (const component of components) for (const index of component.segments) eligible.push(index)
    const loads = assignBuildingsGlobally(roads, eligible, buildings), aadt = new Map<number, RoadAadt>()
    const { rowTrips, streets } = serviceStreetDemands(roads, graph, components, loads, fleets)
    for (const [index, street] of streets) {
      const road = roads[index]
      // Class 7 keeps the historical per-row 20..400/day rule and empty-building retraction.
      if (road.roadClass === 7 && !buildings.length) continue
      const trips = road.roadClass === 7 ? Math.max(20, Math.min(rowTrips[index], 400))
        : localStreetAadt(road.roadClass, road.builtUp, street)
      aadt.set(index, splitAADT(trips, fleets[index]))
    }
    const applied = applyRoadAadt(table, roadsPath, (_row, index) => aadt.get(index) ?? null,
      undefined, undefined, { sourceIds: [SOURCE_ID_SERVICE_TREE_HEURISTIC], when: (_row, index) => !aadt.has(index) })
    counts = { ...counts, ...applied.result, unknownCountryRows }
    return applied.table
  })
  return counts
}

async function main(): Promise<void> {
  await runSquareSteps('usage: enrich-roads-service-tree.ts --prepared-dir PREPARED_YEAR_DIR', directory => enrichServiceTreeSquare(directory))
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => { console.error(error); process.exitCode = 1 })
}
