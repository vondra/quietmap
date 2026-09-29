/** Service-tree motor exits, connected components and bottom-up Dijkstra traffic. */

import { MinHeap } from './min-heap.js'
import { shouldOverwrite, SOURCE_ID_SERVICE_TREE_HEURISTIC } from './sources.js'
import type { CountryFleet } from './country-fleet.js'
import type { BuildingLoad } from './trip-rates.js'

export interface ServiceRoad {
  startLat: number; startLon: number; endLat: number; endLon: number
  /** Endpoint identities: equal numbers are one graph node. */
  startNode: number; endNode: number
  name: string; osmId: bigint; builtUp: number
  roadClass: number; sourceId: number; tunnel: boolean; access: number; length: number
  /** Mapped lane count, 0 when untagged. */
  lanes: number
}
interface GraphNode { eligibleEdges: number[]; hasExitEdge: boolean }
export interface Graph { nodes: GraphNode[]; segNodeIds: Int32Array; eligible: Uint8Array }

export function buildGraph(roads: readonly ServiceRoad[]): Graph {
  const nodes: GraphNode[] = [], ids = new Map<number, number>()
  const segNodeIds = new Int32Array(2 * roads.length), eligible = new Uint8Array(roads.length)
  const intern = (key: number) => {
    let id = ids.get(key)
    if (id === undefined) { id = nodes.length; ids.set(key, id); nodes.push({ eligibleEdges: [], hasExitEdge: false }) }
    return id
  }
  roads.forEach((road, index) => {
    const a = intern(road.startNode), b = intern(road.endNode)
    segNodeIds[index * 2] = a; segNodeIds[index * 2 + 1] = b
    const local = road.roadClass >= 5 && road.roadClass <= 9 && road.roadClass !== 8
    if (local && !road.tunnel && road.access !== 2 && road.access !== 4 &&
        shouldOverwrite(road.sourceId, SOURCE_ID_SERVICE_TREE_HEURISTIC)) {
      eligible[index] = 1; nodes[a].eligibleEdges.push(index); nodes[b].eligibleEdges.push(index)
    } else if (road.roadClass < 5 || (road.roadClass >= 10 && road.roadClass <= 12) || local) {
      // Measured local roads and non-emitting motor links drain flow; tracks do not.
      nodes[a].hasExitEdge = true; nodes[b].hasExitEdge = true
    }
  })
  return { nodes, segNodeIds, eligible }
}

export interface Component {
  segments: number[]
  rootNodes: Set<number>
}

export function findComponents(graph: Graph): Component[] {
  const { nodes, segNodeIds, eligible } = graph

  const visited = new Uint8Array(eligible.length)
  const components: Component[] = []

  const queue: number[] = []

  for (let i = 0; i < eligible.length; i++) {
    if (!eligible[i] || visited[i]) continue

    const comp: Component = { segments: [], rootNodes: new Set() }
    queue.length = 0
    queue.push(i)
    visited[i] = 1

    let head = 0
    while (head < queue.length) {
      const seg = queue[head++]
      comp.segments.push(seg)

      const sId = segNodeIds[seg * 2]
      const eId = segNodeIds[seg * 2 + 1]
      for (let endSel = 0; endSel < 2; endSel++) {
        const nodeId = endSel === 0 ? sId : eId
        const node = nodes[nodeId]
        if (node.hasExitEdge) {
          comp.rootNodes.add(nodeId)
        }
        const edges = node.eligibleEdges
        for (let k = 0; k < edges.length; k++) {
          const adj = edges[k]
          if (!visited[adj]) {
            visited[adj] = 1
            queue.push(adj)
          }
        }
      }
    }

    components.push(comp)
  }

  return components
}

export function flowAccumulate(
  comp: Component,
  segNodeIds: Int32Array,
  lengthCol: { get(index: number): number | null },
  segLoadGlobal: Map<number, BuildingLoad>,
  fleetForSeg: (seg: number) => CountryFleet,
): { trips: Map<number, number>; exitBoundary: Set<number> } {

  const globalToLocal = new Map<number, number>()
  const localToGlobal: number[] = []
  const localAdj: number[][] = []
  function intern(globalId: number): number {
    let local = globalToLocal.get(globalId)
    if (local === undefined) {
      local = localToGlobal.length
      localToGlobal.push(globalId)
      localAdj.push([])
      globalToLocal.set(globalId, local)
    }
    return local
  }

  const segLocalLookup = new Map<number, { a: number; b: number }>()
  for (const seg of comp.segments) {
    const a = intern(segNodeIds[seg * 2]), b = intern(segNodeIds[seg * 2 + 1])
    segLocalLookup.set(seg, { a, b })
    localAdj[a].push(seg)
    localAdj[b].push(seg)
  }

  const numLocal = localToGlobal.length

  const segFlow = new Map<number, number>()
  for (const seg of comp.segments) {
    const load = segLoadGlobal.get(seg)
    segFlow.set(seg, load ? load.dwellings * fleetForSeg(seg).tripsPerDwelling + load.trips : 0)
  }

  const dist = new Float64Array(numLocal)
  dist.fill(Infinity)
  const exit = new Int32Array(numLocal)
  exit.fill(-1)
  const downSeg = new Int32Array(numLocal)
  downSeg.fill(-1)

  const localRoots: number[] = []
  for (const globalId of comp.rootNodes) {
    const local = globalToLocal.get(globalId)
    if (local !== undefined) localRoots.push(local)
  }
  if (localRoots.length === 0) {
    let best = 0, bestDeg = -1
    for (let l = 0; l < numLocal; l++) {
      const d = localAdj[l].length
      if (d > bestDeg) { bestDeg = d; best = l }
    }
    localRoots.push(best)
  }

  const pq = new MinHeap()
  for (const r of localRoots) { dist[r] = 0; exit[r] = r; pq.push(0, r) }

  while (pq.size > 0) {
    const { dist: d, node: u } = pq.pop()
    if (d > dist[u]) continue

    const edges = localAdj[u]
    for (let k = 0; k < edges.length; k++) {
      const seg = edges[k]
      const ends = segLocalLookup.get(seg)!
      const v = ends.a === u ? ends.b : ends.a
      const len = Math.max(1, (lengthCol.get(seg) as number) ?? 1)
      const newDist = d + len
      if (newDist < dist[v]) {
        dist[v] = newDist
        downSeg[v] = seg
        exit[v] = exit[u]
        pq.push(newDist, v)
      }
    }
  }

  const sortedArr = Array.from({ length: numLocal }, (_, index) => index)
  sortedArr.sort((a, b) => dist[b] - dist[a])

  for (const u of sortedArr) {
    let inflow = 0
    const edges = localAdj[u]
    const distU = dist[u]
    for (let k = 0; k < edges.length; k++) {
      const seg = edges[k]
      const ends = segLocalLookup.get(seg)!
      const other = ends.a === u ? ends.b : ends.a
      if (dist[other] > distU) {
        inflow += segFlow.get(seg)!
      }
    }

    const dSeg = downSeg[u]
    if (dSeg !== -1) {
      segFlow.set(dSeg, segFlow.get(dSeg)! + inflow)
    }
  }

  const exitBoundary = new Set<number>()
  for (const [seg, { a, b }] of segLocalLookup) {
    if (exit[a] !== exit[b]) exitBoundary.add(seg)
  }
  return { trips: segFlow, exitBoundary }
}

export interface StreetDemand { trips: number; through: boolean; singleTrack: boolean }

/** Same name inside one component, otherwise the same OSM way, including disconnected pieces. */
export function serviceStreetDemands(
  roads: readonly ServiceRoad[], graph: Graph, components: readonly Component[],
  loads: Map<number, BuildingLoad>, fleets: readonly CountryFleet[],
): { rowTrips: Float64Array; streets: Map<number, StreetDemand> } {
  const rowTrips = new Float64Array(roads.length), streets = new Map<number, StreetDemand>()
  const groups = new Map<string | bigint, StreetDemand>()
  const laneEvidence = new Map<string | bigint, { one: boolean; multi: boolean }>()
  components.forEach((component, componentIndex) => {
    const flow = flowAccumulate(component, graph.segNodeIds, { get: i => roads[i].length }, loads, i => fleets[i])
    for (const index of component.segments) {
      const road = roads[index], trips = flow.trips.get(index)!
      rowTrips[index] = trips
      const key = road.name ? `${componentIndex}/${road.name}` : road.osmId
      let street = groups.get(key)
      if (!street) { street = { trips: 0, through: false, singleTrack: false }; groups.set(key, street) }
      street.trips = Math.max(street.trips, trips)
      street.through ||= flow.exitBoundary.has(index)
      if (road.lanes > 0) {
        const evidence = laneEvidence.get(key) ?? { one: false, multi: false }
        if (road.lanes === 1) evidence.one = true
        else evidence.multi = true
        laneEvidence.set(key, evidence)
      }
      streets.set(index, street)
    }
  })
  // A street is single-track only when some piece is mapped single-lane and none is mapped wider.
  for (const [key, street] of groups) {
    const evidence = laneEvidence.get(key)
    street.singleTrack = evidence?.one === true && evidence.multi !== true
  }
  return { rowTrips, streets }
}
