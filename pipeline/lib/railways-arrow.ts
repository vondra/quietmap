/** Atomic traffic and policy-free divisor writers for z9/z30 railway Arrow data. */

import { DataType, makeTable, makeVector, type Table, type Vector } from 'apache-arrow'
import { withArrowWrite } from './provenance.js'
import { SOURCES_BY_ID, countryIsosForNationalSource, shouldOverwrite } from './sources.js'
import {
  bakedRailwayCountryReader, iso2Code, segmentGeometryReader, type SegmentGeometry,
} from './prepared-grid.js'

export interface RailwayTraffic {
  passenger: number
  freight: number
  sourceId: number
  divisor?: number
}

export interface RailwayRow extends SegmentGeometry {
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
  /** Baked admin owners this scoped run is allowed to mutate. */
  allowedCountryIsos?: readonly string[]
}

export interface WriteRailwayResult {
  rows: number
  matched: number
  updated: boolean
  skippedService: number
  skippedForeign: number
  skippedPriority: number
  retracted: number
}

export interface WriteRailParallelDivisorResult {
  rows: number
  changedRows: number
  updated: boolean
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
  if (!Number.isSafeInteger(value.passenger) || value.passenger < 0 || value.passenger > 2_147_483_647 ||
      !Number.isSafeInteger(value.freight) || value.freight < 0 || value.freight > 2_147_483_647 ||
      !Number.isInteger(value.sourceId) || value.sourceId <= 0 || value.sourceId > 0xffff ||
      (value.divisor !== undefined &&
        (!Number.isInteger(value.divisor) || value.divisor < 1 || value.divisor > 0xff))) {
    throw new Error(`writeRailwayTraffic: invalid match at row ${index} in ${path}: ${JSON.stringify(value)}`)
  }
  if (value.passenger === 0 && value.freight === 0) {
    throw new Error(`writeRailwayTraffic: all-zero traffic at row ${index} in ${path}`)
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

/**
 * Replace counts, provenance and an optional parallel divisor together.
 * A retract disowns only its declared sources and resets the whole payload.
 */
export async function writeRailwayTraffic(
  arrowPath: string,
  match: (row: RailwayRow, index: number) => RailwayTraffic | null,
  onApplied?: (row: RailwayRow, index: number, applied: RailwayTraffic) => void,
  options: RailwayWriteOptions = {},
): Promise<WriteRailwayResult> {
  const result: WriteRailwayResult = {
    rows: 0,
    matched: 0,
    updated: false,
    skippedService: 0,
    skippedForeign: 0,
    skippedPriority: 0,
    retracted: 0,
  }
  const retractIds = validateRetract(options.retract)
  const allowedCountryCodes = options.allowedCountryIsos
    ? new Set(options.allowedCountryIsos.map(iso2Code))
    : null

  await withArrowWrite(arrowPath, (table: Table): Table => {
    result.rows = table.numRows
    const geometry = segmentGeometryReader(table)
    const railType = requiredInteger(table, 'rail_type', false, 8)
    const usage = requiredInteger(table, 'usage', false, 8)
    const service = requiredInteger(table, 'service', false, 8)
    const existingSource = requiredInteger(table, 'source_id', false, 16)
    const existingPassenger = optionalInteger(table, 'trains_passenger', true, 32)
    const existingFreight = optionalInteger(table, 'trains_freight', true, 32)
    const existingDivisor = optionalInteger(table, 'parallel_divisor', false, 8)
    const names = table.getChild('name')
    if (!names || !DataType.isUtf8(names.type)) throw new Error("railways Arrow 'name' must be Utf8")
    if (table.numRows === 0) return table

    const originalPassenger = new Int32Array(table.numRows)
    const originalFreight = new Int32Array(table.numRows)
    const originalSource = new Uint16Array(table.numRows)
    const originalDivisor = new Uint8Array(table.numRows)
    for (let index = 0; index < table.numRows; index++) {
      originalPassenger[index] = (existingPassenger?.get(index) as number) ?? 0
      originalFreight[index] = (existingFreight?.get(index) as number) ?? 0
      originalSource[index] = existingSource.get(index) as number
      originalDivisor[index] = (existingDivisor?.get(index) as number) ?? 1
    }
    const passenger = originalPassenger.slice()
    const freight = originalFreight.slice()
    const source = originalSource.slice()
    const divisor = originalDivisor.slice()

    const acceptedCountryCodes = new Map<number, ReadonlySet<number> | null>()
    let countries: ReturnType<typeof bakedRailwayCountryReader> | null =
      allowedCountryCodes ? bakedRailwayCountryReader(table) : null
    const countryCodesFor = (sourceId: number): ReadonlySet<number> | null => {
      if (acceptedCountryCodes.has(sourceId)) return acceptedCountryCodes.get(sourceId)!
      const isos = countryIsosForNationalSource(sourceId)
      const codes = isos === null ? null : new Set(isos.map(iso2Code))
      acceptedCountryCodes.set(sourceId, codes)
      return codes
    }

    for (let index = 0; index < table.numRows; index++) {
      const row: RailwayRow = {
        ...geometry.row(index),
        railType: railType.get(index) as number,
        usage: usage.get(index) as number,
        service: service.get(index) as number,
        name: (names.get(index) as string | null) ?? '',
        existingSourceId: originalSource[index],
        existingPassenger: originalPassenger[index],
        existingFreight: originalFreight[index],
        existingDivisor: originalDivisor[index],
      }

      const inAllowedCountry = allowedCountryCodes === null ||
        allowedCountryCodes.has(countries!.codeAt(index))
      if (inAllowedCountry && retractIds.has(source[index]) &&
          (row.service > 0 || options.retract!.when(row, index))) {
        passenger[index] = 0
        freight[index] = 0
        source[index] = 0
        divisor[index] = 1
        result.retracted++
      }
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
      if (expectedCountries !== null) {
        countries ??= bakedRailwayCountryReader(table)
        if (!expectedCountries.has(countries.codeAt(index))) {
          result.skippedForeign++
          continue
        }
      }
      if (!shouldOverwrite(source[index], candidate.sourceId)) {
        result.skippedPriority++
        continue
      }

      passenger[index] = candidate.passenger
      freight[index] = candidate.freight
      source[index] = candidate.sourceId
      if (candidate.divisor !== undefined) divisor[index] = candidate.divisor
      result.matched++
      onApplied?.(row, index, candidate)
    }

    let changed = false
    let needsDivisor = existingDivisor !== null
    for (let index = 0; index < table.numRows; index++) {
      changed ||= passenger[index] !== originalPassenger[index] ||
        freight[index] !== originalFreight[index] ||
        source[index] !== originalSource[index] ||
        divisor[index] !== originalDivisor[index]
      needsDivisor ||= divisor[index] !== 1
    }
    if (!changed) return table
    result.updated = true

    const replacements = new Map<string, Vector>([
      ['trains_passenger', makeVector(passenger)],
      ['trains_freight', makeVector(freight)],
      ['source_id', makeVector(source)],
      ...(needsDivisor ? [['parallel_divisor', makeVector(divisor)] as const] : []),
    ])
    const columns: Record<string, unknown> = {}
    for (const field of table.schema.fields) {
      columns[field.name] = replacements.get(field.name) ?? table.getChild(field.name)!
      replacements.delete(field.name)
    }
    for (const [name, vector] of replacements) columns[name] = vector
    return makeTable(columns as never) as unknown as Table
  })

  return result
}

/**
 * Replace only `parallel_divisor`; every other column and its provenance rides
 * through unchanged. Policy belongs to the caller: null keeps the stored byte,
 * while zero is the one accepted shorthand for the physical floor of one.
 */
export async function writeRailParallelDivisor(
  arrowPath: string,
  divisorForRow: (index: number) => number | null,
): Promise<WriteRailParallelDivisorResult> {
  const result: WriteRailParallelDivisorResult = { rows: 0, changedRows: 0, updated: false }

  await withArrowWrite(arrowPath, (table: Table): Table => {
    result.rows = table.numRows
    if (table.numRows === 0) return table
    const existingDivisor = optionalInteger(table, 'parallel_divisor', false, 8)
    const divisor = new Uint8Array(table.numRows)
    for (let index = 0; index < table.numRows; index++) {
      const existing = (existingDivisor?.get(index) as number | null) ?? 1
      const candidate = divisorForRow(index)
      if (candidate === null) {
        divisor[index] = existing
        continue
      }
      if (!Number.isInteger(candidate) || candidate < 0 || candidate > 0xff) {
        throw new Error(
          `writeRailParallelDivisor: invalid divisor at row ${index} in ${arrowPath}: ${candidate}`,
        )
      }
      divisor[index] = Math.max(1, candidate)
      if (divisor[index] !== existing) result.changedRows++
    }
    if (result.changedRows === 0) return table
    result.updated = true

    const replacements = new Map<string, Vector>([['parallel_divisor', makeVector(divisor)]])
    const columns: Record<string, unknown> = {}
    for (const field of table.schema.fields) {
      columns[field.name] = replacements.get(field.name) ?? table.getChild(field.name)!
      replacements.delete(field.name)
    }
    for (const [name, vector] of replacements) columns[name] = vector
    return makeTable(columns as never) as unknown as Table
  })

  return result
}
