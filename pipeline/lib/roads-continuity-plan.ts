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
/** What decides a link at one endpoint; the only road facts a square sends for a chain end it cannot close itself. */
export type ContinuityChainEnd = Pick<ContinuityRoad, 'osmId' | 'cls' | 'access' | 'roundabout' | 'ref' | 'name' | 'direction' | 'a' | 'b'>
/** Anchors of one chain or of a part of it: their agreed flow, or one conflict. */
export interface AnchorAgreement { anchors: number; conflicts: number; flow?: Flow }

/** One road continues under its ref, or under its name when neither piece carries a ref. */
const sameSignedRoad = (a: ContinuityChainEnd, b: ContinuityChainEnd): boolean =>
  a.ref !== '' || b.ref !== '' ? a.ref === b.ref : a.name !== '' && a.name === b.name

/** Symmetric, and both pieces must be fillable; the caller establishes that exactly these two piece ends meet at the endpoint. */
export function chainContinuesThroughEndpoint(a: ContinuityChainEnd, b: ContinuityChainEnd, endpoint: string): boolean {
  if (!FILLABLE.has(b.cls) || a.cls !== b.cls || a.access !== b.access || a.roundabout || b.roundabout ||
      !(a.osmId === b.osmId || sameSignedRoad(a, b))) return false
  if (a.direction === 0 || b.direction === 0) return a.direction === b.direction
  // A permitted arrival must meet a permitted departure; two opposing one-way ends do not connect.
  return ((a.direction !== 2 ? a.b : a.a) === endpoint) !== ((b.direction !== 2 ? b.b : b.a) === endpoint)
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
      if (next && !seen.has(next.i) && chainContinuesThroughEndpoint(road, next, endpoint)) {
        seen.add(next.i)
        component.push(next)
      }
    }
  }
  return component
}

export const isContinuityAnchor = (road: ContinuityRoad): boolean => isMeasured(road.src) && road.observationId !== ''
export const isContinuityFillTarget = (road: Pick<ContinuityRoad, 'cls' | 'src'>): boolean =>
  FILLABLE.has(road.cls) && (road.src === 0 || road.src === SOURCE_ID_ROAD_CONTINUITY_HEURISTIC)

// Without observation uncertainties there is no justified tolerance or reconciliation weight.
const sameFlow = (a: Flow, b: Flow): boolean => a.countBasis === b.countBasis && a.observationId === b.observationId &&
  a.observationSourceId === b.observationSourceId && a.light === b.light && a.medium === b.medium && a.heavy === b.heavy && a.moto === b.moto

/** The parts of one chain agree only when every anchor in every part carries the same observation and counts. */
export function joinAnchorAgreements(parts: Iterable<AnchorAgreement>): AnchorAgreement {
  let anchors = 0, conflicts = 0, flow: Flow | undefined
  for (const part of parts) {
    anchors += part.anchors
    if (part.conflicts || (flow && part.flow && !sameFlow(flow, part.flow))) conflicts = 1
    flow ??= part.flow
  }
  return conflicts ? { anchors, conflicts } : { anchors, conflicts, flow }
}

export function anchorAgreement(roads: ContinuityRoad[]): AnchorAgreement {
  return joinAnchorAgreements(roads.filter(isContinuityAnchor).map(({ aadt: [light, medium, heavy, moto], countBasis, observationId, observationSourceId }) =>
    ({ anchors: 1, conflicts: 0, flow: { light, medium, heavy, moto, countBasis, observationId, observationSourceId } })))
}

export function planContinuityComponent(roads: ContinuityRoad[]) {
  const { anchors, conflicts, flow } = anchorAgreement(roads), fill = new Map<number, Flow>()
  if (flow) for (const road of roads) if (isContinuityFillTarget(road)) fill.set(road.i, flow)
  return { fill, anchors, conflicts }
}
