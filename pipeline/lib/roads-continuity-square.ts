/** One square's share of road continuity: chains it closes itself, and the chain parts that reach an endpoint another square may touch. */

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { DataType, tableFromIPC } from 'apache-arrow'
import { readPlanningRoads } from './road-planning-input.js'
import {
  FILLABLE, anchorAgreement, isContinuityAnchor, isContinuityFillTarget, planContinuityComponent, roadContinuityComponent,
  type AnchorAgreement, type ContinuityChainEnd, type ContinuityRoad, type Flow,
} from './roads-continuity-plan.js'
import { SOURCE_ID_ROAD_CONTINUITY_HEURISTIC } from './source-ids.generated.js'
import { SquarePieces, transportPieceKey } from './transport-topology.js'

export interface ContinuityFill { rows: Int32Array; flow: Flow }
/** Every piece end one square has at a shared endpoint; `chainEnds` names the fillable, non-loop ones by their chain part. */
export interface SharedEndpoint { key: string; pieceEnds: number; chainEnds: Array<{ part: number; road: ContinuityChainEnd }> }
export interface SharedChainPart { agreement: AnchorAgreement; fillRows: Int32Array }
export interface SquareContinuity {
  rows: number; hasOwned: boolean; anchors: number; conflicts: number
  fills: ContinuityFill[]; sharedEndpoints: SharedEndpoint[]; sharedChainParts: SharedChainPart[]
}

const SIDES = ['start', 'end'] as const

/**
 * Endpoint keys whose z30 cell lies in another square, grouped by that home square. Pieces of two squares can meet
 * only at such an endpoint: the home square learns of it here, and every other square sees the foreign cell itself.
 */
export function endpointKeysHomedInOtherSquares(prepared: string, square: string): Map<string, string[]> {
  const pieces = SquarePieces.read(prepared, square, 'roads'), keysByHome = new Map<string, string[]>()
  for (let row = 0; row < pieces.count; row++) for (const side of SIDES) {
    const home = pieces.endpointHomeInAnotherSquare(row, side)
    if (home === null) continue
    const keys = keysByHome.get(home)
    if (keys) keys.push(pieces.endpointKey(row, side))
    else keysByHome.set(home, [pieces.endpointKey(row, side)])
  }
  return keysByHome
}

interface Endpoint { shared: boolean; incident: ContinuityRoad[] }

/** `keysTouchedFromOtherSquares` is what `endpointKeysHomedInOtherSquares` of all other squares named for this one. */
export function planSquareContinuity(prepared: string, square: string, keysTouchedFromOtherSquares: readonly string[]): SquareContinuity {
  const table = tableFromIPC(readFileSync(resolve(prepared, square, 'roads.arrow')))
  if (table.schema.metadata.get('road_traffic_contract') === '1') throw new Error('road continuity must precede final allocation')
  const direction = table.getChild('oneway')
  if (!direction || !DataType.isInt(direction.type) || direction.type.bitWidth !== 8 || direction.type.isSigned || direction.nullCount) {
    throw new Error(`${square}: invalid oneway direction column`)
  }
  const pieces = SquarePieces.read(prepared, square, 'roads'), seen = new Uint8Array(pieces.count)
  const touchedFromOtherSquares = new Set(keysTouchedFromOtherSquares), endpoints = new Map<string, Endpoint>()
  const roads = readPlanningRoads(table), seeds: ContinuityRoad[] = []
  for (const planningRoad of roads) {
    const piece = pieces.row(String(planningRoad.osmId), planningRoad.segIdx)
    if (piece < 0 || seen[piece]) {
      throw new Error(`${square}: source topology missing or repeated road piece ${transportPieceKey(String(planningRoad.osmId), planningRoad.segIdx)}`)
    }
    seen[piece] = 1
    const oneway = Number(direction.get(planningRoad.i))
    if (oneway > 2) throw new Error(`${square}: invalid oneway direction ${oneway}`)
    const { startKey, endKey } = pieces.identity(piece)
    // Source topology replaces the snapped coordinates: equal cells do not connect distinct OSM nodes.
    const road: ContinuityRoad = Object.assign(planningRoad, { a: startKey, b: endKey, direction: oneway })
    for (const side of SIDES) {
      const key = side === 'start' ? startKey : endKey
      let endpoint = endpoints.get(key)
      if (!endpoint) endpoints.set(key, endpoint = { incident: [],
        shared: pieces.endpointHomeInAnotherSquare(piece, side) !== null || touchedFromOtherSquares.has(key) })
      // A non-fillable piece and both ends of a loop still count toward the branch degree; three settle every question.
      if (endpoint.incident.length < 3) endpoint.incident.push(road)
    }
    // A chain matters when it holds an observation, or when another square may continue it to one.
    if (FILLABLE.has(road.cls) && (isContinuityAnchor(road) ||
        (startKey !== endKey && [startKey, endKey].some(key => endpoints.get(key)!.shared)))) seeds.push(road)
  }
  // Holds only before roads-finalize, which drops subpixel pieces from the layer file; the build audit checks the other direction.
  if (roads.length !== pieces.count) throw new Error(`${square}: ${pieces.count - roads.length} source road pieces absent from Arrow`)

  const result: SquareContinuity = { rows: roads.length, anchors: 0, conflicts: 0, fills: [], sharedEndpoints: [], sharedChainParts: [],
    hasOwned: roads.some(road => road.src === SOURCE_ID_ROAD_CONTINUITY_HEURISTIC) }
  const sharedEndpoints = new Map<string, SharedEndpoint>(), walked = new Set<number>()
  for (const [key, endpoint] of endpoints) {
    if (endpoint.shared) sharedEndpoints.set(key, { key, pieceEnds: endpoint.incident.length, chainEnds: [] })
  }
  // Only the parent knows the degree of a shared endpoint, so no chain continues through one here.
  const touching = (key: string): ContinuityRoad[] => {
    const endpoint = endpoints.get(key)!
    return endpoint.shared ? [] : endpoint.incident
  }
  for (const seed of seeds) {
    if (walked.has(seed.i)) continue
    const chain = roadContinuityComponent(seed, touching)
    let reachesSharedEndpoint = false
    for (const road of chain) {
      walked.add(road.i)
      if (road.a !== road.b) for (const key of [road.a, road.b]) {
        const shared = sharedEndpoints.get(key)
        if (!shared || shared.pieceEnds > 2) continue
        reachesSharedEndpoint = true
        const { osmId, cls, access, roundabout, ref, name, direction, a, b } = road
        shared.chainEnds.push({ part: result.sharedChainParts.length, road: { osmId, cls, access, roundabout, ref, name, direction, a, b } })
      }
    }
    if (reachesSharedEndpoint) {
      result.sharedChainParts.push({ agreement: anchorAgreement(chain),
        fillRows: Int32Array.from(chain.filter(isContinuityFillTarget), road => road.i) })
      continue
    }
    const plan = planContinuityComponent(chain)
    result.anchors += plan.anchors; result.conflicts += plan.conflicts
    if (plan.fill.size) result.fills.push({ rows: Int32Array.from(plan.fill.keys()), flow: plan.fill.values().next().value! })
  }
  result.sharedEndpoints = [...sharedEndpoints.values()]
  return result
}
