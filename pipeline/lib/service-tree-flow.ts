/** Service-tree motor exits, connected components and bottom-up Dijkstra traffic. */

import { nodeKey } from './spatial.js'
import { MinHeap } from './min-heap.js'
import { shouldOverwrite, SOURCE_ID_SERVICE_TREE_HEURISTIC } from './sources.js'
import type { SegmentGeometry } from './prepared-grid.js'
import type { CountryFleet } from './country-fleet.js'
import type { BuildingLoad } from './trip-rates.js'

export interface ServiceRoad extends SegmentGeometry {
  roadClass: number; sourceId: number; tunnel: boolean; access: number; length: number
}
interface GraphNode { eligibleEdges: number[]; hasExitEdge: boolean }
export interface Graph { nodes: GraphNode[]; segNodeIds: Int32Array; eligible: Uint8Array }

export function buildGraph(roads: readonly ServiceRoad[]): Graph {
  const nodes: GraphNode[] = [], ids = new Map<string, number>()
  const segNodeIds = new Int32Array(2 * roads.length), eligible = new Uint8Array(roads.length)
  const intern = (lat: number, lon: number) => {
    const key = nodeKey(lat, lon)
    let id = ids.get(key)
    if (id === undefined) { id = nodes.length; ids.set(key, id); nodes.push({ eligibleEdges: [], hasExitEdge: false }) }
    return id
  }
  roads.forEach((road, index) => {
    const a = intern(road.startLat, road.startLon), b = intern(road.endLat, road.endLon)
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
): Map<number, number> {

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

  const segLocalEnds: { a: number; b: number }[] = new Array(comp.segments.length)
  for (let i = 0; i < comp.segments.length; i++) {
    const seg = comp.segments[i]
    const a = intern(segNodeIds[seg * 2])
    const b = intern(segNodeIds[seg * 2 + 1])
    segLocalEnds[i] = { a, b }
    localAdj[a].push(seg)
    localAdj[b].push(seg)
  }

  const segLocalLookup = new Map<number, { a: number; b: number }>()
  for (let i = 0; i < comp.segments.length; i++) {
    segLocalLookup.set(comp.segments[i], segLocalEnds[i])
  }

  const numLocal = localToGlobal.length

  const segFlow = new Map<number, number>()
  for (const seg of comp.segments) {
    const load = segLoadGlobal.get(seg)
    segFlow.set(seg, load ? load.dwellings * fleetForSeg(seg).tripsPerDwelling + load.trips : 0)
  }

  const dist = new Float64Array(numLocal)
  dist.fill(Infinity)
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
  for (const r of localRoots) { dist[r] = 0; pq.push(0, r) }

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

  return segFlow
}
