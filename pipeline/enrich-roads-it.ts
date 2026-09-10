/** Enrich z9 Italian roads with Anas TGM point measurements. */

import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { listPreparedSquares } from './lib/prepared-grid.js'
import { shouldOverwrite } from './lib/provenance.js'
import { parseRoadLoaderArguments, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import {
  loadItalianTgmSource, normalizeItalianOsmRef, type ItalianTgmStation,
} from './lib/roads-it-source.js'
import { flatDist } from './lib/spatial.js'
import { SOURCE_ID_IT_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'

const SOURCE_ID = SOURCE_ID_IT_NATIONAL_ROADS
const ITALY_BBOX = [35.5, 6.6, 47.1, 18.6] as const
const MAXIMUM_MATCH_DISTANCE_M = 30_000

export function indexItalianTgm(
  stations: readonly ItalianTgmStation[],
): ReadonlyMap<string, readonly ItalianTgmStation[]> {
  const byRef = new Map<string, ItalianTgmStation[]>()
  for (const station of stations) {
    const bucket = byRef.get(station.ref)
    if (bucket) bucket.push(station)
    else byRef.set(station.ref, [station])
  }
  return byRef
}

export function matchItalianTgm(
  row: RoadRow,
  stationsByRef: ReadonlyMap<string, readonly ItalianTgmStation[]>,
): ItalianTgmStation | null {
  const ref = normalizeItalianOsmRef(row.ref ?? '')
  const candidates = ref ? stationsByRef.get(ref) : undefined
  if (!candidates) return null
  let closest: ItalianTgmStation | null = null
  let closestDistance = MAXIMUM_MATCH_DISTANCE_M
  for (const station of candidates) {
    const distance = flatDist(row.midLat, row.midLon, station.latitude, station.longitude)
    if (distance < closestDistance) {
      closest = station
      closestDistance = distance
    }
  }
  return closest
}

/** Preserve the published total while applying the source's dev1 class heuristic. */
export function splitItalianTgm(total: number, roadClass: number) {
  const heavyShare = roadClass <= 1 ? 0.18 : roadClass === 2 ? 0.12 : roadClass === 3 ? 0.08 : 0.05
  const moto = Math.round(total * 0.04)
  const heavyTotal = Math.min(total - moto, Math.round(total * heavyShare))
  const medium = Math.round(heavyTotal * 0.20)
  const heavy = heavyTotal - medium
  return { light: total - moto - heavyTotal, medium, heavy, moto }
}

export async function enrichItalianRoads(
  preparedDirectory: string,
  stations: readonly ItalianTgmStation[],
) {
  if (stations.length === 0) throw new Error('Italian TGM source has no usable measurements')
  const squares = listPreparedSquares(preparedDirectory, ITALY_BBOX)
  if (squares.length === 0) throw new Error(`no Italian roads.arrow squares found under ${preparedDirectory}`)
  const stationsByRef = indexItalianTgm(stations)
  const match = (row: RoadRow): ItalianTgmStation | null => matchItalianTgm(row, stationsByRef)
  const result = { rows: 0, matched: 0, retracted: 0, skippedForeign: 0,
    squares: squares.length, squaresUpdated: 0 }
  for (const square of squares) {
    const write = await writeRoadAadt(
      resolve(preparedDirectory, square, 'roads.arrow'),
      row => {
        if (!shouldOverwrite(row.existingSourceId, SOURCE_ID)) return null
        const station = match(row)
        return station ? { ...splitItalianTgm(station.total, row.roadClass), sourceId: SOURCE_ID } : null
      },
      undefined,
      undefined,
      { sourceIds: [SOURCE_ID], when: row => match(row) === null },
    )
    result.rows += write.rows
    result.matched += write.matched
    result.retracted += write.retracted
    result.skippedForeign += write.skippedForeign
    if (write.updated) result.squaresUpdated++
  }
  return result
}

export async function runItalianRoadEnrichment(options: RoadLoaderArguments) {
  const source = loadItalianTgmSource(options)
  return { sourceRows: source.sourceRows, stations: source.stations.length,
    invalidGeometrySkipped: source.invalidGeometrySkipped,
    invalidTrafficSkipped: source.invalidTrafficSkipped,
    invalidMetadataSkipped: source.invalidMetadataSkipped,
    ...await enrichItalianRoads(options.preparedDirectory, source.stations) }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  runItalianRoadEnrichment(parseRoadLoaderArguments(process.argv.slice(2), 'enrich-roads-it.ts'))
    .then(result => console.log(JSON.stringify(result)))
    .catch((error: unknown) => {
      console.error(error instanceof Error ? error.message : error)
      process.exitCode = 1
    })
}
