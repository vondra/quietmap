/** Carry observed four-class traffic only through compatible unbranched source chains. */

import { isMeasured } from './sources.js'
import { SOURCE_ID_ROAD_CONTINUITY_HEURISTIC } from './source-ids.generated.js'
import { DATASETS } from './enrichment-datasets.js'
import type { RoadObservation } from './road-observation.js'
import type { PlanningRoad } from './road-planning-input.js'

export const FILLABLE = new Set(DATASETS.find(row => row.id === SOURCE_ID_ROAD_CONTINUITY_HEURISTIC)!.roadCoverage!)
export type ContinuityRoad = Pick<PlanningRoad, 'i' | 'osmId' | 'cls' | 'src' | 'a' | 'b' | 'ref' | 'name' | 'aadt' | 'access' | 'roundabout'> & RoadObservation & {
  direction: number; observationSourceId: number
}
export interface Flow extends RoadObservation { light: number; medium: number; heavy: number; moto: number; observationSourceId: number }

/** One road continues under its ref, or under its name when neither piece carries a ref. */
const sameSignedRoad = (a: ContinuityRoad, b: ContinuityRoad): boolean =>
  a.ref !== '' || b.ref !== '' ? a.ref === b.ref : a.name !== '' && a.name === b.name

function compatible(a: ContinuityRoad, b: ContinuityRoad, endpoint: string): boolean {
  if (!FILLABLE.has(b.cls) || a.cls !== b.cls || a.access !== b.access || a.roundabout || b.roundabout ||
      !(a.osmId === b.osmId || sameSignedRoad(a, b))) return false
  if (a.direction === 0 || b.direction === 0) return a.direction === b.direction
  // A permitted arrival must meet a permitted departure; two opposing one-way ends do not connect.
  return ((a.direction === 1 ? a.b : a.a) === endpoint) !== ((b.direction === 1 ? b.b : b.a) === endpoint)
}

export function roadContinuityComponent<T extends ContinuityRoad>(seed: T, touching: (endpoint: string) => T[]): T[] {
  const component = [seed], seen = new Set([seed.i])
  for (let cursor = 0; cursor < component.length; cursor++) {
    const road = component[cursor]
    if (!FILLABLE.has(road.cls) || road.a === road.b) continue
    for (const endpoint of [road.a, road.b]) {
      const incident = touching(endpoint)
      // An unsigned count on a side arm does not determine any turning movement.
      if (incident.length !== 2) continue
      const next = incident.find(candidate => candidate.i !== road.i)
      if (next && !seen.has(next.i) && compatible(road, next, endpoint)) {
        seen.add(next.i)
        component.push(next)
      }
    }
  }
  return component
}

export function planContinuityComponent(roads: ContinuityRoad[]) {
  const anchors = roads.filter(road => isMeasured(road.src) && road.observationId !== '')
  const fill = new Map<number, Flow>()
  if (!anchors.length) return { fill, anchors: 0, conflicts: 0 }
  // Without observation uncertainties there is no justified tolerance or reconciliation weight.
  if (anchors.some(road => road.countBasis !== anchors[0].countBasis || road.observationId !== anchors[0].observationId ||
      road.observationSourceId !== anchors[0].observationSourceId || road.aadt.some((value, index) => value !== anchors[0].aadt[index]))) {
    return { fill, anchors: anchors.length, conflicts: 1 }
  }
  const [light, medium, heavy, moto] = anchors[0].aadt
  const { countBasis, observationId, observationSourceId } = anchors[0]
  const flow = { light, medium, heavy, moto, countBasis, observationId, observationSourceId }
  for (const road of roads) {
    if (FILLABLE.has(road.cls) && (road.src === 0 || road.src === SOURCE_ID_ROAD_CONTINUITY_HEURISTIC)) fill.set(road.i, flow)
  }
  return { fill, anchors: anchors.length, conflicts: 0 }
}
