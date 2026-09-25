/** Extract S2p predictors from the production tree for spatially held-out-safe DfT training. */

import { readFileSync, writeFileSync } from 'node:fs'
import { createHash } from 'node:crypto'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { tableFromIPC } from 'apache-arrow'
import { readServiceRoads } from './enrich-roads-service-tree.js'
import { assignBuildingsGlobally, readServiceBuildings } from './lib/service-tree-buildings.js'
import { buildGraph, findComponents, serviceStreetDemands } from './lib/service-tree-flow.js'
import { isCountHoldoutSquare } from './lib/count-holdout.js'
import { buildOneHundredthDegreeSegmentGrid, pointGridCandidates, pointToSegmentDist } from './lib/spatial.js'

export interface LocalCountPoint { id: string; x: number; y: number; lat: number; lon: number }

export function trainingSquareFeatures(directory: string, points: readonly LocalCountPoint[]) {
  for (const p of points) {
    const x = Math.floor((p.lon + 180) / 360 * 512)
    const y = Math.floor((1 - Math.asinh(Math.tan(p.lat * Math.PI / 180)) / Math.PI) * 256)
    if (x !== p.x || y !== p.y || isCountHoldoutSquare(x, y)) throw new Error(`not a training point: ${p.id}`)
  }
  const roadBytes = readFileSync(resolve(directory, 'roads.arrow'))
  const structureBytes = readFileSync(resolve(directory, 'structures.arrow'))
  const { roads, fleets } = readServiceRoads(tableFromIPC(roadBytes))
  const buildings = readServiceBuildings(tableFromIPC(structureBytes))
  const graph = buildGraph(roads), components = findComponents(graph)
  const loads = assignBuildingsGlobally(roads, components.flatMap(c => c.segments), buildings)
  const { streets } = serviceStreetDemands(roads, graph, components, loads, fleets)
  const segments = roads.map((r, index) => ({ index, startLatitude: r.startLat, startLongitude: r.startLon,
    endLatitude: r.endLat, endLongitude: r.endLon }))
  const grid = buildOneHundredthDegreeSegmentGrid(segments)
  const features = points.flatMap(p => {
    const nearby = [...new Set(pointGridCandidates(p.lat, p.lon, 25, grid))].map(s => ({
      index: s.index, distance: pointToSegmentDist(p.lat, p.lon, s.startLatitude, s.startLongitude, s.endLatitude, s.endLongitude),
    })).sort((a, b) => a.distance - b.distance || a.index - b.index)
    const best = nearby[0]
    if (!best || best.distance > 12) return []
    const road = roads[best.index], street = streets.get(best.index)
    if (!street || ![5, 9].includes(road.roadClass) || road.sourceId !== 11 ||
        nearby.some(n => n.distance <= 20 && roads[n.index].roadClass !== road.roadClass)) return []
    const latitude = (y: number) => Math.atan(Math.sinh(Math.PI * (1 - y / 256))) * 180 / Math.PI
    const edge = Math.min((p.lon - (p.x / 512 * 360 - 180)) * 111320 * Math.cos(p.lat * Math.PI / 180),
      ((p.x + 1) / 512 * 360 - 180 - p.lon) * 111320 * Math.cos(p.lat * Math.PI / 180),
      (p.lat - latitude(p.y + 1)) * 110540, (latitude(p.y) - p.lat) * 110540)
    if (edge <= 30) return []
    return [{ id: p.id, x: p.x, y: p.y, roadClass: road.roadClass, builtUp: road.builtUp,
      trips: street.trips, through: street.through, row: best.index, osmId: road.osmId.toString() }]
  })
  const hash = (bytes: Buffer) => createHash('sha256').update(bytes).digest('hex')
  return { features, roadsSha256: hash(roadBytes), structuresSha256: hash(structureBytes) }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const [directory, queries, output] = process.argv.slice(2)
  if (!output) throw new Error('usage: local-street-features.ts SQUARE_DIRECTORY TRAINING_QUERIES_JSON OUTPUT_JSON')
  writeFileSync(output, JSON.stringify(trainingSquareFeatures(directory, JSON.parse(readFileSync(queries, 'utf8')))))
}
