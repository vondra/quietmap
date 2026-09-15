/** Apply preserved European city traffic observations to z9 roads within 50 metres. */

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
import { buildOneHundredthDegreePointGrid, flatDist, pointGridCandidates, pointSearchReach } from './lib/spatial.js'

const MAXIMUM_DISTANCE_METRES = 50

export function nearestEuropeanTraffic(
  latitude: number,
  longitude: number,
  grid: ReadonlyMap<string, readonly EuropeanTrafficRecord[]>,
  osmId?: number | null,
  directionalWays?: ReadonlyMap<string, number>,
): EuropeanTrafficRecord | null {
  let closest: EuropeanTrafficRecord | null = null
  let closestDistance = MAXIMUM_DISTANCE_METRES
  for (const record of pointGridCandidates(latitude, longitude, MAXIMUM_DISTANCE_METRES, grid)) {
    if (directionalWays && record.countBasis === 'directional' &&
        directionalWays.get(record.observationId) !== osmId) continue
    const distance = flatDist(latitude, longitude, record.latitude, record.longitude)
    if (distance <= MAXIMUM_DISTANCE_METRES && (closest === null || distance < closestDistance ||
        (distance === closestDistance && record.observationId < closest.observationId))) {
      closest = record
      closestDistance = distance
    }
  }
  return closest
}

/** Choose across every owner before writes; source IDs refer to ways, not clipped rows. */
function assignDirectionalObservations(
  paths: readonly string[],
  records: readonly EuropeanTrafficRecord[],
): ReadonlyMap<string, number> {
  const directional = records.filter(record => record.countBasis === 'directional')
  const selected = new Map<string, { osmId: number; distance: number }>()
  if (!directional.length) return new Map()
  const grid = buildOneHundredthDegreePointGrid(directional)
  for (const path of paths) {
    const table = tableFromIPC(readFileSync(path)), geometry = segmentGeometryReader(table)
    const ids = table.getChild('osm_id'), sources = table.getChild('source_id')
    if (!ids || !DataType.isInt(ids.type) || ids.type.bitWidth !== 64 || !ids.type.isSigned || ids.nullCount ||
        !sources || !DataType.isInt(sources.type) || sources.type.bitWidth !== 16 || sources.type.isSigned || sources.nullCount) {
      throw new Error(`${path}: invalid road identity or source column`)
    }
    const rows = table.numRows
    for (let index = 0; index < rows; index++) {
      if (!shouldOverwrite(Number(sources.get(index)), SOURCE_ID_EU_CITY_TRAFFIC)) continue
      const osmId = Number(ids.get(index))
      if (!Number.isSafeInteger(osmId) || osmId <= 0) throw new Error(`${path}: invalid OSM way identity at row ${index}`)
      const row = geometry.row(index)
      for (const record of pointGridCandidates(row.midLat, row.midLon, MAXIMUM_DISTANCE_METRES, grid)) {
        const distance = flatDist(row.midLat, row.midLon, record.latitude, record.longitude)
        if (distance > MAXIMUM_DISTANCE_METRES) continue
        const previous = selected.get(record.observationId)
        const preferred = osmId === record.sourceOsmId, previousPreferred = previous?.osmId === record.sourceOsmId
        if (!previous || (preferred && !previousPreferred) || (preferred === previousPreferred &&
            (distance < previous.distance || (distance === previous.distance && osmId < previous.osmId)))) {
          selected.set(record.observationId, { osmId, distance })
        }
      }
    }
  }
  return new Map([...selected].map(([id, choice]) => [id, choice.osmId]))
}

export async function enrichEuropeanRoads(preparedDirectory: string, cities: readonly EuropeanCityTraffic[]) {
  const records = cities.flatMap(city => city.records)
  const grid = buildOneHundredthDegreePointGrid(records)
  const squares = new Set<string>()
  for (const city of cities) {
    const [south, west, north, east] = city.records.reduce(
      ([south, west, north, east], record) => [
        Math.min(south, record.latitude), Math.min(west, record.longitude),
        Math.max(north, record.latitude), Math.max(east, record.longitude),
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
  const directionalWays = assignDirectionalObservations(paths, records)
  const result = { rows: 0, matched: 0, squares: squares.size, squaresUpdated: 0 }
  for (const path of paths) {
    const match = (row: RoadRow): EuropeanTrafficRecord | null => {
      if (!shouldOverwrite(row.existingSourceId, SOURCE_ID_EU_CITY_TRAFFIC)) return null
      return nearestEuropeanTraffic(row.midLat, row.midLon, grid, row.osmId, directionalWays)
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
