/** The single atomic writer for road traffic enrichment on z9/z30 Arrow data. */

import { closeSync, fstatSync, openSync, readSync } from 'node:fs'
import { DataType, Table, Utf8, makeData, vectorFromArray } from 'apache-arrow'
import { resolve } from 'node:path'
import { Footer } from 'apache-arrow/ipc/metadata/file'
import { ROAD_COUNT_BASES, type RoadObservation } from './road-observation.js'
import { withArrowWrite } from './provenance.js'
import {
  SOURCES_BY_ID, countryIsosForNationalSource, shouldOverwrite,
} from './sources.js'
import { nearestCompatiblePointWithin200Metres, type RankedPoint } from './spatial.js'
import { bakedRoadCountryReader, iso2Code, type SegmentGeometry } from './prepared-grid.js'
import {
  ColumnBackedRoadRow, numericColumnChunk, roadRowColumns, seedColumn, tableWithRebuiltColumns, type ColumnChunkOfBatch,
} from './roads-arrow-columns.js'

/** A slip road carries the ref of its mainline but not its traffic. A census matched by
 *  ref alone must not stamp it; it keeps the link-class default. */
export const isSlipRoadClass = (roadClass: number): boolean => roadClass >= 10 && roadClass <= 12

export const ROAD_CLASS_RANK_TOLERANCE = 1

export const osmRoadClassRank = (roadClass: number): number =>
  roadClass <= 4 ? roadClass : roadClass === 10 ? 0 : roadClass === 11 ? 1 : roadClass === 12 ? 2 : 6

/** A count is stamped only on the class family it was counted on: a publisher ramp count
 *  only on slip roads, every other count never on them; then the functional ranks must agree. */
export const roadClassTakesCount = (roadClass: number, counted: Pick<RankedPoint, 'rank' | 'isRamp'>): boolean =>
  isSlipRoadClass(roadClass) === (counted.isRamp === true) && (counted.rank === null ||
    Math.abs(osmRoadClassRank(roadClass) - counted.rank) <= ROAD_CLASS_RANK_TOLERANCE)

export const nearestCountWithin200Metres = <T extends RankedPoint>(
  row: RoadRow, grid: ReadonlyMap<string, readonly T[]>,
): T | null => nearestCompatiblePointWithin200Metres(
  row.midLat, row.midLon, grid, point => roadClassTakesCount(row.roadClass, point))

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
  /** Bits light=1, medium=2, heavy=4, moto=8: set where the value is a policy share of a published
   *  total (or an invented zero), clear only where the publisher counted that class. Omitted = 15:
   *  the count basis says whose directions a total covers, never which classes were counted. */
  estimatedClasses?: number
  /** Original dataset of a propagated observation; authored writes use sourceId. */
  observationSourceId?: number
}

/** Both writers offer every field lazily. No AADT matcher reads `oneway` or `countryCode`: a count
 *  stamps either carriageway and the writer itself gates national sources by baked country. */
export interface RoadRow extends SegmentGeometry {
  ref: string | null
  name: string | null
  osmId: number | null
  roadClass: number
  existingSourceId: number
  /** Baked square-country-city code (`iso2Code` numeric form). */
  countryCode?: number
  /** Stored geometry direction: 0 both, 1 forward, 2 reverse. */
  oneway?: number
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

/** One observed period-profile dictionary entry: attribution is carried here,
 * deliberately SEPARATE from the AADT observation columns — a profile never
 * re-stamps counts, `source_id`, basis or estimated bits. */
export interface RoadTimeProfileEntry {
  /** Owning counter station (e.g. BW `svznr`); never an internal path. */
  station: string
  /** Observed window, e.g. `2025-01..2025-12`. */
  window: string
  /** Complete observed station-days behind the shares. */
  days: number
  /** Coverage/flags documentation (accepted quality flags, direction caveats). */
  status: string
  /** Per-class day/evening/night shares of 24 h volume; absent class =
   * unknown. `total` is the unclassified-volume share: an explicitly
   * transferred estimate for every class without its own observation. */
  profile: { light?: [number, number, number]; medium?: [number, number, number];
    heavy?: [number, number, number]; moto?: [number, number, number];
    total?: [number, number, number] }
}

export const ROADS_TIME_PROFILES_METADATA_KEY = 'roads_time_profiles'

/** Schema metadata of an Arrow IPC file from its footer flatbuffer alone —
 * record batches are never read — so a sweep over every square stays cheap
 * (Arrow file layout: footer, then int32 footer length, then the closing
 * `ARROW1` magic). */
export function readArrowFileSchemaMetadata(arrowPath: string): Map<string, string> {
  const file = openSync(arrowPath, 'r')
  try {
    const { size } = fstatSync(file)
    if (size < 20) throw new Error(`truncated Arrow IPC file: ${arrowPath}`)
    const tail = Buffer.alloc(10)
    readSync(file, tail, 0, 10, size - 10)
    if (tail.toString('latin1', 4) !== 'ARROW1') {
      throw new Error(`${arrowPath} is not an Arrow IPC file`)
    }
    const footerLength = tail.readUInt32LE(0)
    if (footerLength <= 0 || footerLength > size - 20) throw new Error(`invalid Arrow footer: ${arrowPath}`)
    const footer = Buffer.alloc(footerLength)
    readSync(file, footer, 0, footerLength, size - 10 - footerLength)
    return Footer.decode(footer).schema.metadata
  } finally {
    closeSync(file)
  }
}

/** The owning profile-source URL of a roads.arrow file, or null without a `roads_time_profiles` dictionary. */
export function readRoadTimeProfilesSource(arrowPath: string): string | null {
  const raw = readArrowFileSchemaMetadata(arrowPath).get(ROADS_TIME_PROFILES_METADATA_KEY)
  return raw ? decodeTimeProfileDictionary(raw).source : null
}

function assertTimeProfileEntry(entry: RoadTimeProfileEntry): void {
  if (!entry.station || !entry.window || !entry.status || !Number.isInteger(entry.days) || entry.days <= 0) {
    throw new Error(`writeRoadTimeProfiles: invalid dictionary entry ${JSON.stringify(entry)}`)
  }
  for (const [group, shares] of Object.entries(entry.profile)) {
    if (shares.length !== 3 || shares.some(v => !Number.isFinite(v) || v < 0) ||
        Math.abs(shares.reduce((a, b) => a + b, 0) - 1) > 1e-9) {
      throw new Error(`writeRoadTimeProfiles: invalid ${group} shares at ${entry.station}`)
    }
  }
  if (Object.keys(entry.profile).length === 0) {
    throw new Error(`writeRoadTimeProfiles: entry ${entry.station} has no observed class`)
  }
}

function decodeTimeProfileDictionary(raw: string | undefined): { source: string; entries: RoadTimeProfileEntry[] } {
  if (!raw) return { source: '', entries: [] }
  const value = JSON.parse(raw) as { source: string; entries: RoadTimeProfileEntry[] }
  if (typeof value.source !== 'string' || !Array.isArray(value.entries)) {
    throw new Error('writeRoadTimeProfiles: corrupt roads_time_profiles metadata')
  }
  for (const entry of value.entries) assertTimeProfileEntry(entry)
  return value
}

/** Stamp sparse observed period profiles WITHOUT touching traffic: adds the
 * `traffic_profile_id` u16 column (0 = none) and the `roads_time_profiles`
 * schema-metadata dictionary restricted to referenced entries. Safe on both
 * raw and finalized (`road_traffic_contract=1`) tables — traffic columns are
 * never rewritten. Idempotent per source: rows this source stamped but no
 * longer matches are retracted to 0; a different source's dictionary is an
 * explicit error, never silently re-attributed. */
export async function writeRoadTimeProfiles(
  arrowPath: string,
  sourceUrl: string,
  entries: readonly RoadTimeProfileEntry[],
  match: (row: RoadRow, index: number) => number,
): Promise<WriteRoadResult> {
  for (const entry of entries) assertTimeProfileEntry(entry)
  let result!: WriteRoadResult
  await withArrowWrite(arrowPath, table => {
    const rows = table.numRows
    const columns = roadRowColumns(table)
    columns.countries = bakedRoadCountryReader(table)
    const existingSourceIds = new Uint16Array(rows), ids = new Uint16Array(rows)
    seedColumn(existingSourceIds, table.getChild('source_id'), () => 0)
    seedColumn(ids, table.getChild('traffic_profile_id'), () => 0)
    const dictionary = decodeTimeProfileDictionary(table.schema.metadata.get(ROADS_TIME_PROFILES_METADATA_KEY))
    if (dictionary.source && dictionary.source !== sourceUrl) {
      throw new Error(`writeRoadTimeProfiles: ${arrowPath} carries '${dictionary.source}' profiles; refusing to restamp as '${sourceUrl}'`)
    }
    // Merge: previously stamped references stay valid; a re-stamp of the same
    // station (same window/days/status/profile) reuses its id instead of duplicating.
    const merged = [...dictionary.entries]
    const storedIds = new Map(merged.map((entry, index) => [JSON.stringify(entry), index + 1]))
    const idOf = new Map<number, number>()
    entries.forEach((entry, index) => {
      const key = JSON.stringify(entry)
      let id = storedIds.get(key)
      if (id === undefined) {
        merged.push(entry)
        id = merged.length
        storedIds.set(key, id)
      }
      idOf.set(index + 1, id)
    })
    if (merged.length > 65_535) {
      throw new Error(`writeRoadTimeProfiles: ${merged.length} entries exceed the u16 id capacity`)
    }
    let matched = 0
    let updated = false
    for (let index = 0; index < rows; index++) {
      const picked = match(new ColumnBackedRoadRow(columns, index, existingSourceIds[index]), index)
      if (!Number.isInteger(picked) || picked < 0 || picked > entries.length) {
        throw new Error(`writeRoadTimeProfiles: match returned ${picked} out of range`)
      }
      if (picked > 0) {
        const id = idOf.get(picked)!
        matched++
        if (ids[index] !== id) { ids[index] = id; updated = true }
      } else if (ids[index] !== 0) {
        // This source owns the whole dictionary (mismatched sources are
        // rejected above): unmatched-but-stamped rows retract to 0 = unknown.
        ids[index] = 0
        updated = true
      }
    }
    result = { rows, matched, updated, skipped: 0, skippedForeign: 0, retracted: 0 }
    if (!updated) return table
    const referenced = new Set([...ids])
    const kept = merged.map((entry, index) => ({ entry, index: index + 1 }))
      .filter(({ index }) => referenced.has(index))
    const remap = new Map(kept.map(({ entry }, position) => [entry, position + 1]))
    for (let index = 0; index < rows; index++) {
      const entry = merged[ids[index] - 1]
      if (ids[index] !== 0) ids[index] = remap.get(entry)!
    }
    const metadata = new Map(table.schema.metadata)
    // `withArrowWrite` re-imposes input metadata (output keys only override),
    // so a full retraction keeps an explicit EMPTY dictionary — "this source
    // stamped here and now covers nothing" — instead of a stale entry list.
    metadata.set(ROADS_TIME_PROFILES_METADATA_KEY, JSON.stringify({
      source: dictionary.source || sourceUrl,
      entries: kept.map(({ entry }) => entry),
    }))
    return tableWithRebuiltColumns(table, metadata, new Map([['traffic_profile_id', numericColumnChunk(ids)]]))
  })
  return result
}

/** Walk prepared squares stamping one source's profile dictionary — the
 * reusable profile-only producer entry shared by every profile enrichment
 * (DE BW counters, US TMAS hourly totals). Applying never touches traffic
 * columns, so it is safe on finalized copies. */
export async function applyRoadTimeProfiles(
  preparedDirectory: string,
  squares: readonly string[],
  sourceUrl: string,
  entries: readonly RoadTimeProfileEntry[],
  match: (row: RoadRow, index: number) => number,
): Promise<{ rows: number; matched: number; squaresUpdated: number }> {
  let rows = 0
  let matched = 0
  let squaresUpdated = 0
  for (const square of squares) {
    const write = await writeRoadTimeProfiles(
      resolve(preparedDirectory, square, 'roads.arrow'), sourceUrl, entries, match,
    )
    rows += write.rows
    matched += write.matched
    if (write.updated) squaresUpdated++
  }
  return { rows, matched, squaresUpdated }
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
  const columns = roadRowColumns(table)
  if (result.rows === 0) return { table, result }

  const existingTaper = table.getChild('speed_taper')
  if (table.schema.metadata.get('road_traffic_contract') === '1') {
    throw new Error(`writeRoadAadt: rebuild raw inputs before enriching finalized traffic in ${arrowPath}`)
  }
  const existingEstimated = table.getChild('traffic_estimated')
  const existingBasis = table.getChild('traffic_count_basis')
  const existingObservation = table.getChild('traffic_observation_id')
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
  const taper = new Uint8Array(result.rows)
  seedColumn(light, table.getChild('aadt_light'), () => 0)
  seedColumn(medium, table.getChild('aadt_medium'), () => 0)
  seedColumn(heavy, table.getChild('aadt_heavy'), () => 0)
  seedColumn(moto, table.getChild('aadt_moto'), () => 0)
  seedColumn(source, table.getChild('source_id'), () => 0)
  seedColumn(basis, existingBasis, () => 0)
  seedColumn(estimated, existingEstimated, () => 15)
  seedColumn(origins, table.getChild('traffic_observation_source'), index => source[index])
  seedColumn(taper, existingTaper, () => 0)
  // Stored observation strings are decoded only where a row is compared or its batch is rewritten.
  const storedObservationIsEmpty = new Uint8Array(result.rows).fill(1)
  let chunkStart = 0
  for (const chunk of existingObservation?.data ?? []) {
    const offsets = chunk.valueOffsets
    for (let index = 0; index < chunk.length; index++) {
      storedObservationIsEmpty[chunkStart + index] = offsets[index + 1] === offsets[index] ? 1 : 0
    }
    chunkStart += chunk.length
  }
  for (let index = 0; index < result.rows; index++) {
    if (basis[index] >= ROAD_COUNT_BASES.length || estimated[index] > 15 ||
        (source[index] !== 0 && basis[index] !== 3 && storedObservationIsEmpty[index])) {
      throw new Error(`writeRoadAadt: missing or invalid source observation at row ${index} in ${arrowPath}`)
    }
  }
  const changedObservations = new Map<number, string>()
  const observationAt = (index: number): string => changedObservations.get(index) ??
    (storedObservationIsEmpty[index] ? '' : existingObservation!.get(index) as string)
  const setObservation = (index: number, value: string): void => {
    if (observationAt(index) !== value) changedObservations.set(index, value)
  }

  let taperChanged = false
  const setTaper = (index: number, value: number): void => {
    if (taper[index] === value) return
    taper[index] = value
    taperChanged = true
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
    const row = new ColumnBackedRoadRow(columns, index, source[index])

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
      setObservation(index, '')
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
    const nextEstimated = candidate.estimatedClasses ?? 15
    const nextTaper = candidate.speedTaper ?? 0
    const valueChanged = light[index] !== candidate.light || medium[index] !== candidate.medium ||
      heavy[index] !== candidate.heavy || moto[index] !== candidate.moto ||
      source[index] !== candidate.sourceId || taper[index] !== nextTaper ||
      basis[index] !== nextBasis || estimated[index] !== nextEstimated || origins[index] !== nextOrigin ||
      observationAt(index) !== candidate.observationId
    light[index] = candidate.light
    medium[index] = candidate.medium
    heavy[index] = candidate.heavy
    moto[index] = candidate.moto
    source[index] = candidate.sourceId
    basis[index] = nextBasis
    estimated[index] = nextEstimated
    origins[index] = nextOrigin
    setObservation(index, candidate.observationId)
    setTaper(index, nextTaper)
    result.matched++
    changed ||= valueChanged
    onApplied?.(row, index, candidate)
  }

  if (!changed) return { table, result }
  result.updated = true
  const metadata = new Map(table.schema.metadata)
  metadata.set('road_traffic_contract', '0')
  const rebuiltColumns = new Map<string, ColumnChunkOfBatch>([
    ['aadt_light', numericColumnChunk(light)], ['aadt_medium', numericColumnChunk(medium)],
    ['aadt_heavy', numericColumnChunk(heavy)], ['aadt_moto', numericColumnChunk(moto)],
    ['source_id', numericColumnChunk(source)], ['traffic_estimated', numericColumnChunk(estimated)],
    ['traffic_observation_source', numericColumnChunk(origins)], ['traffic_count_basis', numericColumnChunk(basis)],
    ['traffic_observation_id', (startRow, endRow, batch) => {
      const stored = batch.getChild('traffic_observation_id')?.data[0]
      let batchChanged = !stored
      for (let index = startRow; index < endRow && !batchChanged; index++) batchChanged = changedObservations.has(index)
      if (!batchChanged) return stored!
      const observations = Array.from({ length: endRow - startRow }, (_, index) => observationAt(startRow + index))
      // Most batches of a first write hold no observation; the builder costs more than the rest of their rebuild.
      return observations.some(Boolean) ? vectorFromArray(observations, new Utf8()).data[0] : makeData({
        type: new Utf8(), length: observations.length, nullCount: 0,
        valueOffsets: new Int32Array(observations.length + 1), data: new Uint8Array(0) })
    }],
  ])
  if (taperChanged) rebuiltColumns.set('speed_taper', numericColumnChunk(taper))
  return { table: tableWithRebuiltColumns(table, metadata, rebuiltColumns), result }
}
