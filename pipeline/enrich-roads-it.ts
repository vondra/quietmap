/** Enrich z9 Italian roads with Anas TGM point measurements. */

import { shouldOverwrite } from './lib/provenance.js'
import { runRoadLoaderCli, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import {
  loadItalianTgmSource, normalizeItalianOsmRef, type ItalianTgmStation,
} from './lib/roads-it-source.js'
import { flatDist } from './lib/spatial.js'
import { SOURCE_ID_IT_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { isSlipRoadClass, writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { writeNationalRoadSquares } from './lib/square-pool.js'

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
  if (isSlipRoadClass(row.roadClass)) return null
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
  const stationsByRef = indexItalianTgm(stations)
  const match = (row: RoadRow): ItalianTgmStation | null => matchItalianTgm(row, stationsByRef)
  return writeNationalRoadSquares(preparedDirectory, ITALY_BBOX, 'Italian', {}, path =>
    writeRoadAadt(
      path,
      row => {
        if (!shouldOverwrite(row.existingSourceId, SOURCE_ID)) return null
        const station = match(row)
        return station ? { countBasis: station.countBasis, observationId: station.observationId, ...splitItalianTgm(station.total, row.roadClass), sourceId: SOURCE_ID } : null
      },
      undefined,
      undefined,
      { sourceIds: [SOURCE_ID], when: row => match(row) === null },
    ))
}

export async function runItalianRoadEnrichment(options: RoadLoaderArguments) {
  const source = loadItalianTgmSource(options)
  return { sourceRows: source.sourceRows, stations: source.stations.length,
    invalidGeometrySkipped: source.invalidGeometrySkipped,
    invalidTrafficSkipped: source.invalidTrafficSkipped,
    invalidMetadataSkipped: source.invalidMetadataSkipped,
    ...await enrichItalianRoads(options.preparedDirectory, source.stations) }
}

runRoadLoaderCli(import.meta.url, runItalianRoadEnrichment)
