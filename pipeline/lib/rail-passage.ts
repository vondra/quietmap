/** Clipped source-way passages and category evidence for the railway traffic sidecar. */

import { existsSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { tableFromIPC } from 'apache-arrow'
import { isNationallyOwnedSource } from './sources.js'
import { isWalkableRailType } from './rail-graph.js'
import { bakedRailwayCountryReader, iso2Code, segmentGeometryReader } from './prepared-grid.js'
import { SourceTransportTopology, transportPieceKey } from './transport-topology.js'
import {
  inferredRailStatus, insertRailInterval, openRailTrafficSidecar, railMatchingMask,
  railStatusCode, replaceRailQuarantine, retractRailSourceSquare, type RailIntervalRow,
} from './rail-traffic-store.js'
import { type RailwayRow, type RailwayTraffic } from './railways-arrow.js'

export { railTrafficSidecarPath } from './rail-traffic-store.js'
export type RailTrafficStatus = 'known' | 'estimated' | 'unknown'
export type RailServiceMatching = 'relation_estimated' | 'graph_estimated'

/** Directed interval on one acoustic piece; repeats keep a distinct occurrence. */
export interface ClippedRailPassage {
  wayId: string
  segmentIndex: number
  square: string
  fromM: number
  toM: number
  occurrence: number
}

export interface RailCategoryEvidence {
  sourceId: number
  passenger: number
  freight: number
  passengerStatus: RailTrafficStatus
  freightStatus: RailTrafficStatus
  matching: RailServiceMatching
  relationId?: string
}

export interface RailServicePassages {
  evidence: RailCategoryEvidence
  passages: ClippedRailPassage[]
}

export interface WriteClippedRailPassagesRequest {
  preparedDirectory: string
  squares: readonly string[]
  countryIso: string
  sourceId: number
  retractSafe: boolean
  services: readonly RailServicePassages[]
  quarantinedPieceKeys: ReadonlySet<string>
  silentResidual?: Pick<RailwayTraffic, 'sourceId' | 'passenger' | 'freight'>
  extraMatch?: (row: RailwayRow, index: number, square: string) => RailwayTraffic | null
}

export interface WriteClippedRailPassagesResult {
  rows: number
  walkStamped: number
  silentStamped: number
  extraStamped: number
  retracted: number
  skippedService: number
  skippedForeign: number
  skippedForeignNational: number
}

export function transportPassageKey(passage: Pick<ClippedRailPassage, 'wayId' | 'segmentIndex'>): string {
  return transportPieceKey(passage.wayId, passage.segmentIndex)
}

function parseOsmId(wayId: string): number {
  const osmId = Number(wayId)
  if (!Number.isSafeInteger(osmId)) throw new Error(`railway way id is not an integer osm_id: ${wayId}`)
  return osmId
}

function trafficToInterval(
  square: string,
  osmId: number,
  segmentIndex: number,
  fromM: number,
  toM: number,
  occurrence: number,
  countryIso: string,
  traffic: Pick<RailwayTraffic, 'passenger' | 'freight' | 'sourceId' | 'passengerStatus' | 'freightStatus' | 'matching' | 'divisor'>,
): RailIntervalRow | null {
  const divisor = traffic.divisor && traffic.divisor > 0 ? traffic.divisor : 1
  const passenger = traffic.passenger / divisor
  const freight = traffic.freight / divisor
  const passengerStatus = inferredRailStatus(passenger, traffic.passengerStatus)
  const freightStatus = inferredRailStatus(freight, traffic.freightStatus)
  if (passengerStatus === 'unknown' && freightStatus === 'unknown') return null
  const matching = traffic.matching ?? 0
  return {
    square, osmId, segmentIndex, fromM, toM, occurrence,
    sourceId: traffic.sourceId, countryIso,
    passenger, freight,
    passengerStatus: railStatusCode(passengerStatus),
    freightStatus: railStatusCode(freightStatus),
    matching,
  }
}

/**
 * Persist clipped visits and whole-piece extra/silent stamps in the sidecar.
 * Visit ordinals are snapshot-local; changed services require retractSafe replacement.
 * Does not mutate railways.arrow — unique (osm_id, segment_idx) stays valid for routing.
 */
export async function writeClippedRailPassages(
  request: WriteClippedRailPassagesRequest,
): Promise<WriteClippedRailPassagesResult> {
  const prepared = resolve(request.preparedDirectory)
  const allowedSquares = new Set(request.squares)
  const ownSourceIds = [request.sourceId, ...(request.silentResidual ? [request.silentResidual.sourceId] : [])]
  const result: WriteClippedRailPassagesResult = {
    rows: 0, walkStamped: 0, silentStamped: 0, extraStamped: 0, retracted: 0,
    skippedService: 0, skippedForeign: 0, skippedForeignNational: 0,
  }
  const database = openRailTrafficSidecar(prepared)
  const topology = new SourceTransportTopology(prepared)
  try {
    const quarantineBySquare = new Map<string, Array<{ osmId: number; segmentIndex: number }>>()
    for (const key of request.quarantinedPieceKeys) {
      const split = key.lastIndexOf(':')
      const wayId = key.slice(0, split)
      const segmentIndex = Number(key.slice(split + 1))
      try {
        const square = topology.pieceExtent(wayId, segmentIndex).square
        if (!allowedSquares.has(square)) continue
        const quarantined = quarantineBySquare.get(square) ?? []
        quarantined.push({ osmId: parseOsmId(wayId), segmentIndex })
        quarantineBySquare.set(square, quarantined)
      } catch {
        // Piece may sit outside this country's listed squares.
      }
    }
    database.exec('BEGIN IMMEDIATE')
    for (const square of request.squares) {
      const quarantined = quarantineBySquare.get(square) ?? []
      replaceRailQuarantine(database, request.sourceId, request.countryIso, square, quarantined)
      if (request.silentResidual) {
        replaceRailQuarantine(
          database, request.silentResidual.sourceId, request.countryIso, square, quarantined,
        )
      }
      if (request.retractSafe) {
        result.retracted += retractRailSourceSquare(database, ownSourceIds, request.countryIso, square)
      }
    }

    const walkPieces = new Set<string>()
    const replaceAcceptedPiece = database.prepare(`
      DELETE FROM rail_interval WHERE source_id IN (${ownSourceIds.map(() => '?').join(',')})
        AND country_iso = ? AND square = ? AND osm_id = ? AND segment_idx = ?
    `)
    let nextOccurrence = 0
    for (const service of request.services) {
      if (service.evidence.sourceId !== request.sourceId) throw new Error('railway service source differs from snapshot owner')
      // Local visit zero on two different services represents two passages.
      const serviceOccurrenceOffset = nextOccurrence
      const matching = railMatchingMask(service.evidence.matching)
      for (const passage of service.passages) {
        const occurrence = serviceOccurrenceOffset + passage.occurrence
        if (!Number.isSafeInteger(passage.occurrence) || passage.occurrence < 0 ||
            !Number.isSafeInteger(occurrence + 1)) throw new Error('invalid railway passage occurrence')
        nextOccurrence = Math.max(nextOccurrence, occurrence + 1)
        const quarantined = request.quarantinedPieceKeys.has(transportPassageKey(passage))
        if (!allowedSquares.has(passage.square)) {
          result.skippedForeign++
          continue
        }
        const row = trafficToInterval(
          passage.square, parseOsmId(passage.wayId), passage.segmentIndex,
          passage.fromM, passage.toM, occurrence, request.countryIso, {
            passenger: service.evidence.passenger,
            freight: service.evidence.freight,
            sourceId: service.evidence.sourceId,
            passengerStatus: quarantined && service.evidence.passengerStatus !== 'unknown'
              ? 'estimated' : service.evidence.passengerStatus,
            freightStatus: quarantined && service.evidence.freightStatus !== 'unknown'
              ? 'estimated' : service.evidence.freightStatus,
            matching,
          },
        )
        if (!row || !Number.isFinite(row.fromM) || !Number.isFinite(row.toM) || row.fromM === row.toM) continue
        const key = `${passage.square}\x1f${transportPassageKey(passage)}`
        if (quarantined && !walkPieces.has(key)) {
          // A current accepted snapshot replaces old visits once, even where another
          // failed service left quarantine. Keep that uncertainty and its fallback veto.
          result.retracted += Number(replaceAcceptedPiece.run(
            ...ownSourceIds, request.countryIso, passage.square, row.osmId, row.segmentIndex,
          ).changes)
        }
        insertRailInterval(database, row)
        if (!walkPieces.has(key)) {
          walkPieces.add(key)
          result.walkStamped++
        }
      }
    }

    for (const square of request.squares) {

      const arrowPath = resolve(prepared, square, 'railways.arrow')
      if (!existsSync(arrowPath)) continue
      const table = tableFromIPC(readFileSync(arrowPath))
      const rows = table.numRows
      result.rows += rows
      if (rows === 0 || (!request.silentResidual && !request.extraMatch)) continue
      const geometry = segmentGeometryReader(table)
      const osmId = table.getChild('osm_id')!
      const segmentIndex = table.getChild('segment_idx')!
      const railType = table.getChild('rail_type')!
      const serviceCol = table.getChild('service')!
      const existingSource = table.getChild('source_id')!
      const names = table.getChild('name')!
      const countries = bakedRailwayCountryReader(table)
      const allowedCountry = iso2Code(request.countryIso)

      for (let index = 0; index < rows; index++) {
        if ((serviceCol.get(index) as number) > 0) {
          result.skippedService++
          continue
        }
        if (countries.codeAt(index) !== allowedCountry) {
          result.skippedForeign++
          continue
        }
        const wayId = String(osmId.get(index))
        const segment = segmentIndex.get(index) as number
        const key = transportPieceKey(wayId, segment)
        if (walkPieces.has(`${square}\x1f${key}`) || request.quarantinedPieceKeys.has(key)) continue
        const existing = existingSource.get(index) as number
        const row: RailwayRow = {
          ...geometry.row(index),
          osmId: wayId,
          segmentIndex: segment,
          railType: railType.get(index) as number,
          usage: table.getChild('usage')!.get(index) as number,
          service: 0,
          name: (names.get(index) as string | null) ?? '',
          existingSourceId: existing,
          existingPassenger: 0,
          existingFreight: 0,
          existingDivisor: 1,
        }
        const silent = request.silentResidual && isWalkableRailType(row.railType)
        const candidate = silent
          ? { ...request.silentResidual!, passengerStatus: 'estimated' as const, freightStatus: 'estimated' as const }
          : request.extraMatch?.(row, index, square) ?? null
        if (!candidate) continue
        if (!ownSourceIds.includes(existing) && isNationallyOwnedSource(existing)) {
          result.skippedForeignNational++
          continue
        }
        const extent = topology.pieceExtent(wayId, segment)
        const interval = trafficToInterval(
          square, parseOsmId(wayId), segment, extent.from, extent.to, 0, request.countryIso, candidate,
        )
        if (!interval) continue
        insertRailInterval(database, interval)
        if (silent) result.silentStamped++
        else result.extraStamped++
      }
    }
    database.exec('COMMIT')
  } catch (error) {
    try { database.exec('ROLLBACK') } catch { /* transaction may not have started */ }
    throw error
  } finally {
    topology[Symbol.dispose]()
    database.close()
  }
  return result
}
