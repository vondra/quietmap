/** Route whole GTFS railway services onto source ways, then clip acoustic pieces. */

import type { GtfsService, GtfsServiceStop } from './gtfs-service-store.js'
import {
  STATION_SNAP_RADIUS_M, SHAPE_CORRIDOR_TOLERANCE_M, UNLOCALIZED_PAIR_QUARANTINE_RADIUS_M,
  isWalkableRailType, walkFamilyBit, type RailGraph, type RailGraphEdge,
} from './rail-graph.js'
import { createDijkstraScratch, dijkstraShortestPath, type DijkstraScratch } from './rail-graph-metrics.js'
import {
  CompleteTrainRouteIndex, type RailRelationAssociation,
} from './rail-service-match.js'
import { alignRailServiceShape } from './rail-service-shape.js'
import { flatDist, pointToPolylineDist, pointToSegmentDist, pointToSegmentParamT, M_PER_DEG_LAT, M_PER_DEG_LON_EQ } from './spatial.js'
import type { RailServicePassages, ClippedRailPassage } from './rail-passage.js'
import { SourceTransportTopology, type SourceTrainRoute } from './transport-topology.js'

const EDGE_CELL_DEG = 0.01
/** Turn sharpness (cosine against the incoming heading) that splits one
 *  station leg into separately walked runs: a Dijkstra shortest path never
 *  doubles back, so an out-and-back inside a single leg survives only as
 *  consecutive runs that retrace the shared edges. */
const SHAPE_REVERSAL_COS = -0.5

export interface RailServiceRoutingCounts {
  total: number
  relationEstimated: number
  graphEstimated: number
  unmatched: number
  failures: { snapFailed: number; disconnected: number; ambiguous: number }
}

export interface RailServiceRouteResult extends RailServiceRoutingCounts {
  dailyDepartures: RailServiceRoutingCounts
  services: RailServicePassages[]
  quarantinedPieceKeys: Set<string>
}

interface EdgeSnap {
  edgeIndex: number
  parameter: number
  distM: number
}

interface DirectedVisit {
  edgeIndex: number
  fromNode: number
  toNode: number
  fromParameter: number
  toParameter: number
}

/** Pattern identity is the observed geometry, not feed-local ids alone: one
 *  enrichment call folds every feed of a country together and shape/stop ids
 *  collide across feeds. Equal rounded observations merge (one announced
 *  service, counts add); unequal geometry never shares one corridor. */
function patternKey(service: GtfsService): string {
  const at = (latitude: number, longitude: number): string => `${latitude.toFixed(5)},${longitude.toFixed(5)}`
  return `${service.directionId}\n${service.stops.map(stop => `${stop.sourceStopId}@${at(stop.lat, stop.lon)}`).join('\n')}\n${service.shape.map(point => at(point.lat, point.lon)).join('\n')}`
}

function distinctConsecutiveCoordinates(points: readonly { lat: number; lon: number }[]): Array<{ lat: number; lon: number }> {
  return points.filter((point, index) => index === 0 ||
    point.lat !== points[index - 1].lat || point.lon !== points[index - 1].lon)
}

/** Some feeds supply only station-to-station lines as shapes. They carry no
 *  track geometry and must retain the shape-less ambiguity check. */
function gtfsShape(service: GtfsService): Array<[number, number]> {
  if (service.shape.length < 2) return []
  const shape = distinctConsecutiveCoordinates(service.shape)
  const stops = distinctConsecutiveCoordinates(service.stops)
  if (shape.length === stops.length && shape.every((point, index) =>
    point.lat === stops[index].lat && point.lon === stops[index].lon)) return []
  return service.shape.map(point => [point.lat, point.lon])
}

function stopPolyline(stops: readonly GtfsServiceStop[]): Array<[number, number]> {
  return collapseStops(stops).map(stop => [stop.lat, stop.lon])
}

function collapseStops(stops: readonly GtfsServiceStop[]): GtfsServiceStop[] {
  const collapsed: GtfsServiceStop[] = []
  for (const stop of stops) {
    if (collapsed.at(-1)?.stopId === stop.stopId) continue
    collapsed.push(stop)
  }
  return collapsed
}

function cell(latitude: number, longitude: number): string {
  return `${Math.floor(latitude / EDGE_CELL_DEG)}_${Math.floor(longitude / EDGE_CELL_DEG)}`
}

function isStampable(edge: RailGraphEdge): boolean {
  return !edge.isTraversalOnly && isWalkableRailType(edge.railType)
}

class RailEdgeIndex {
  private readonly cells = new Map<string, number[]>()
  constructor(private readonly graph: RailGraph) {
    graph.edges.forEach((edge, index) => {
      this.push(edge.startLat, edge.startLon, index)
      this.push(edge.endLat, edge.endLon, index)
    })
  }
  private push(latitude: number, longitude: number, index: number): void {
    const key = cell(latitude, longitude)
    const bucket = this.cells.get(key)
    if (bucket) bucket.push(index)
    else this.cells.set(key, [index])
  }
  snapsByComponent(latitude: number, longitude: number, radiusM: number, allow?: (edge: RailGraphEdge) => boolean): Map<number, EdgeSnap> {
    const latSpan = EDGE_CELL_DEG * M_PER_DEG_LAT
    const lonSpan = EDGE_CELL_DEG * M_PER_DEG_LON_EQ * Math.max(0.05, Math.cos(latitude * Math.PI / 180))
    const dyMax = Math.max(1, Math.ceil(radiusM / latSpan))
    const dxMax = Math.max(1, Math.ceil(radiusM / lonSpan))
    const originY = Math.floor(latitude / EDGE_CELL_DEG)
    const originX = Math.floor(longitude / EDGE_CELL_DEG)
    const seen = new Set<number>()
    const bestByComponent = new Map<number, EdgeSnap>()
    for (let dy = -dyMax; dy <= dyMax; dy++) {
      for (let dx = -dxMax; dx <= dxMax; dx++) {
        for (const index of this.cells.get(`${originY + dy}_${originX + dx}`) ?? []) {
          if (seen.has(index)) continue
          seen.add(index)
          const edge = this.graph.edges[index]
          if (allow && !allow(edge)) continue
          if (!edge.isTraversalOnly && !isWalkableRailType(edge.railType)) continue
          const distM = pointToSegmentDist(latitude, longitude, edge.startLat, edge.startLon, edge.endLat, edge.endLon)
          const component = this.graph.componentOfNode[edge.nodeA]
          const best = bestByComponent.get(component)
          if (distM > radiusM || (best && distM >= best.distM)) continue
          bestByComponent.set(component, {
            edgeIndex: index, distM,
            parameter: pointToSegmentParamT(latitude, longitude, edge.startLat, edge.startLon, edge.endLat, edge.endLon),
          })
        }
      }
    }
    return bestByComponent
  }
  nearby(latitude: number, longitude: number, radiusM: number): number[] {
    const latSpan = EDGE_CELL_DEG * M_PER_DEG_LAT
    const lonSpan = EDGE_CELL_DEG * M_PER_DEG_LON_EQ * Math.max(0.05, Math.cos(latitude * Math.PI / 180))
    const dyMax = Math.max(1, Math.ceil(radiusM / latSpan))
    const dxMax = Math.max(1, Math.ceil(radiusM / lonSpan))
    const originY = Math.floor(latitude / EDGE_CELL_DEG)
    const originX = Math.floor(longitude / EDGE_CELL_DEG)
    const seen = new Set<number>(), hits: number[] = []
    for (let dy = -dyMax; dy <= dyMax; dy++) {
      for (let dx = -dxMax; dx <= dxMax; dx++) {
        for (const index of this.cells.get(`${originY + dy}_${originX + dx}`) ?? []) {
          if (seen.has(index)) continue
          seen.add(index)
          const edge = this.graph.edges[index]
          if (!isStampable(edge)) continue
          const distM = pointToSegmentDist(latitude, longitude, edge.startLat, edge.startLon, edge.endLat, edge.endLon)
          if (distM <= radiusM) hits.push(index)
        }
      }
    }
    return hits
  }
}

function shapeFilter(shape: Array<[number, number]>): (edge: RailGraphEdge) => boolean {
  const lonLat = shape.map(([lat, lon]) => [lon, lat] as [number, number])
  return edge => pointToPolylineDist(
    (edge.startLat + edge.endLat) / 2, (edge.startLon + edge.endLon) / 2, lonLat,
  ) <= SHAPE_CORRIDOR_TOLERANCE_M
}

/** Shape-vertex anchor per station, searched forward from the previous
 *  station's anchor so a repeated stop anchors to its later occurrence — an
 *  out-and-back service keeps its return leg distinct. */
function stationShapeAnchors(
  shape: ReadonlyArray<[number, number]>,
  stations: readonly GtfsServiceStop[],
): number[] {
  const anchors: number[] = []
  let cursor = 0
  for (const station of stations) {
    let best = cursor, bestDistM = Infinity
    for (let index = cursor; index < shape.length; index++) {
      const distM = flatDist(station.lat, station.lon, shape[index][0], shape[index][1])
      if (distM < bestDistM) { best = index; bestDistM = distM }
    }
    anchors.push(best)
    cursor = best
  }
  return anchors
}

/** Ordered walk anchors for one station leg: the two stations with the
 *  shape's own vertices between their anchors — the leg's slice of the
 *  service shape, never the whole-trip corridor. */
function legObservations(
  shape: ReadonlyArray<[number, number]>,
  stations: readonly GtfsServiceStop[],
  anchors: readonly number[],
  index: number,
): Array<[number, number]> {
  const from = stations[index - 1], to = stations[index]
  if (!shape.length) return [[from.lat, from.lon], [to.lat, to.lon]]
  const points: Array<[number, number]> = [[from.lat, from.lon]]
  for (let vertex = anchors[index - 1] + 1; vertex < anchors[index]; vertex++) points.push(shape[vertex])
  points.push([to.lat, to.lon])
  return points
}

function splitReversalRuns(points: ReadonlyArray<[number, number]>): Array<Array<[number, number]>> {
  const runs: Array<Array<[number, number]>> = []
  let run: Array<[number, number]> = [points[0]]
  for (let index = 1; index < points.length - 1; index++) {
    const previous = run.at(-1)!, current = points[index], next = points[index + 1]
    run.push(current)
    const cosLat = Math.max(0.05, Math.cos((current[0] + next[0]) / 2 * Math.PI / 180))
    const ax = (current[1] - previous[1]) * M_PER_DEG_LON_EQ * cosLat
    const ay = (current[0] - previous[0]) * M_PER_DEG_LAT
    const bx = (next[1] - current[1]) * M_PER_DEG_LON_EQ * cosLat
    const by = (next[0] - current[0]) * M_PER_DEG_LAT
    const spans = Math.hypot(ax, ay) * Math.hypot(bx, by)
    if (spans > 0 && (ax * bx + ay * by) / spans <= SHAPE_REVERSAL_COS) {
      runs.push(run)
      run = [current]
    }
  }
  run.push(points.at(-1)!)
  runs.push(run)
  return runs
}

function pointAlongPolyline(points: ReadonlyArray<[number, number]>, fraction: number): [number, number] {
  const cumulative = [0]
  for (let index = 1; index < points.length; index++) {
    cumulative.push(cumulative[index - 1] + flatDist(...points[index - 1], ...points[index]))
  }
  const targetM = fraction * cumulative.at(-1)!
  for (let index = 1; index < cumulative.length; index++) {
    if (cumulative[index] >= targetM) {
      const spanM = cumulative[index] - cumulative[index - 1] || 1
      const along = (targetM - cumulative[index - 1]) / spanM
      return [
        points[index - 1][0] + along * (points[index][0] - points[index - 1][0]),
        points[index - 1][1] + along * (points[index][1] - points[index - 1][1]),
      ]
    }
  }
  return points.at(-1)!
}

function stitch(visits: DirectedVisit[], next: DirectedVisit[]): DirectedVisit[] {
  if (!visits.length) return next
  if (!next.length) return visits
  const previous = visits.at(-1)!, first = next[0]
  if (previous.edgeIndex === first.edgeIndex && previous.toNode === first.toNode) {
    previous.toParameter = first.toParameter
    previous.toNode = first.toNode
    return [...visits, ...next.slice(1)]
  }
  return [...visits, ...next]
}

function clipVisit(
  graph: RailGraph,
  topology: SourceTransportTopology,
  visit: DirectedVisit,
  occurrence: number,
): ClippedRailPassage | null {
  const edge = graph.edges[visit.edgeIndex]
  const colon = edge.key.lastIndexOf(':')
  const wayId = edge.osmId
  const segmentIndex = Number(edge.key.slice(colon + 1))
  const extent = topology.pieceExtent(wayId, segmentIndex)
  const fromM = extent.from + visit.fromParameter * (extent.to - extent.from)
  const toM = extent.from + visit.toParameter * (extent.to - extent.from)
  if (fromM === toM) return null
  return { wayId, segmentIndex, square: extent.square, fromM, toM, occurrence }
}

/** Walk one anchor pair across the graph. The fractional edge projections
 *  are solved, never snapped to a node: each end may leave through EITHER
 *  endpoint of its snapped edge, the (endpoint, endpoint) combination with
 *  the smallest approach + path total wins, and combos are tried in
 *  lower-bound order with pruning so most legs still run one search. */
function walkPair(
  graph: RailGraph,
  edges: RailEdgeIndex,
  from: readonly [number, number],
  to: readonly [number, number],
  corridor: ((edge: RailGraphEdge) => boolean) | null,
  scratch: DijkstraScratch,
): DirectedVisit[] | 'snap' | 'disconnected' | 'ambiguous' {
  const fromCandidates = edges.snapsByComponent(from[0], from[1], STATION_SNAP_RADIUS_M, corridor ?? undefined)
  const toCandidates = edges.snapsByComponent(to[0], to[1], STATION_SNAP_RADIUS_M, corridor ?? undefined)
  if (!fromCandidates.size || !toCandidates.size) return 'snap'
  // A nearer isolated platform track cannot carry a service to another component.
  let pair: { from: EdgeSnap; to: EdgeSnap; distance: number } | null = null
  for (const [component, fromCandidate] of fromCandidates) {
    const toCandidate = toCandidates.get(component)
    if (!toCandidate) continue
    const distance = fromCandidate.distM + toCandidate.distM
    if (!pair || distance < pair.distance) pair = { from: fromCandidate, to: toCandidate, distance }
  }
  if (!pair) return 'disconnected'
  const fromSnap = pair.from, toSnap = pair.to
  const fromEdge = graph.edges[fromSnap.edgeIndex], toEdge = graph.edges[toSnap.edgeIndex]
  if (fromSnap.edgeIndex === toSnap.edgeIndex) {
    if (fromSnap.parameter === toSnap.parameter) return []
    const forward = fromSnap.parameter < toSnap.parameter
    return [{
      edgeIndex: fromSnap.edgeIndex,
      fromNode: forward ? fromEdge.nodeA : fromEdge.nodeB,
      toNode: forward ? fromEdge.nodeB : fromEdge.nodeA,
      fromParameter: fromSnap.parameter, toParameter: toSnap.parameter,
    }]
  }
  const family = walkFamilyBit(fromEdge.railType)
  const familyFilter = (edge: RailGraphEdge) =>
    edge.isTraversalOnly || (family === 0 ? isWalkableRailType(edge.railType) : walkFamilyBit(edge.railType) === family)
  const filter = corridor ? (edge: RailGraphEdge) => familyFilter(edge) && corridor(edge) : familyFilter
  const approachM = (edge: RailGraphEdge, parameter: number, node: number): number =>
    Math.abs(parameter - (node === edge.nodeA ? 0 : 1)) * edge.lengthM
  const combos = [fromEdge.nodeA, fromEdge.nodeB].flatMap(u =>
    [toEdge.nodeA, toEdge.nodeB].map(v => ({
      u, v,
      lowerBoundM: approachM(fromEdge, fromSnap.parameter, u) + approachM(toEdge, toSnap.parameter, v) +
        flatDist(graph.nodes[u].lat, graph.nodes[u].lon, graph.nodes[v].lat, graph.nodes[v].lon),
    })))
  combos.sort((a, b) => a.lowerBoundM - b.lowerBoundM)
  let winner: { u: number; v: number; path: NonNullable<ReturnType<typeof dijkstraShortestPath>> } | null = null
  let winnerTotalM = Infinity
  for (const combo of combos) {
    if (combo.lowerBoundM >= winnerTotalM) break
    const path = dijkstraShortestPath(graph, combo.u, combo.v, filter, (_index, edge) => edge.lengthM, scratch)
    if (!path) continue
    const totalM = approachM(fromEdge, fromSnap.parameter, combo.u) + path.lengthM +
      approachM(toEdge, toSnap.parameter, combo.v)
    if (totalM < winnerTotalM) { winner = { u: combo.u, v: combo.v, path }; winnerTotalM = totalM }
  }
  if (!winner) return 'disconnected'
  if (!corridor) {
    // No GTFS shape decided this leg — the stop polyline must not silently
    // pose as one. Probe whether a second comparably short route shares
    // under half the winner's edges, and report ambiguity so the caller
    // quarantines instead of stamping one of two plausible corridors.
    const best = winner.path
    const alt = dijkstraShortestPath(graph, winner.u, winner.v, familyFilter,
      (index, edge) => edge.lengthM * (best.edgeIndices.has(index) ? 3 : 1), scratch)
    if (alt && alt.lengthM <= 1.2 * best.lengthM) {
      let shared = 0
      for (const index of alt.edgeIndices) if (best.edgeIndices.has(index)) shared++
      if (best.edgeIndices.size && shared / best.edgeIndices.size < 0.5) return 'ambiguous'
    }
  }
  const visits: DirectedVisit[] = []
  let node = winner.u
  for (const edgeIndex of winner.path.orderedEdges) {
    const edge = graph.edges[edgeIndex]
    const next = edge.nodeA === node ? edge.nodeB : edge.nodeA
    visits.push({ edgeIndex, fromNode: node, toNode: next, fromParameter: edge.nodeA === node ? 0 : 1, toParameter: edge.nodeA === next ? 0 : 1 })
    node = next
  }
  if (visits[0]?.edgeIndex === fromSnap.edgeIndex) visits[0].fromParameter = fromSnap.parameter
  else {
    visits.unshift({
      edgeIndex: fromSnap.edgeIndex,
      fromNode: winner.u === fromEdge.nodeA ? fromEdge.nodeB : fromEdge.nodeA,
      toNode: winner.u,
      fromParameter: fromSnap.parameter, toParameter: winner.u === fromEdge.nodeA ? 0 : 1,
    })
  }
  const last = visits.at(-1)
  if (last?.edgeIndex === toSnap.edgeIndex) last.toParameter = toSnap.parameter
  else {
    visits.push({
      edgeIndex: toSnap.edgeIndex,
      fromNode: winner.v,
      toNode: winner.v === toEdge.nodeA ? toEdge.nodeB : toEdge.nodeA,
      fromParameter: winner.v === toEdge.nodeA ? 0 : 1, toParameter: toSnap.parameter,
    })
  }
  return visits
}

function graphPassages(
  graph: RailGraph,
  topology: SourceTransportTopology,
  edges: RailEdgeIndex,
  stops: readonly GtfsServiceStop[],
  shape: ReadonlyArray<[number, number]>,
  scratch: DijkstraScratch,
): ClippedRailPassage[] | { unmatched: 'snap' | 'disconnected' | 'ambiguous' } {
  const stations = collapseStops(stops)
  if (stations.length < 2) return { unmatched: 'snap' }
  const anchors = stationShapeAnchors(shape, stations)
  let visits: DirectedVisit[] = []
  for (let index = 1; index < stations.length; index++) {
    for (const run of splitReversalRuns(legObservations(shape, stations, anchors, index))) {
      const leg = walkPair(graph, edges, run[0], run.at(-1)!, shape.length ? shapeFilter(run) : null, scratch)
      if (typeof leg === 'string') return { unmatched: leg }
      visits = stitch(visits, leg)
    }
  }
  const passages: ClippedRailPassage[] = []
  visits.forEach((visit, occurrence) => {
    if (!isStampable(graph.edges[visit.edgeIndex])) return
    const passage = clipVisit(graph, topology, visit, occurrence)
    if (passage) passages.push(passage)
  })
  return passages.length ? passages : { unmatched: 'disconnected' }
}

function relationPassages(
  topology: SourceTransportTopology,
  relation: SourceTrainRoute,
  observations: Array<[number, number]>,
): ClippedRailPassage[] | null {
  const alignment = alignRailServiceShape(observations, relation.ways)
  if (alignment.status !== 'aligned') return null
  const passages: ClippedRailPassage[] = []
  alignment.passages.forEach(passage => {
    for (const piece of topology.passagePieces(passage)) {
      passages.push({
        wayId: passage.way, segmentIndex: piece.segmentIndex, square: piece.square,
        fromM: piece.from, toM: piece.to, occurrence: passage.occurrence,
      })
    }
  })
  return passages
}

function quarantineStops(
  graph: RailGraph,
  edges: RailEdgeIndex,
  stops: readonly GtfsServiceStop[],
  shape: ReadonlyArray<[number, number]>,
  keys: Set<string>,
): void {
  const stations = collapseStops(stops)
  const anchors = stationShapeAnchors(shape, stations)
  const mark = (at: readonly [number, number]): void => {
    for (const index of edges.nearby(at[0], at[1], UNLOCALIZED_PAIR_QUARANTINE_RADIUS_M)) {
      keys.add(graph.edges[index].key)
    }
  }
  for (const stop of stations) mark([stop.lat, stop.lon])
  for (let index = 1; index < stations.length; index++) {
    for (const fraction of [0.25, 0.5, 0.75]) {
      mark(pointAlongPolyline(legObservations(shape, stations, anchors, index), fraction))
    }
  }
}

export function routeRailServices(
  services: Iterable<GtfsService>,
  topology: SourceTransportTopology,
  graph: RailGraph,
  sourceId: number,
): RailServiceRouteResult {
  const patterns = new Map<string, { service: GtfsService; passenger: number }>()
  for (const service of services) {
    const existing = patterns.get(patternKey(service))
    if (existing) existing.passenger += service.departureMultiplier
    else patterns.set(patternKey(service), { service, passenger: service.departureMultiplier })
  }
  const routes = new CompleteTrainRouteIndex([...patterns.values()].flatMap(pattern => pattern.service.stops))
  for (const relation of topology.trainRoutes()) routes.add(relation)
  const edges = new RailEdgeIndex(graph)
  const scratch = createDijkstraScratch(graph.nodeCount)
  const result: RailServiceRouteResult = {
    total: 0, relationEstimated: 0, graphEstimated: 0, unmatched: 0,
    failures: { snapFailed: 0, disconnected: 0, ambiguous: 0 },
    dailyDepartures: { total: 0, relationEstimated: 0, graphEstimated: 0, unmatched: 0,
      failures: { snapFailed: 0, disconnected: 0, ambiguous: 0 } },
    services: [], quarantinedPieceKeys: new Set(),
  }
  for (const pattern of patterns.values()) {
    result.total++
    result.dailyDepartures.total += pattern.passenger
    const shape = gtfsShape(pattern.service)
    // Relation alignment and quarantine sampling may use stop positions as
    // observations when the feed ships no shape; corridor walking never does.
    const observations = shape.length ? shape : stopPolyline(pattern.service.stops)
    const association: RailRelationAssociation = routes.associate(pattern.service.stops)
    let passages: ClippedRailPassage[] | null = null
    let matching: RailServicePassages['evidence']['matching'] = 'graph_estimated'
    let relationId: string | undefined
    if (association.status === 'unique_complete_candidate' && observations.length >= 2) {
      passages = relationPassages(topology, association.relation, observations)
      if (passages) { matching = 'relation_estimated'; relationId = association.relation.id }
    }
    if (!passages) {
      const walked = graphPassages(graph, topology, edges, pattern.service.stops, shape, scratch)
      if (Array.isArray(walked)) { passages = walked; matching = 'graph_estimated' }
      else {
        const reason = walked.unmatched === 'snap' ? 'snapFailed' : walked.unmatched
        result.failures[reason]++
        result.dailyDepartures.failures[reason] += pattern.passenger
      }
    }
    if (!passages) {
      result.unmatched++
      result.dailyDepartures.unmatched += pattern.passenger
      quarantineStops(graph, edges, pattern.service.stops, shape, result.quarantinedPieceKeys)
      continue
    }
    const category = matching === 'relation_estimated' ? 'relationEstimated' : 'graphEstimated'
    result[category]++
    result.dailyDepartures[category] += pattern.passenger
    result.services.push({
      evidence: {
        sourceId, passenger: pattern.passenger, freight: 0,
        passengerStatus: 'estimated', freightStatus: 'unknown', matching, relationId,
      },
      passages,
    })
  }
  return result
}
