/** Align a whole observed railway shape to ordered source-way occurrences, preserving estimated fractional passages. */

import { flatDist, pointToSegmentParamT, wrapLonDeltaDeg } from './spatial.js'
import type { OrientedSourceRailWay } from './transport-topology.js'

type Point = readonly [latitude: number, longitude: number]
type Way = { id: string; points: Point[]; nodes: string[]; distances: number[]; length: number; direction: number }
type Position = { occurrence: number; local: number; point: Point; error: number }
type State = Position & { cost: number; previous: number }

export interface RailServicePassage {
  occurrence: number
  way: string
  from: number
  to: number
  shapeVertices: number
}

export interface RailServiceTurn {
  incoming: number
  outgoing: number
  way: string
  estimateM: number
  /** Conditional on the selected projections; not a physical confidence interval. */
  projectionAdmissibleRangeM: [number, number]
}

export type RailServiceShapeAlignment = {
  status: 'unmatched'
  reason: string
  occurrence?: number
  joinCount?: number
  failedVertex?: number
  transitions?: number
  maxCandidates?: number
} | {
  status: 'aligned'
  interpretation: 'estimated_alignment'
  turns: RailServiceTurn[]
  cost: number
  transitions: number
  maxCandidates: number
  lengthM: number
  passages: RailServicePassage[]
  maxErrorM: number
}

// Retained shape-candidate search radius; this bounds matching, not claimed accuracy.
const SHAPE_CANDIDATE_RADIUS_M = 250

function sourceWayGeometry(way: OrientedSourceRailWay): Way {
  const points = way.nodes.map(([, point]) => {
    if (!point) throw new Error(`unresolved source coordinates for way ${way.id}`)
    return point
  })
  if (points.length < 2) throw new Error(`source way ${way.id} has fewer than two nodes`)
  const distances = [0]
  for (let index = 1; index < points.length; index++) {
    distances.push(distances.at(-1)! + flatDist(...points[index - 1], ...points[index]))
  }
  return { id: way.id, points, nodes: way.nodes.map(node => node[0]), distances,
    length: distances.at(-1)!, direction: way.reverse ? -1 : 1 }
}

export function alignRailServiceShape(
  shape: readonly Point[],
  itinerary: readonly OrientedSourceRailWay[],
): RailServiceShapeAlignment {
  if (shape.length < 2 || !itinerary.length) return { status: 'unmatched', reason: 'insufficient source observations or empty itinerary' }
  const ways = itinerary.map(sourceWayGeometry)
  const reversalBefore = ways.map((way, index) => index > 0 &&
    way.id === ways[index - 1].id && way.direction !== ways[index - 1].direction)
  const entries = ways.map(way => way.direction === 1 ? 0 : way.length)
  const exits = ways.map(way => way.direction === 1 ? way.length : 0)
  for (let occurrence = 1; occurrence < ways.length; occurrence++) {
    if (reversalBefore[occurrence]) continue
    const previous = ways[occurrence - 1], next = ways[occurrence]
    const joins = new Map<string, [number, number]>()
    for (let a = 0; a < previous.nodes.length; a++) {
      for (let b = 0; b < next.nodes.length; b++) {
        if (previous.nodes[a] !== next.nodes[b]) continue
        const from = previous.distances[a], to = next.distances[b]
        // Explicit zero hops can repeat one canonical node at the same physical position.
        joins.set(`${from}:${to}`, [from, to])
      }
    }
    if (joins.size !== 1) return { status: 'unmatched', reason: 'source junction missing or ambiguous', occurrence, joinCount: joins.size }
    ;[exits[occurrence - 1], entries[occurrence]] = joins.values().next().value!
  }
  if (ways.some((way, index) => (exits[index] - entries[index]) * way.direction < 0)) {
    return { status: 'unmatched', reason: 'source junctions conflict with direction' }
  }

  function candidates(point: Point, previous: State[]): Position[] {
    const positions = new Map<string, Position>()
    const add = (occurrence: number, local: number, projected: Point) => {
      const error = flatDist(...point, ...projected)
      if (error <= SHAPE_CANDIDATE_RADIUS_M) positions.set(`${occurrence}:${local}`, { occurrence, local, point: projected, error })
    }
    for (let occurrence = 0; occurrence < ways.length; occurrence++) {
      const way = ways[occurrence]
      const lower = Math.min(entries[occurrence], exits[occurrence])
      const upper = Math.max(entries[occurrence], exits[occurrence])
      for (let vertex = 1; vertex < way.points.length; vertex++) {
        if (way.distances[vertex] < lower || way.distances[vertex - 1] > upper) continue
        const a = way.points[vertex - 1], b = way.points[vertex]
        const edgeLength = way.distances[vertex] - way.distances[vertex - 1]
        if (!edgeLength) continue
        const fraction = Math.max(Math.max(0, (lower - way.distances[vertex - 1]) / edgeLength),
          Math.min(Math.min(1, (upper - way.distances[vertex - 1]) / edgeLength), pointToSegmentParamT(...point, ...a, ...b)))
        const projected: Point = [a[0] + fraction * (b[0] - a[0]), a[1] + fraction * wrapLonDeltaDeg(b[1] - a[1])]
        const local = fraction === 1 ? way.distances[vertex] :
          way.distances[vertex - 1] + fraction * (way.distances[vertex] - way.distances[vertex - 1])
        add(occurrence, local, projected)
      }
    }
    // A noisy observation can leave progress unchanged instead of inventing a reverse movement.
    for (const state of previous) add(state.occurrence, state.local, state.point)
    return [...positions.values()]
  }

  function traversal(from: Position, to: Position, visit?: (occurrence: number, from: number, to: number) => void): number {
    if (to.occurrence < from.occurrence) return Infinity
    let length = 0
    const travel = (occurrence: number, start: number, end: number) => {
      const distance = (end - start) * ways[occurrence].direction
      length = distance < 0 ? Infinity : length + distance
      if (Number.isFinite(length)) visit?.(occurrence, start, end)
    }
    if (from.occurrence === to.occurrence) travel(from.occurrence, from.local, to.local)
    else if (to.occurrence === from.occurrence + 1 && reversalBefore[to.occurrence]) {
      const turn = ways[from.occurrence].direction === 1 ? Math.max(from.local, to.local) : Math.min(from.local, to.local)
      travel(from.occurrence, from.local, turn)
      travel(to.occurrence, turn, to.local)
    } else {
      // Skipping a reversal leaves its fractional turning position unobserved.
      for (let index = from.occurrence + 1; index <= to.occurrence; index++) if (reversalBefore[index]) return Infinity
      travel(from.occurrence, from.local, exits[from.occurrence])
      for (let index = from.occurrence + 1; index < to.occurrence; index++) travel(index, entries[index], exits[index])
      travel(to.occurrence, entries[to.occurrence], to.local)
    }
    return length
  }

  const layers: State[][] = []
  let transitions = 0, maxCandidates = 0
  for (let vertex = 0; vertex < shape.length; vertex++) {
    const previous = layers.at(-1) ?? []
    const column: State[] = []
    const sourceDistance = vertex ? flatDist(...shape[vertex - 1], ...shape[vertex]) : 0
    for (const position of candidates(shape[vertex], previous)) {
      let bestCost = vertex ? Infinity : position.error ** 2, bestPrevious = -1
      for (let index = 0; index < previous.length; index++) {
        transitions++
        const distance = traversal(previous[index], position)
        const cost = previous[index].cost + position.error ** 2 + (distance - sourceDistance) ** 2
        if (cost < bestCost) { bestCost = cost; bestPrevious = index }
      }
      if (Number.isFinite(bestCost)) column.push({ ...position, cost: bestCost, previous: bestPrevious })
    }
    maxCandidates = Math.max(maxCandidates, column.length)
    if (!column.length) return { status: 'unmatched', reason: 'shape conflicts with source itinerary', failedVertex: vertex, transitions, maxCandidates }
    layers.push(column)
  }
  let chosen = layers.at(-1)!.reduce((best, state, index, all) => state.cost < all[best].cost ? index : best, 0)
  const cost = layers.at(-1)![chosen].cost
  const positions: Position[] = []
  for (let vertex = layers.length - 1; vertex >= 0; vertex--) {
    const state = layers[vertex][chosen]
    positions.push(state)
    chosen = state.previous
  }
  positions.reverse()
  const passages: RailServicePassage[] = []
  const shapeVertices = new Map<number, number>()
  for (const position of positions) shapeVertices.set(position.occurrence, (shapeVertices.get(position.occurrence) ?? 0) + 1)
  const turns: RailServiceTurn[] = []
  function append(occurrence: number, from: number, to: number) {
    if (from === to) return
    if ((to - from) * ways[occurrence].direction < 0) throw new Error('invented reversal inside a directed occurrence')
    const last = passages.at(-1)
    if (last && last.occurrence === occurrence && last.to === from) last.to = to
    else passages.push({ occurrence, way: ways[occurrence].id, from, to, shapeVertices: shapeVertices.get(occurrence) ?? 0 })
  }
  for (let index = 1; index < positions.length; index++) {
    const a = positions[index - 1], b = positions[index]
    if (!Number.isFinite(traversal(a, b, append))) throw new Error('reconstructed an unreachable transition')
    if (b.occurrence === a.occurrence + 1 && reversalBefore[b.occurrence]) {
      const forward = ways[a.occurrence].direction === 1
      const estimate = forward ? Math.max(a.local, b.local) : Math.min(a.local, b.local)
      turns.push({ incoming: a.occurrence, outgoing: b.occurrence, way: ways[a.occurrence].id, estimateM: estimate,
        projectionAdmissibleRangeM: forward ? [estimate, exits[a.occurrence]] : [exits[a.occurrence], estimate] })
    }
  }
  return { status: 'aligned', interpretation: 'estimated_alignment', turns, cost, transitions, maxCandidates,
    lengthM: passages.reduce((sum, passage) => sum + Math.abs(passage.to - passage.from), 0), passages,
    maxErrorM: positions.reduce((maximum, position) => Math.max(maximum, position.error), 0) }
}
