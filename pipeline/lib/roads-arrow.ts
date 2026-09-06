/** The single atomic writer for road traffic enrichment on z9/z30 Arrow data. */

import { makeTable, makeVector, type Table } from 'apache-arrow'
import { withArrowWrite } from './provenance.js'
import {
  SOURCES_BY_ID, countryIsoForNationalSource, isMeasured, shouldOverwrite,
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

export interface RoadAadt {
  light: number
  medium: number
  heavy: number
  moto: number
  sourceId: number
  /** Derived effective speed; any accepted write clears it unless restated. */
  speedTaper?: number
}

export interface RoadRow extends SegmentGeometry {
  ref: string | null
  name: string | null
  osmId: number | null
  roadClass: number
  existingSourceId: number
}

export interface RoadRetract {
  sourceId: number
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
  if (aadtValues.some(value => !Number.isSafeInteger(value) || value < 0 || value > 2_147_483_647) ||
      !Number.isInteger(match.sourceId) || match.sourceId <= 0) {
    throw new Error(`writeRoadAadt: invalid match at row ${index} in ${path}: ${JSON.stringify(match)}`)
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
  if (table.numRows === 0) return { table, result }

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

  const light = new Int32Array(table.numRows)
  const medium = new Int32Array(table.numRows)
  const heavy = new Int32Array(table.numRows)
  const moto = new Int32Array(table.numRows)
  const source = new Uint16Array(table.numRows)
  for (let index = 0; index < table.numRows; index++) {
    light[index] = (existingLight?.get(index) as number) ?? 0
    medium[index] = (existingMedium?.get(index) as number) ?? 0
    heavy[index] = (existingHeavy?.get(index) as number) ?? 0
    moto[index] = (existingMoto?.get(index) as number) ?? 0
    source[index] = (existingSource?.get(index) as number) ?? 0
  }

  let taper: Uint8Array | null = null
  const taperAt = (index: number) => taper?.[index] ?? ((existingTaper?.get(index) as number) ?? 0)
  const setTaper = (index: number, value: number): void => {
    if (taperAt(index) === value) return
    if (!taper) {
      taper = new Uint8Array(table.numRows)
      for (let i = 0; i < table.numRows; i++) taper[i] = (existingTaper?.get(i) as number) ?? 0
    }
    taper[index] = value
  }

  let countries: ReturnType<typeof bakedRoadCountryReader> | null = null
  const countryCodes = new Map<number, number>()
  const expectedCountryCode = (sourceId: number): number | null => {
    const countryIso = countryIsoForNationalSource(sourceId)
    if (countryIso === null) return null
    countries ??= bakedRoadCountryReader(table)
    let expected = countryCodes.get(sourceId)
    if (expected === undefined) {
      expected = iso2Code(countryIso)
      countryCodes.set(sourceId, expected)
    }
    return expected
  }
  const retractCountryCode = retract ? expectedCountryCode(retract.sourceId) : null
  let changed = false

  for (let index = 0; index < table.numRows; index++) {
    const row: RoadRow = {
      ...geometry.row(index),
      ref: (ref?.get(index) as string | null) ?? null,
      name: (name?.get(index) as string | null) ?? null,
      osmId: osmId ? Number(osmId.get(index)) : null,
      roadClass: (roadClass?.get(index) as number) ?? 5,
      existingSourceId: source[index],
    }

    // Retraction precedes every eligibility gate so stale out-of-scope rows heal.
    const retractsForeignNationalStamp = retractCountryCode !== null &&
      countries!.codeAt(index) !== retractCountryCode
    if (retract && source[index] === retract.sourceId &&
        (retractsForeignNationalStamp || retract.when(row, index))) {
      light[index] = 0
      medium[index] = 0
      heavy[index] = 0
      moto[index] = 0
      source[index] = 0
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

    const expectedCode = expectedCountryCode(candidate.sourceId)
    if (expectedCode !== null) {
      if (countries!.codeAt(index) !== expectedCode) {
        result.skippedForeign++
        continue
      }
    }

    if (candidate.light === 0 && candidate.medium === 0 && candidate.heavy === 0 &&
        candidate.moto === 0 && isMeasured(candidate.sourceId)) {
      throw new Error(`writeRoadAadt: all-zero AADT from measured source at row ${index} in ${arrowPath}: ${JSON.stringify(candidate)}`)
    }
    if (!shouldOverwrite(source[index], candidate.sourceId)) continue

    const nextTaper = candidate.speedTaper ?? 0
    const valueChanged = light[index] !== candidate.light || medium[index] !== candidate.medium ||
      heavy[index] !== candidate.heavy || moto[index] !== candidate.moto ||
      source[index] !== candidate.sourceId || taperAt(index) !== nextTaper
    light[index] = candidate.light
    medium[index] = candidate.medium
    heavy[index] = candidate.heavy
    moto[index] = candidate.moto
    source[index] = candidate.sourceId
    setTaper(index, nextTaper)
    result.matched++
    changed ||= valueChanged
    onApplied?.(row, index, candidate)
  }

  if (!changed) return { table, result }
  result.updated = true
  const rebuilt = new Set(['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto', 'source_id'])
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
  if (taper) columns.speed_taper = makeVector(taper)
  return { table: makeTable(columns as never) as unknown as Table, result }
}
