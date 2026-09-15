/** Associate a GTFS stop order with complete OSM train-route itineraries. */

import { flatDist, pointToSegmentDist, pointToSegmentParamT } from './spatial.js'
import { STATION_SNAP_RADIUS_M } from './rail-graph.js'
import type { GtfsServiceStop } from './gtfs-service-store.js'
import type { SourceTrainRoute } from './transport-topology.js'

type Point = [number, number]
type Stop = Pick<GtfsServiceStop, 'sourceStopId' | 'name' | 'lat' | 'lon'>
type Candidate = { position: number; error: number; cost: number; previous: number }

const RELATION_CELL_DEG = 0.05

export interface RailStopOrderFit {
  relationId: string
  stopCount: number
  maxStopDistanceM: number
}

export type RailRelationAssociation =
  | { status: 'unique_complete_candidate'; relation: SourceTrainRoute; maxStopDistanceM: number }
  | { status: 'ambiguous' }
  | { status: 'unmatched' }

export function relationWayPoints(relation: SourceTrainRoute): Point[] {
  const points: Point[] = []
  relation.ways.forEach((way, index) => {
    const original = way.nodes.map(node => node[1]!)
    const directed = way.reverse ? [...original].reverse() : original
    points.push(...(index ? directed.slice(1) : directed))
  })
  return points
}

export class StopOrderGeometry {
  private readonly cumulative = [0]

  constructor(private readonly points: readonly Point[]) {
    for (let index = 1; index < points.length; index++) {
      this.cumulative.push(this.cumulative[index - 1] + flatDist(...points[index - 1], ...points[index]))
    }
  }

  match(stops: readonly Stop[]): RailStopOrderFit | null {
    if (!stops.length || this.points.length < 2) return null
    const layers: Candidate[][] = []
    for (const stop of stops) {
      const at: Point = [stop.lat, stop.lon]
      if (!at.every(Number.isFinite)) throw new Error('unresolved source stop')
      const column: Candidate[] = []
      for (let index = 1; index < this.points.length; index++) {
        const error = pointToSegmentDist(...at, ...this.points[index - 1], ...this.points[index])
        if (error > STATION_SNAP_RADIUS_M) continue
        const fraction = pointToSegmentParamT(...at, ...this.points[index - 1], ...this.points[index])
        column.push({
          position: this.cumulative[index - 1] + fraction * (this.cumulative[index] - this.cumulative[index - 1]),
          error, cost: Infinity, previous: -1,
        })
      }
      column.sort((a, b) => a.position - b.position)
      if (!column.length) return null
      const previous = layers.at(-1)
      let cursor = 0, best = -1
      for (const candidate of column) {
        if (!previous) { candidate.cost = candidate.error ** 2; continue }
        while (cursor < previous.length && previous[cursor].position <= candidate.position) {
          if (best < 0 || previous[cursor].cost < previous[best].cost) best = cursor
          cursor++
        }
        if (best >= 0) { candidate.previous = best; candidate.cost = previous[best].cost + candidate.error ** 2 }
      }
      if (!column.some(point => Number.isFinite(point.cost))) return null
      layers.push(column)
    }
    const last = layers.at(-1)!
    let current = last.reduce((best, point, index) => point.cost < last[best].cost ? index : best, 0)
    let maxStopDistanceM = 0
    for (let index = layers.length - 1; index >= 0; index--) {
      maxStopDistanceM = Math.max(maxStopDistanceM, layers[index][current].error)
      current = layers[index][current].previous
    }
    return { relationId: '', stopCount: stops.length, maxStopDistanceM }
  }
}

function cellKey(latitude: number, longitude: number): string {
  return `${Math.floor(latitude / RELATION_CELL_DEG)}_${Math.floor(longitude / RELATION_CELL_DEG)}`
}

function* nearbyCellKeys(stops: Iterable<Stop>): Generator<string> {
  const span = Math.max(1, Math.ceil(STATION_SNAP_RADIUS_M / (RELATION_CELL_DEG * 111_000)))
  for (const stop of stops) {
    const originY = Math.floor(stop.lat / RELATION_CELL_DEG)
    const originX = Math.floor(stop.lon / RELATION_CELL_DEG)
    for (let dy = -span; dy <= span; dy++) {
      for (let dx = -span; dx <= span; dx++) yield `${originY + dy}_${originX + dx}`
    }
  }
}

/** Complete OSM orders only; unique means unique among those candidates. */
export class CompleteTrainRouteIndex {
  private readonly routes: Array<{ relation: SourceTrainRoute; geometry: StopOrderGeometry }> = []
  private readonly cells = new Map<string, number[]>()

  constructor(stops: Iterable<Stop>) {
    for (const key of nearbyCellKeys(stops)) this.cells.set(key, [])
  }

  add(relation: SourceTrainRoute): void {
    if (relation.status !== 'complete' || relation.ways.length === 0) return
    const points = relationWayPoints(relation)
    const seen = new Set<string>()
    for (const [latitude, longitude] of points) {
      const key = cellKey(latitude, longitude)
      if (this.cells.has(key)) seen.add(key)
    }
    // Every future query is known: unrelated world itineraries never need to stay in memory.
    if (!seen.size) return
    const index = this.routes.length
    this.routes.push({ relation, geometry: new StopOrderGeometry(points) })
    for (const key of seen) this.cells.get(key)!.push(index)
  }

  associate(stops: readonly Stop[]): RailRelationAssociation {
    if (stops.length < 2) return { status: 'unmatched' }
    const nearby = new Set<number>()
    for (const key of nearbyCellKeys(stops)) {
      for (const index of this.cells.get(key) ?? []) nearby.add(index)
    }
    const fits: Array<{ relation: SourceTrainRoute; maxStopDistanceM: number }> = []
    for (const index of nearby) {
      const { relation, geometry } = this.routes[index]
      const fit = geometry.match(stops)
      if (fit) fits.push({ relation, maxStopDistanceM: fit.maxStopDistanceM })
    }
    if (fits.length === 1) {
      return { status: 'unique_complete_candidate', relation: fits[0].relation, maxStopDistanceM: fits[0].maxStopDistanceM }
    }
    return { status: fits.length ? 'ambiguous' : 'unmatched' }
  }
}
