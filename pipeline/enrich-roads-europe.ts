/** Apply preserved European city traffic observations to z9 roads of the published way or within 50 metres of its line. */

import { readFileSync } from 'node:fs'
import { DataType, tableFromIPC } from 'apache-arrow'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'
import { listPreparedSquares, normalizeLongitude, segmentGeometryReader } from './lib/prepared-grid.js'
import { shouldOverwrite } from './lib/sources.js'
import { SOURCE_ID_EU_CITY_TRAFFIC } from './lib/source-ids.generated.js'
import { writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import {
  loadEuropeanCityTraffic, type EuropeanCityTraffic, type EuropeanTrafficRecord,
} from './lib/roads-europe-source.js'
import {
  buildOneHundredthDegreeSegmentGrid, pointGridCandidates, pointSearchReach,
  pointToPolylineDist, pointToSegmentDist, runsAlongSegment, type SegmentCoordinates,
} from './lib/spatial.js'

const MAXIMUM_DISTANCE_METRES = 50

type MatchedRoad = Pick<RoadRow, 'startLat' | 'startLon' | 'endLat' | 'endLon' | 'midLat' | 'midLon' | 'osmId' | 'roadClass'>
interface ObservationSegment extends SegmentCoordinates { record: EuropeanTrafficRecord }
export interface EuropeanTrafficIndex {
  segments: ReadonlyMap<string, readonly ObservationSegment[]>
  byWayId: ReadonlyMap<number, readonly EuropeanTrafficRecord[]>
}

export function indexEuropeanTraffic(records: readonly EuropeanTrafficRecord[]): EuropeanTrafficIndex {
  const segments: ObservationSegment[] = [], byWayId = new Map<number, EuropeanTrafficRecord[]>()
  for (const record of records) {
    const vertices = record.coordinates
    for (let index = Math.min(1, vertices.length - 1); index < vertices.length; index++) {
      const start = vertices[Math.max(0, index - 1)], end = vertices[index]
      segments.push({ record, startLatitude: start[1], startLongitude: start[0], endLatitude: end[1], endLongitude: end[0] })
    }
    if (record.sourceOsmId === null) continue
    const sameWay = byWayId.get(record.sourceOsmId)
    if (sameWay) sameWay.push(record)
    else byWayId.set(record.sourceOsmId, [record])
  }
  return { segments: buildOneHundredthDegreeSegmentGrid(segments), byWayId }
}

/** A point count has no heading, so one current way is chosen for it across every owner. */
const isDirectionalPoint = (record: EuropeanTrafficRecord): boolean =>
  record.countBasis === 'directional' && record.coordinates.length === 1

// A counted street is not its slip road, its service lane or a track beside it.
const NEVER_MATCHED_BY_PROXIMITY: ReadonlySet<number> = new Set([6, 7, 8, 10, 11, 12])

/** Without the publisher's way id a row must be the counted street itself: an eligible class
 *  running along the line in either direction (a cross street within 50 m is not). */
function liesAlongObservation(row: MatchedRoad, segment: ObservationSegment): boolean {
  return !NEVER_MATCHED_BY_PROXIMITY.has(row.roadClass) && runsAlongSegment(row, segment)
}

/** The publisher's own way identity qualifies at any distance; every other row must lie along the line. */
function* qualifyingObservations(row: MatchedRoad, index: EuropeanTrafficIndex) {
  for (const segment of pointGridCandidates(row.midLat, row.midLon, MAXIMUM_DISTANCE_METRES, index.segments)) {
    const sameWay = row.osmId !== null && row.osmId > 0 && segment.record.sourceOsmId === row.osmId
    const distance = pointToSegmentDist(row.midLat, row.midLon,
      segment.startLatitude, segment.startLongitude, segment.endLatitude, segment.endLongitude)
    if (distance <= MAXIMUM_DISTANCE_METRES && (sameWay || liesAlongObservation(row, segment))) {
      yield { record: segment.record, sameWay, distance }
    }
  }
  for (const record of row.osmId === null || row.osmId <= 0 ? [] : index.byWayId.get(row.osmId) ?? []) {
    yield { record, sameWay: true, distance: pointToPolylineDist(row.midLat, row.midLon, record.coordinates) }
  }
}

export function nearestEuropeanTraffic(
  row: MatchedRoad,
  index: EuropeanTrafficIndex,
  directionalPointWays?: ReadonlyMap<string, number>,
): EuropeanTrafficRecord | null {
  let best: { record: EuropeanTrafficRecord; sameWay: boolean; distance: number } | null = null
  for (const candidate of qualifyingObservations(row, index)) {
    if (directionalPointWays && isDirectionalPoint(candidate.record) &&
        directionalPointWays.get(candidate.record.observationId) !== row.osmId) continue
    if (best === null || (candidate.sameWay && !best.sameWay) || (candidate.sameWay === best.sameWay &&
        (candidate.distance < best.distance || (candidate.distance === best.distance &&
          candidate.record.observationId < best.record.observationId)))) best = candidate
  }
  return best?.record ?? null
}

/** Choose the physical way independently of existing traffic priority, so reruns cannot displace a count. */
function assignDirectionalPointObservations(
  paths: readonly string[],
  records: readonly EuropeanTrafficRecord[],
): ReadonlyMap<string, number> {
  const directionalPoints = records.filter(isDirectionalPoint)
  const selected = new Map<string, { osmId: number; sameWay: boolean; distance: number }>()
  if (!directionalPoints.length) return new Map()
  const index = indexEuropeanTraffic(directionalPoints)
  for (const path of paths) {
    const table = tableFromIPC(readFileSync(path)), geometry = segmentGeometryReader(table)
    const ids = table.getChild('osm_id'), sources = table.getChild('source_id'), classes = table.getChild('road_class')
    if (!ids || !DataType.isInt(ids.type) || ids.type.bitWidth !== 64 || !ids.type.isSigned || ids.nullCount ||
        !sources || !DataType.isInt(sources.type) || sources.type.bitWidth !== 16 || sources.type.isSigned || sources.nullCount ||
        !classes || classes.nullCount) {
      throw new Error(`${path}: invalid road identity, source or class column`)
    }
    const rows = table.numRows
    for (let rowIndex = 0; rowIndex < rows; rowIndex++) {
      const osmId = Number(ids.get(rowIndex))
      if (!Number.isSafeInteger(osmId) || osmId <= 0) throw new Error(`${path}: invalid OSM way identity at row ${rowIndex}`)
      for (const { record, sameWay, distance } of qualifyingObservations({ ...geometry.row(rowIndex), osmId, roadClass: Number(classes.get(rowIndex)) }, index)) {
        const previous = selected.get(record.observationId)
        if (!previous || (sameWay && !previous.sameWay) || (sameWay === previous.sameWay &&
            (distance < previous.distance || (distance === previous.distance && osmId < previous.osmId)))) {
          selected.set(record.observationId, { osmId, sameWay, distance })
        }
      }
    }
  }
  return new Map([...selected].map(([id, choice]) => [id, choice.osmId]))
}

export async function enrichEuropeanRoads(preparedDirectory: string, cities: readonly EuropeanCityTraffic[]) {
  const records = cities.flatMap(city => city.records)
  const index = indexEuropeanTraffic(records)
  const squares = new Set<string>()
  for (const city of cities) {
    const [south, west, north, east] = city.records.flatMap(record => record.coordinates).reduce(
      ([south, west, north, east], [longitude, latitude]) => [
        Math.min(south, latitude), Math.min(west, longitude),
        Math.max(north, latitude), Math.max(east, longitude),
      ], [90, 180, -90, -180])
    // Include neighbouring road owners, not just the observations' own cells.
    const [latitudeReach, longitudeReach] = pointSearchReach(
      Math.max(Math.abs(south), Math.abs(north)), MAXIMUM_DISTANCE_METRES)
    const coversAllLongitudes = east - west + 2 * longitudeReach >= 360
    for (const square of listPreparedSquares(preparedDirectory,
      [Math.max(-90, south - latitudeReach),
        coversAllLongitudes ? -180 : normalizeLongitude(west - longitudeReach),
        Math.min(90, north + latitudeReach),
        coversAllLongitudes ? 180 : normalizeLongitude(east + longitudeReach)])) squares.add(square)
  }
  if (!squares.size) throw new Error(`No prepared road squares intersect the European observations under ${preparedDirectory}`)
  const paths = [...squares].sort().map(square => resolve(preparedDirectory, square, 'roads.arrow'))
  const directionalPointWays = assignDirectionalPointObservations(paths, records)
  const result = { rows: 0, matched: 0, squares: squares.size, squaresUpdated: 0 }
  for (const path of paths) {
    const match = (row: RoadRow): EuropeanTrafficRecord | null => {
      if (!shouldOverwrite(row.existingSourceId, SOURCE_ID_EU_CITY_TRAFFIC)) return null
      return nearestEuropeanTraffic(row, index, directionalPointWays)
    }
    const written = await writeRoadAadt(path, match, undefined, undefined,
      { sourceIds: [SOURCE_ID_EU_CITY_TRAFFIC], when: row => match(row) === null })
    result.rows += written.rows
    result.matched += written.matched
    if (written.updated) result.squaresUpdated++
  }
  return result
}

async function main(): Promise<void> {
  const { values } = parseArgs({ options: {
    'prepared-dir': { type: 'string' }, 'enrichment-dir': { type: 'string' },
  } })
  if (!values['prepared-dir'] || !values['enrichment-dir']) {
    throw new Error('usage: enrich-roads-europe.ts --prepared-dir DIR --enrichment-dir EU_CITY_CACHE_DIR')
  }
  const cities = loadEuropeanCityTraffic(resolve(values['enrichment-dir']))
  const sources = cities.map(({ records, ...source }) => ({ ...source, accepted: records.length }))
  console.log(JSON.stringify({ sources }))
  const result = await enrichEuropeanRoads(resolve(values['prepared-dir']), cities)
  console.log(JSON.stringify({ citiesRead: cities.length,
    features: cities.reduce((sum, city) => sum + city.features, 0),
    accepted: cities.reduce((sum, city) => sum + city.records.length, 0),
    rejected: cities.reduce((sum, city) => sum + city.rejected.length, 0), ...result }))
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : error)
    process.exitCode = 1
  })
}
