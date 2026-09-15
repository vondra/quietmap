/** Join acoustic railway rows to their original OSM connectivity before matching traffic. */

import { resolve } from 'node:path'
import { type Table, type Vector } from 'apache-arrow'
import { restoreRailwayParentsForEnrichment } from './rail-parent.js'
import { listPreparedSquares, segmentGeometryReader, type PreparedBbox } from './prepared-grid.js'
import { buildRailGraph, isWalkableRailType, type RailGraphSegmentInput, type RailStationPairCount, type RailFailedPairRecord } from './rail-graph.js'
import { walkRailStationPairs } from './rail-graph-metrics.js'
import { writeRailwayTraffic, type RailwayRow, type RailwayTraffic } from './railways-arrow.js'
import { writeClippedRailPassages } from './rail-passage.js'
import { routeRailServices, type RailServiceRoutingCounts } from './rail-service-route.js'
import { isNationallyOwnedSource } from './sources.js'
import { SourceTransportTopology, transportPieceKey } from './transport-topology.js'
import type { GtfsService } from './gtfs-service-store.js'

function requiredVector(table: Table, name: string): Vector {
  const vector = table.getChild(name)
  if (!vector) throw new Error(`railways Arrow missing '${name}'`)
  return vector
}

/** Read graph rows once; all topology/routing remains pure in rail-graph*.ts. */
export function collectZ9RailGraphSegments(
  preparedDirectory: string,
  squares: readonly string[],
  topology?: SourceTransportTopology,
): RailGraphSegmentInput[] {
  const owned = topology ? undefined : new SourceTransportTopology(preparedDirectory)
  const source = topology ?? owned!
  try {
    const segments: RailGraphSegmentInput[] = []
    for (const square of squares) {
      const path = resolve(preparedDirectory, square, 'railways.arrow')
      const table = restoreRailwayParentsForEnrichment(path, preparedDirectory, square, source)
      const geometry = segmentGeometryReader(table)
      const railType = requiredVector(table, 'rail_type')
      const usage = requiredVector(table, 'usage')
      const service = requiredVector(table, 'service')
      const length = requiredVector(table, 'length_m')
      const name = requiredVector(table, 'name')
      const ref = requiredVector(table, 'ref')
      const osmId = requiredVector(table, 'osm_id')
      const segmentIndex = requiredVector(table, 'segment_idx')
      const identities = source.squareWayPieces(square, Array.from(osmId, value => String(value)))

      const rows = table.numRows
      for (let index = 0; index < rows; index++) {
        const type = railType.get(index) as number
        const serviceCode = service.get(index) as number
        const isTraversalOnly = serviceCode === 4
        if (!isTraversalOnly && !(isWalkableRailType(type) && serviceCode === 0)) continue
        const row = geometry.row(index)
        const corridorRef = (ref.get(index) as string | null) ?? ''
        const corridorName = (name.get(index) as string | null) ?? ''
        const key = transportPieceKey(String(osmId.get(index)), segmentIndex.get(index) as number)
        const identity = identities.get(key)
        if (!identity) throw new Error(`source topology missing or repeated railway piece ${key} in ${square}`)
        identities.delete(key)
        segments.push({
          ...identity,
          key,
          osmId: String(osmId.get(index)),
          railType: type,
          usage: usage.get(index) as number,
          isTraversalOnly,
          corridorToken: corridorRef || corridorName,
          startLat: row.startLat,
          startLon: row.startLon,
          endLat: row.endLat,
          endLon: row.endLon,
          lengthM: length.get(index) as number,
        })
      }
    }
    return segments
  } finally {
    owned?.[Symbol.dispose]()
  }
}

export interface Z9RailWalkOptions {
  preparedDirectory: string
  bbox: PreparedBbox
  pairs: readonly RailStationPairCount[]
  sourceId: number
  countryIso: string
  silentResidual?: Pick<RailwayTraffic, 'sourceId' | 'passenger' | 'freight'>
  extraMatch?: (row: RailwayRow, index: number, square: string) => RailwayTraffic | null
  /** Only a complete current feed snapshot may disown earlier stamps. */
  retractSafe: boolean
}

export interface Z9RailWalkResult {
  squares: number
  rows: number
  walkStamped: number
  silentStamped: number
  extraStamped: number
  retracted: number
  skippedService: number
  skippedForeign: number
  skippedForeignNational: number
  pairsWalked: number
  pairsTotal: number
  servicesTotal: number
  servicesRelationEstimated: number
  servicesGraphEstimated: number
  servicesUnmatched: number
  serviceDailyDepartures: RailServiceRoutingCounts | null
  failedPairs: RailFailedPairRecord[]
  unlocalizedPairs: number
  failures: {
    snapFailed: number
    disconnected: number
    detourRejected: number
    ambiguous: number
  }
  quarantinedKilometres: number
  stampableKilometres: number
}

export async function enrichZ9RailwaysByGraphWalk(
  options: Z9RailWalkOptions,
): Promise<Z9RailWalkResult> {
  const prepared = resolve(options.preparedDirectory)
  const squares = listPreparedSquares(prepared, options.bbox, 'railways.arrow')
  if (squares.length === 0) {
    throw new Error(`no railways.arrow squares found for bbox ${options.bbox.join(',')}`)
  }
  const segments = collectZ9RailGraphSegments(prepared, squares)
  const walk = walkRailStationPairs(buildRailGraph(segments), [...options.pairs])

  const result: Z9RailWalkResult = {
    squares: squares.length,
    rows: 0,
    walkStamped: 0,
    silentStamped: 0,
    extraStamped: 0,
    retracted: 0,
    skippedService: 0,
    skippedForeign: 0,
    skippedForeignNational: 0,
    pairsWalked: walk.pairsWalked,
    pairsTotal: walk.pairsTotal,
    servicesTotal: 0,
    servicesRelationEstimated: 0,
    servicesGraphEstimated: 0,
    servicesUnmatched: 0,
    serviceDailyDepartures: null,
    failedPairs: walk.failedPairChords,
    unlocalizedPairs: walk.unlocalizedPairs,
    failures: walk.failures,
    quarantinedKilometres: 0,
    stampableKilometres: 0,
  }
  for (const segment of segments) {
    if (segment.isTraversalOnly) continue
    result.stampableKilometres += segment.lengthM / 1000
    if (walk.quarantinedSegmentKeys.has(segment.key)) {
      result.quarantinedKilometres += segment.lengthM / 1000
    }
  }

  const ownSourceIds = [options.sourceId, ...(options.silentResidual ? [options.silentResidual.sourceId] : [])]
  for (const square of squares) {
    const branch = new Map<number, 'walk' | 'silent' | 'extra'>()
    const write = await writeRailwayTraffic(
      resolve(prepared, square, 'railways.arrow'),
      (row, index) => {
        const key = transportPieceKey(row.osmId, row.segmentIndex)
        const stamp = walk.stampsBySegmentKey.get(key)
        const silent = !stamp && options.silentResidual && isWalkableRailType(row.railType) &&
          !walk.quarantinedSegmentKeys.has(key)
        const candidate = stamp
          ? {
              passenger: stamp.pax,
              freight: stamp.frt,
              sourceId: options.sourceId,
              divisor: stamp.divisor,
            }
          : silent ? { ...options.silentResidual!, divisor: walk.divisorBySegmentKey.get(key) ?? 1 }
          : options.extraMatch?.(row, index, square) ?? null
        if (!candidate) return null
        if (!ownSourceIds.includes(row.existingSourceId) &&
            isNationallyOwnedSource(row.existingSourceId)) {
          result.skippedForeignNational++
          return null
        }
        branch.set(index, stamp ? 'walk' : silent ? 'silent' : 'extra')
        return candidate
      },
      (_row, index) => {
        if (branch.get(index) === 'walk') result.walkStamped++
        else if (branch.get(index) === 'silent') result.silentStamped++
        else if (branch.get(index) === 'extra') result.extraStamped++
      },
      {
        allowedCountryIsos: [options.countryIso],
        countryIso: options.countryIso,
        retract: options.retractSafe ? {
            sourceIds: ownSourceIds,
            when: (row) => {
              const key = transportPieceKey(row.osmId, row.segmentIndex)
              return !walk.stampsBySegmentKey.has(key) &&
                !walk.quarantinedSegmentKeys.has(key)
            },
          } : undefined,
      },
    )
    result.rows += write.rows
    result.retracted += write.retracted
    result.skippedService += write.skippedService
    result.skippedForeign += write.skippedForeign
  }
  return result
}

export interface Z9RailServiceOptions {
  preparedDirectory: string
  bbox: PreparedBbox
  services: Iterable<GtfsService>
  sourceId: number
  countryIso: string
  silentResidual?: Pick<RailwayTraffic, 'sourceId' | 'passenger' | 'freight'>
  extraMatch?: (row: RailwayRow, index: number, square: string) => RailwayTraffic | null
  retractSafe: boolean
  beforeWrite?: () => Promise<void>
}

export async function enrichZ9RailwaysByServices(
  options: Z9RailServiceOptions,
): Promise<Z9RailWalkResult> {
  const prepared = resolve(options.preparedDirectory)
  const squares = listPreparedSquares(prepared, options.bbox, 'railways.arrow')
  if (squares.length === 0) {
    throw new Error(`no railways.arrow squares found for bbox ${options.bbox.join(',')}`)
  }
  using topology = new SourceTransportTopology(prepared)
  const segments = collectZ9RailGraphSegments(prepared, squares, topology)
  const routed = routeRailServices(options.services, topology, buildRailGraph(segments), options.sourceId)
  let stampableKilometres = 0, quarantinedKilometres = 0
  for (const segment of segments) {
    if (segment.isTraversalOnly) continue
    stampableKilometres += segment.lengthM / 1000
    if (routed.quarantinedPieceKeys.has(segment.key)) quarantinedKilometres += segment.lengthM / 1000
  }
  await options.beforeWrite?.()
  const write = await writeClippedRailPassages({
    preparedDirectory: prepared,
    squares,
    countryIso: options.countryIso,
    sourceId: options.sourceId,
    retractSafe: options.retractSafe,
    services: routed.services,
    quarantinedPieceKeys: routed.quarantinedPieceKeys,
    silentResidual: options.silentResidual,
    extraMatch: options.extraMatch,
  })
  return {
    squares: squares.length,
    ...write,
    pairsWalked: routed.relationEstimated + routed.graphEstimated,
    pairsTotal: routed.total,
    servicesTotal: routed.total,
    servicesRelationEstimated: routed.relationEstimated,
    servicesGraphEstimated: routed.graphEstimated,
    servicesUnmatched: routed.unmatched,
    serviceDailyDepartures: routed.dailyDepartures,
    failedPairs: [],
    unlocalizedPairs: 0,
    failures: { ...routed.failures, detourRejected: 0 },
    quarantinedKilometres,
    stampableKilometres,
  }
}
