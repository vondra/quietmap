/** Whole-piece railway evidence writer for the final preparation sidecar. */

import { dirname, basename, resolve } from 'node:path'
import { DataType, type Table, type Vector } from 'apache-arrow'
import { SOURCES_BY_ID, countryIsosForNationalSource } from './sources.js'
import {
  bakedRailwayCountryReader, iso2Code, segmentGeometryReader, type SegmentGeometry,
} from './prepared-grid.js'
import { SourceTransportTopology } from './transport-topology.js'
import { restoreRailwayParentsForEnrichment } from './rail-parent.js'
import {
  inferredRailStatus, insertRailInterval, openRailTrafficSidecar, railMatchingMask,
  railStatusCode, retractRailSourceSquare,
} from './rail-traffic-store.js'
import type { RailTrafficStatus } from './rail-passage.js'

export interface RailwayTraffic {
  passenger: number
  freight: number
  sourceId: number
  divisor?: number
  passengerStatus?: RailTrafficStatus
  freightStatus?: RailTrafficStatus
  matching?: number
}

export interface RailwayRow extends SegmentGeometry {
  osmId: string
  segmentIndex: number
  railType: number
  usage: number
  service: number
  name: string
  existingSourceId: number
  existingPassenger: number
  existingFreight: number
  existingDivisor: number
}

export interface RailwayRetract {
  sourceIds: readonly number[]
  when: (row: RailwayRow, index: number) => boolean
}

export interface RailwayWriteOptions {
  retract?: RailwayRetract
  allowedCountryIsos?: readonly string[]
  countryIso?: string
}

export interface WriteRailwayResult {
  rows: number
  matched: number
  updated: boolean
  skippedService: number
  skippedForeign: number
  retracted: number
}

export function preparedDirectoryFromRailwayArrow(arrowPath: string): { preparedDirectory: string; square: string } {
  const file = resolve(arrowPath)
  if (basename(file) !== 'railways.arrow') {
    throw new Error(`railway Arrow path is not z9/x/y/railways.arrow: ${arrowPath}`)
  }
  const y = dirname(file)
  const x = dirname(y)
  const z9 = dirname(x)
  if (basename(z9) !== 'z9') throw new Error(`railway Arrow path is not z9/x/y/railways.arrow: ${arrowPath}`)
  return { preparedDirectory: dirname(z9), square: `z9/${basename(x)}/${basename(y)}` }
}

const topologyCache: { prepared?: string; topology?: SourceTransportTopology } = {}

function topologyFor(preparedDirectory: string): SourceTransportTopology {
  if (topologyCache.prepared === preparedDirectory && topologyCache.topology) return topologyCache.topology
  topologyCache.topology?.[Symbol.dispose]()
  topologyCache.prepared = preparedDirectory
  topologyCache.topology = new SourceTransportTopology(preparedDirectory)
  return topologyCache.topology
}

function requiredInteger(table: Table, name: string, signed: boolean, bitWidth: number): Vector {
  const vector = table.getChild(name)
  if (!vector || !DataType.isInt(vector.type) || vector.type.isSigned !== signed ||
      vector.type.bitWidth !== bitWidth || vector.nullCount !== 0) {
    throw new Error(`railways Arrow '${name}' must be non-null ${signed ? 'Int' : 'Uint'}${bitWidth}`)
  }
  return vector
}

function optionalInteger(table: Table, name: string, signed: boolean, bitWidth: number): Vector | null {
  const vector = table.getChild(name)
  if (!vector) return null
  if (!DataType.isInt(vector.type) || vector.type.isSigned !== signed ||
      vector.type.bitWidth !== bitWidth || vector.nullCount !== 0) {
    throw new Error(`railways Arrow '${name}' must be non-null ${signed ? 'Int' : 'Uint'}${bitWidth}`)
  }
  return vector
}

function assertTraffic(value: RailwayTraffic, index: number, path: string): void {
  if (!Number.isFinite(value.passenger) || value.passenger < 0 ||
      !Number.isFinite(value.freight) || value.freight < 0 ||
      !Number.isInteger(value.sourceId) || value.sourceId <= 0 || value.sourceId > 0xffff ||
      (value.divisor !== undefined &&
        (!Number.isInteger(value.divisor) || value.divisor < 1 || value.divisor > 0xff))) {
    throw new Error(`writeRailwayTraffic: invalid match at row ${index} in ${path}: ${JSON.stringify(value)}`)
  }
  if (SOURCES_BY_ID.get(value.sourceId)?.layer !== 'railways') {
    throw new Error(`writeRailwayTraffic: sourceId ${value.sourceId} is not a registered railways source`)
  }
}

function validateRetract(retract: RailwayRetract | undefined): ReadonlySet<number> {
  const ids = new Set(retract?.sourceIds ?? [])
  for (const id of ids) {
    if (SOURCES_BY_ID.get(id)?.layer !== 'railways') {
      throw new Error(`writeRailwayTraffic: retract sourceId ${id} is not a registered railways source`)
    }
  }
  return ids
}

function writerCountryIso(options: RailwayWriteOptions): string {
  if (options.countryIso) return options.countryIso
  if (options.allowedCountryIsos?.length === 1) return options.allowedCountryIsos[0]
  throw new Error('writeRailwayTraffic: countryIso is required for sidecar evidence')
}

/**
 * Persist whole-piece daily evidence in the rail-traffic sidecar.
 * Restores finalized parents when needed; traffic is written only to the sidecar.
 */
export async function writeRailwayTraffic(
  arrowPath: string,
  match: (row: RailwayRow, index: number) => RailwayTraffic | null,
  onApplied?: (row: RailwayRow, index: number, applied: RailwayTraffic) => void,
  options: RailwayWriteOptions = {},
): Promise<WriteRailwayResult> {
  const { preparedDirectory, square } = preparedDirectoryFromRailwayArrow(arrowPath)
  const countryIso = writerCountryIso(options)
  const result: WriteRailwayResult = {
    rows: 0, matched: 0, updated: false,
    skippedService: 0, skippedForeign: 0, retracted: 0,
  }
  const retractIds = validateRetract(options.retract)
  const allowedCountryCodes = options.allowedCountryIsos
    ? new Set(options.allowedCountryIsos.map(iso2Code))
    : null
  const topology = topologyFor(preparedDirectory)
  const table = restoreRailwayParentsForEnrichment(arrowPath, preparedDirectory, square, topology)
  const database = openRailTrafficSidecar(preparedDirectory)
  try {
    result.rows = table.numRows
    if (result.rows === 0) return result

    const geometry = segmentGeometryReader(table)
    const osmId = requiredInteger(table, 'osm_id', true, 64)
    const segmentIndex = requiredInteger(table, 'segment_idx', true, 16)
    const railType = requiredInteger(table, 'rail_type', false, 8)
    const usage = requiredInteger(table, 'usage', false, 8)
    const service = requiredInteger(table, 'service', false, 8)
    const existingSource = requiredInteger(table, 'source_id', false, 16)
    const existingPassenger = optionalInteger(table, 'trains_passenger', true, 32)
    const existingFreight = optionalInteger(table, 'trains_freight', true, 32)
    const existingDivisor = optionalInteger(table, 'parallel_divisor', false, 8)
    const names = table.getChild('name')
    if (!names || !DataType.isUtf8(names.type)) throw new Error("railways Arrow 'name' must be Utf8")
    const countries = bakedRailwayCountryReader(table)

    const acceptedCountryCodes = new Map<number, ReadonlySet<number> | null>()
    const countryCodesFor = (sourceId: number): ReadonlySet<number> | null => {
      if (acceptedCountryCodes.has(sourceId)) return acceptedCountryCodes.get(sourceId)!
      const isos = countryIsosForNationalSource(sourceId)
      const codes = isos === null ? null : new Set(isos.map(iso2Code))
      acceptedCountryCodes.set(sourceId, codes)
      return codes
    }

    database.exec('BEGIN IMMEDIATE')
    if (options.retract) {
      for (let index = 0; index < result.rows; index++) {
        const rowSource = existingSource.get(index) as number
        const inAllowedCountry = allowedCountryCodes === null || allowedCountryCodes.has(countries.codeAt(index))
        const row: RailwayRow = {
          ...geometry.row(index),
          osmId: String(osmId.get(index)),
          segmentIndex: segmentIndex.get(index) as number,
          railType: railType.get(index) as number,
          usage: usage.get(index) as number,
          service: service.get(index) as number,
          name: (names.get(index) as string | null) ?? '',
          existingSourceId: rowSource,
          existingPassenger: (existingPassenger?.get(index) as number) ?? 0,
          existingFreight: (existingFreight?.get(index) as number) ?? 0,
          existingDivisor: (existingDivisor?.get(index) as number) ?? 1,
        }
        if (inAllowedCountry && (row.service > 0 || options.retract.when(row, index))) {
          result.retracted += retractRailSourceSquare(
            database, [...retractIds], countryIso, square,
            { osmId: Number(row.osmId), segmentIndex: row.segmentIndex },
          )
        }
      }
    }

    for (let index = 0; index < result.rows; index++) {
      const row: RailwayRow = {
        ...geometry.row(index),
        osmId: String(osmId.get(index)),
        segmentIndex: segmentIndex.get(index) as number,
        railType: railType.get(index) as number,
        usage: usage.get(index) as number,
        service: service.get(index) as number,
        name: (names.get(index) as string | null) ?? '',
        existingSourceId: existingSource.get(index) as number,
        existingPassenger: (existingPassenger?.get(index) as number) ?? 0,
        existingFreight: (existingFreight?.get(index) as number) ?? 0,
        existingDivisor: (existingDivisor?.get(index) as number) ?? 1,
      }
      const inAllowedCountry = allowedCountryCodes === null || allowedCountryCodes.has(countries.codeAt(index))
      if (row.service > 0) {
        result.skippedService++
        continue
      }
      if (!inAllowedCountry) {
        result.skippedForeign++
        continue
      }
      const candidate = match(row, index)
      if (!candidate) continue
      assertTraffic(candidate, index, arrowPath)
      const expectedCountries = countryCodesFor(candidate.sourceId)
      if (expectedCountries !== null && !expectedCountries.has(countries.codeAt(index))) {
        result.skippedForeign++
        continue
      }
      const extent = topology.pieceExtent(row.osmId, row.segmentIndex)
      const divisor = candidate.divisor && candidate.divisor > 0 ? candidate.divisor : 1
      const passenger = candidate.passenger / divisor
      const freight = candidate.freight / divisor
      const passengerStatus = inferredRailStatus(passenger, candidate.passengerStatus)
      const freightStatus = inferredRailStatus(freight, candidate.freightStatus)
      if (passengerStatus === 'unknown' && freightStatus === 'unknown') continue
      insertRailInterval(database, {
        square, osmId: Number(row.osmId), segmentIndex: row.segmentIndex,
        fromM: extent.from, toM: extent.to, occurrence: 0,
        sourceId: candidate.sourceId, countryIso,
        passenger, freight,
        passengerStatus: railStatusCode(passengerStatus),
        freightStatus: railStatusCode(freightStatus),
        matching: candidate.matching ?? railMatchingMask(undefined),
      })
      result.matched++
      result.updated = true
      onApplied?.(row, index, candidate)
    }
    database.exec('COMMIT')
  } catch (error) {
    try { database.exec('ROLLBACK') } catch { /* no transaction */ }
    throw error
  } finally {
    database.close()
  }
  return result
}
