/** The single atomic writer for road traffic enrichment on z9/z30 Arrow data. */

import { DataType, RecordBatch, Schema, Table, Utf8, makeTable, makeVector, vectorFromArray } from 'apache-arrow'
import { ROAD_COUNT_BASES, type RoadObservation } from './road-observation.js'
import { withArrowWrite } from './provenance.js'
import {
  SOURCES_BY_ID, countryIsosForNationalSource, isMeasured, shouldOverwrite,
} from './sources.js'
import {
  bakedRoadCountryReader, iso2Code, segmentGeometryReader, type SegmentGeometry,
} from './prepared-grid.js'

export const ROAD_CLASS_RANK_TOLERANCE = 1

export const osmRoadClassRank = (roadClass: number): number =>
  roadClass <= 4 ? roadClass : roadClass === 10 ? 0 : roadClass === 11 ? 1 : roadClass === 12 ? 2 : 6

/** Validate disjoint class counts against an exact or independently rounded total. */
export function disjointVehicleClassCountsFitPublishedTotal(
  total: number,
  classCounts: readonly number[],
  totalSemantics: 'exact' | 'independently-rounded',
): boolean {
  if (!Number.isSafeInteger(total) || total < 0 || classCounts.length === 0 ||
      classCounts.some(value => !Number.isSafeInteger(value) || value < 0)) return false
  // n rounded classes and one separately rounded total can differ by <(n+1)/2,
  // hence an integer excess of at most floor(n/2); exact-derived splits get none.
  const maximumExcess = totalSemantics === 'independently-rounded' ? Math.floor(classCounts.length / 2) : 0
  return classCounts.reduce((sum, value) => sum + value, 0) <= total + maximumExcess
}

export interface RoadAadt extends RoadObservation {
  light: number
  medium: number
  heavy: number
  moto: number
  sourceId: number
  /** Derived effective speed; any accepted write clears it unless restated. */
  speedTaper?: number
  /** Bits light=1, medium=2, heavy=4, moto=8: estimated class counts. */
  estimatedClasses?: number
  /** Original dataset of a propagated observation; authored writes use sourceId. */
  observationSourceId?: number
}

export interface RoadRow extends SegmentGeometry {
  ref: string | null
  name: string | null
  osmId: number | null
  roadClass: number
  existingSourceId: number
}

export interface RoadRetract {
  sourceIds: readonly number[]
  when: (row: RoadRow, index: number) => boolean
}

export interface WriteRoadResult {
  rows: number
  matched: number
  updated: boolean
  skipped: number
  skippedForeign: number
  retracted: number
}

function assertMatch(match: RoadAadt, index: number, path: string): void {
  const aadtValues = [match.light, match.medium, match.heavy, match.moto]
  if (aadtValues.some(value => !Number.isFinite(value) || value < 0 ||
      (match.countBasis !== 'allocated' && !Number.isSafeInteger(value))) ||
      !Number.isInteger(match.sourceId) || match.sourceId <= 0) {
    throw new Error(`writeRoadAadt: invalid match at row ${index} in ${path}: ${JSON.stringify(match)}`)
  }
  if (!ROAD_COUNT_BASES.includes(match.countBasis) || typeof match.observationId !== 'string' ||
      (match.countBasis !== 'allocated' && !match.observationId)) {
    throw new Error(`writeRoadAadt: invalid observation at row ${index} in ${path}`)
  }
  if (match.estimatedClasses !== undefined && (!Number.isInteger(match.estimatedClasses) ||
      match.estimatedClasses < 0 || match.estimatedClasses > 15)) {
    throw new Error(`writeRoadAadt: invalid class status at row ${index} in ${path}`)
  }
  if (match.speedTaper !== undefined &&
      (!Number.isInteger(match.speedTaper) || match.speedTaper < 1 || match.speedTaper > 254)) {
    throw new Error(`writeRoadAadt: invalid speedTaper at row ${index} in ${path}: ${JSON.stringify(match)}`)
  }
  if (SOURCES_BY_ID.get(match.sourceId)?.layer !== 'roads') {
    throw new Error(`writeRoadAadt: sourceId ${match.sourceId} is not a registered roads source (row ${index} in ${path})`)
  }
}

/**
 * Seed stored values, offer each covered row to `match`, apply provenance and
 * baked-country gates, then replace only the five traffic fields. `withArrowWrite`
 * preserves metadata and record-batch boundaries and leaves exact no-ops untouched.
 */
export async function writeRoadAadt(
  arrowPath: string,
  match: (row: RoadRow, index: number) => RoadAadt | null,
  onApplied?: (row: RoadRow, index: number, applied: RoadAadt) => void,
  coverage?: ReadonlySet<number>,
  retract?: RoadRetract,
): Promise<WriteRoadResult> {
  let result!: WriteRoadResult
  await withArrowWrite(arrowPath, table => {
    const applied = applyRoadAadt(table, arrowPath, match, onApplied, coverage, retract)
    result = applied.result
    return applied.table
  })
  return result
}

/** Apply the same traffic/priority/taper rules to a caller's already locked table. */
export function applyRoadAadt(
  table: Table,
  arrowPath: string,
  match: (row: RoadRow, index: number) => RoadAadt | null,
  onApplied?: (row: RoadRow, index: number, applied: RoadAadt) => void,
  coverage?: ReadonlySet<number>,
  retract?: RoadRetract,
): { table: Table; result: WriteRoadResult } {
  const result: WriteRoadResult = {
    rows: 0, matched: 0, updated: false, skipped: 0, skippedForeign: 0, retracted: 0,
  }
  result.rows = table.numRows
  const geometry = segmentGeometryReader(table)
  if (result.rows === 0) return { table, result }

  const ref = table.getChild('ref')
  const name = table.getChild('name')
  const osmId = table.getChild('osm_id')
  const roadClass = table.getChild('road_class')
  const existingLight = table.getChild('aadt_light')
  const existingMedium = table.getChild('aadt_medium')
  const existingHeavy = table.getChild('aadt_heavy')
  const existingMoto = table.getChild('aadt_moto')
  const existingSource = table.getChild('source_id')
  const existingTaper = table.getChild('speed_taper')
  if (table.schema.metadata.get('road_traffic_contract') === '1') {
    throw new Error(`writeRoadAadt: rebuild raw inputs before enriching finalized traffic in ${arrowPath}`)
  }
  const existingEstimated = table.getChild('traffic_estimated')
  const existingBasis = table.getChild('traffic_count_basis')
  const existingObservation = table.getChild('traffic_observation_id')
  const existingOrigin = table.getChild('traffic_observation_source')
  if (existingEstimated) {
    if (!existingEstimated || !DataType.isInt(existingEstimated.type) || existingEstimated.type.bitWidth !== 8 ||
        existingEstimated.type.isSigned || existingEstimated.nullCount) {
      throw new Error(`writeRoadAadt: invalid class status column in ${arrowPath}`)
    }
  }
  if (existingBasis || existingObservation) {
    if (!existingBasis || !DataType.isInt(existingBasis.type) || existingBasis.type.bitWidth !== 8 ||
        existingBasis.type.isSigned || existingBasis.nullCount || !existingObservation ||
        !DataType.isUtf8(existingObservation.type) || existingObservation.nullCount) {
      throw new Error(`writeRoadAadt: invalid observation columns in ${arrowPath}`)
    }
  }

  const light = new Float64Array(result.rows)
  const medium = new Float64Array(result.rows)
  const heavy = new Float64Array(result.rows)
  const moto = new Float64Array(result.rows)
  const source = new Uint16Array(result.rows)
  const basis = new Uint8Array(result.rows)
  const estimated = new Uint8Array(result.rows)
  const origins = new Uint16Array(result.rows)
  const observations = new Array<string>(result.rows)
  for (let index = 0; index < result.rows; index++) {
    light[index] = (existingLight?.get(index) as number) ?? 0
    medium[index] = (existingMedium?.get(index) as number) ?? 0
    heavy[index] = (existingHeavy?.get(index) as number) ?? 0
    moto[index] = (existingMoto?.get(index) as number) ?? 0
    source[index] = (existingSource?.get(index) as number) ?? 0
    basis[index] = (existingBasis?.get(index) as number) ?? 0
    observations[index] = (existingObservation?.get(index) as string) ?? ''
    estimated[index] = (existingEstimated?.get(index) as number) ?? 15
    origins[index] = (existingOrigin?.get(index) as number) ?? source[index]
    if (basis[index] >= ROAD_COUNT_BASES.length || estimated[index] > 15 ||
        (source[index] !== 0 && basis[index] !== 3 && !observations[index])) {
      throw new Error(`writeRoadAadt: missing or invalid source observation at row ${index} in ${arrowPath}`)
    }
  }

  let taper: Uint8Array | null = null
  const taperAt = (index: number) => taper?.[index] ?? ((existingTaper?.get(index) as number) ?? 0)
  const setTaper = (index: number, value: number): void => {
    if (taperAt(index) === value) return
    if (!taper) {
      taper = new Uint8Array(result.rows)
      for (let i = 0; i < result.rows; i++) taper[i] = (existingTaper?.get(i) as number) ?? 0
    }
    taper[index] = value
  }

  let countries: ReturnType<typeof bakedRoadCountryReader> | null = null
  const countryCodes = new Map<number, ReadonlySet<number>>()
  const expectedCountryCodes = (sourceId: number): ReadonlySet<number> | null => {
    const countryIsos = countryIsosForNationalSource(sourceId)
    if (countryIsos === null) return null
    countries ??= bakedRoadCountryReader(table)
    let expected = countryCodes.get(sourceId)
    if (expected === undefined) {
      expected = new Set(countryIsos.map(iso2Code))
      countryCodes.set(sourceId, expected)
    }
    return expected
  }
  const retractCountries = new Map(retract?.sourceIds.map(id => [id, expectedCountryCodes(id)]))
  let changed = false

  for (let index = 0; index < result.rows; index++) {
    const row: RoadRow = {
      ...geometry.row(index),
      ref: (ref?.get(index) as string | null) ?? null,
      name: (name?.get(index) as string | null) ?? null,
      osmId: osmId ? Number(osmId.get(index)) : null,
      roadClass: (roadClass?.get(index) as number) ?? 5,
      existingSourceId: source[index],
    }

    // Retraction precedes every eligibility gate so stale out-of-scope rows heal.
    const owned = retractCountries.has(source[index])
    const retractCountryCodes = retractCountries.get(source[index]) ?? null
    const retractsForeignNationalStamp = retractCountryCodes !== null &&
      !retractCountryCodes.has(countries!.codeAt(index))
    if (retract && owned &&
        (retractsForeignNationalStamp || retract.when(row, index))) {
      light[index] = 0
      medium[index] = 0
      heavy[index] = 0
      moto[index] = 0
      source[index] = 0
      basis[index] = 0
      observations[index] = ''
      estimated[index] = 15
      origins[index] = 0
      row.existingSourceId = 0
      setTaper(index, 0)
      result.retracted++
      changed = true
    }

    if (coverage && !coverage.has(row.roadClass)) {
      result.skipped++
      continue
    }
    const candidate = match(row, index)
    if (!candidate) continue
    assertMatch(candidate, index, arrowPath)

    const expectedCodes = expectedCountryCodes(candidate.sourceId)
    if (expectedCodes !== null) {
      if (!expectedCodes.has(countries!.codeAt(index))) {
        result.skippedForeign++
        continue
      }
    }

    if (!shouldOverwrite(source[index], candidate.sourceId)) continue

    const nextOrigin = candidate.observationSourceId ?? candidate.sourceId
    if (!Number.isInteger(nextOrigin) || SOURCES_BY_ID.get(nextOrigin)?.layer !== 'roads') {
      throw new Error(`writeRoadAadt: invalid observation source ${nextOrigin}`)
    }
    const nextBasis = ROAD_COUNT_BASES.indexOf(candidate.countBasis)
    const nextEstimated = candidate.estimatedClasses ??
      (isMeasured(candidate.sourceId) && candidate.countBasis !== 'unknown' ? 0 : 15)
    const nextTaper = candidate.speedTaper ?? 0
    const valueChanged = light[index] !== candidate.light || medium[index] !== candidate.medium ||
      heavy[index] !== candidate.heavy || moto[index] !== candidate.moto ||
      source[index] !== candidate.sourceId || taperAt(index) !== nextTaper ||
      basis[index] !== nextBasis || estimated[index] !== nextEstimated || origins[index] !== nextOrigin ||
      observations[index] !== candidate.observationId
    light[index] = candidate.light
    medium[index] = candidate.medium
    heavy[index] = candidate.heavy
    moto[index] = candidate.moto
    source[index] = candidate.sourceId
    basis[index] = nextBasis
    estimated[index] = nextEstimated
    origins[index] = nextOrigin
    observations[index] = candidate.observationId
    setTaper(index, nextTaper)
    result.matched++
    changed ||= valueChanged
    onApplied?.(row, index, candidate)
  }

  if (!changed) return { table, result }
  result.updated = true
  const rebuilt = new Set(['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto', 'source_id',
    'traffic_count_basis', 'traffic_observation_id', 'traffic_observation_source', 'traffic_estimated'])
  if (taper) rebuilt.add('speed_taper')
  const columns: Record<string, unknown> = {}
  for (const field of table.schema.fields) {
    if (!rebuilt.has(field.name)) columns[field.name] = table.getChild(field.name)!
  }
  columns.aadt_light = makeVector(light)
  columns.aadt_medium = makeVector(medium)
  columns.aadt_heavy = makeVector(heavy)
  columns.aadt_moto = makeVector(moto)
  columns.source_id = makeVector(source)
  columns.traffic_estimated = makeVector(estimated)
  columns.traffic_observation_source = makeVector(origins)
  columns.traffic_count_basis = makeVector(basis)
  columns.traffic_observation_id = vectorFromArray(observations, new Utf8())
  if (taper) columns.speed_taper = makeVector(taper)
  const rebuiltTable = makeTable(columns as never) as unknown as Table
  const metadata = new Map(table.schema.metadata)
  metadata.set('road_traffic_contract', '0')
  const schema = new Schema(rebuiltTable.schema.fields, metadata)
  return { table: new Table(schema, rebuiltTable.batches.map(batch => new RecordBatch(schema, batch.data))), result }
}
