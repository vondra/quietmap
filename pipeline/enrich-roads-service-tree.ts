/** Accumulate building-generated local-road traffic per z9 owner before speed taper. */

import { existsSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'
import { DataType, tableFromIPC, type Table } from 'apache-arrow'
import { withArrowWrite } from './lib/provenance.js'
import { applyRoadAadt, type RoadAadt } from './lib/roads-arrow.js'
import { bakedRoadCountryReader, listPreparedSquares, segmentGeometryReader } from './lib/prepared-grid.js'
import { SOURCE_ID_SERVICE_TREE_HEURISTIC } from './lib/source-ids.generated.js'
import { fleetForIso, LOCAL_MEDIUM_SHARE, LOCAL_HEAVY_SHARE, type CountryFleet } from './lib/country-fleet.js'
import { buildGraph, findComponents, flowAccumulate, type ServiceRoad } from './lib/service-tree-flow.js'
import { assignBuildingsGlobally, readServiceBuildings } from './lib/service-tree-buildings.js'

// Historical empirical local-road caps; class 7's 400/day protects apartment
// driveways from runaway routed flow. Tracks and highway links are not eligible.
export const SERVICE_TREE_CAP_PER_CLASS: Readonly<Record<number, number>> = { 5: 1200, 6: 250, 7: 400, 9: 2000 }

export function splitAADT(trips: number, fleet: CountryFleet): RoadAadt {
  // Round only after float accumulation. The historical 20/day floor represents
  // the quietest cul-de-sac; medium/heavy local shares have no national signal.
  const total = Math.round(Math.max(trips, 20))
  const medium = Math.round(total * LOCAL_MEDIUM_SHARE), heavy = Math.round(total * LOCAL_HEAVY_SHARE)
  const moto = Math.round(total * fleet.motoTrafficShare)
  return { light: total - medium - heavy - moto, medium, heavy, moto, sourceId: SOURCE_ID_SERVICE_TREE_HEURISTIC }
}

export function readServiceRoads(table: Table): { roads: ServiceRoad[]; fleets: CountryFleet[]; unknownCountryRows: number } {
  const geometry = segmentGeometryReader(table), countries = bakedRoadCountryReader(table)
  for (const [name, bits] of [['road_class', 8], ['source_id', 16], ['access', 8]] as const) {
    const vector = table.getChild(name)
    if (!vector || !DataType.isInt(vector.type) || vector.type.isSigned || vector.type.bitWidth !== bits || vector.nullCount) {
      throw new Error(`invalid service-tree road column ${name}`)
    }
  }
  const tunnel = table.getChild('tunnel'), length = table.getChild('length_m')
  if (!tunnel || !DataType.isBool(tunnel.type) || tunnel.nullCount || !length || !DataType.isFloat(length.type) || length.nullCount) {
    throw new Error('invalid service-tree tunnel/length_m columns')
  }
  let unknownCountryRows = 0
  const fleets: CountryFleet[] = []
  const roads = Array.from({ length: table.numRows }, (_, index) => {
    const code = countries.codeAt(index), iso = code === 0 ? undefined : String.fromCharCode(code & 255, code >> 8)
    if (iso !== undefined && !/^[A-Z]{2}$/.test(iso)) throw new Error(`invalid baked country at road ${index}`)
    if (iso === undefined) unknownCountryRows++
    fleets.push(fleetForIso(iso))
    const roadClass = table.getChild('road_class')!.get(index) as number
    const metres = length.get(index) as number
    if (roadClass > 12 || !Number.isFinite(metres) || metres < 0) throw new Error(`invalid service-tree road ${index}`)
    return { ...geometry.row(index), ...geometry.endpointKeys(index), roadClass, length: metres, sourceId: table.getChild('source_id')!.get(index) as number,
      access: table.getChild('access')!.get(index) as number, tunnel: tunnel.get(index) as boolean }
  })
  return { roads, fleets, unknownCountryRows }
}

export async function enrichServiceTreeSquare(directory: string) {
  const roadsPath = resolve(directory, 'roads.arrow'), buildingsPath = resolve(directory, 'buildings.arrow')
  if (!existsSync(buildingsPath)) throw new Error(`${directory}: missing original OSM buildings.arrow`)
  let counts = { rows: 0, matched: 0, retracted: 0, updated: false, unknownCountryRows: 0 }
  await withArrowWrite(roadsPath, table => {
    const { roads, fleets, unknownCountryRows } = readServiceRoads(table)
    const buildings = readServiceBuildings(tableFromIPC(readFileSync(buildingsPath)))
    const graph = buildGraph(roads), components = findComponents(graph)
    const eligible: number[] = []
    for (const component of components) for (const index of component.segments) eligible.push(index)
    const loads = assignBuildingsGlobally(roads, eligible, buildings), aadt = new Map<number, RoadAadt>()
    // A valid empty building table does not invent the 20/day floor. Retractions
    // clear obsolete self-stamps on eligible or excluded roads when demand is absent.
    if (buildings.length) for (const component of components) {
      const flow = flowAccumulate(component, graph.segNodeIds, { get: index => roads[index].length }, loads, index => fleets[index])
      for (const [index, trips] of flow) {
        aadt.set(index, splitAADT(Math.min(trips, SERVICE_TREE_CAP_PER_CLASS[roads[index].roadClass]), fleets[index]))
      }
    }
    const applied = applyRoadAadt(table, roadsPath, (_row, index) => aadt.get(index) ?? null,
      undefined, undefined, { sourceIds: [SOURCE_ID_SERVICE_TREE_HEURISTIC], when: (_row, index) => !buildings.length || graph.eligible[index] === 0 })
    counts = { ...counts, ...applied.result, unknownCountryRows }
    return applied.table
  })
  return counts
}

async function main(): Promise<void> {
  const { values } = parseArgs({ options: { 'prepared-dir': { type: 'string' } } })
  if (!values['prepared-dir']) throw new Error('usage: enrich-roads-service-tree.ts --prepared-dir PREPARED_YEAR_DIR')
  const directory = resolve(values['prepared-dir'])
  const squares = listPreparedSquares(directory, [-90, -180, 90, 180])
  if (!squares.length) throw new Error(`${directory}: no prepared road scope`)
  for (const square of squares) {
    if (!existsSync(resolve(directory, square, 'buildings.arrow'))) throw new Error(`${square}: missing original OSM buildings.arrow`)
  }
  for (const square of squares) console.log(JSON.stringify({ square, ...await enrichServiceTreeSquare(resolve(directory, square)) }))
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => { console.error(error); process.exitCode = 1 })
}
