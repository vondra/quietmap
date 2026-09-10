/** Enrich z9 Irish roads with TII counter class totals. */

import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { listPreparedSquares } from './lib/prepared-grid.js'
import { shouldOverwrite } from './lib/provenance.js'
import { parseRoadLoaderArguments, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { loadIrishTiiSource, normalizeIrishRoadRef, type IrishTiiObservation } from './lib/roads-ie-source.js'
import { haversineM } from './lib/spatial.js'
import { SOURCE_ID_IE_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'

const SOURCE_ID = SOURCE_ID_IE_NATIONAL_ROADS
const IRELAND_BBOX = [51.4, -10.5, 55.4, -5.4] as const
const MAXIMUM_MATCH_DISTANCE_M = 25_000

export function indexIrishTii(
  observations: readonly IrishTiiObservation[],
): ReadonlyMap<string, readonly IrishTiiObservation[]> {
  const byRef = new Map<string, IrishTiiObservation[]>()
  for (const observation of observations) {
    const bucket = byRef.get(observation.ref)
    if (bucket) bucket.push(observation)
    else byRef.set(observation.ref, [observation])
  }
  return byRef
}

export function matchIrishTii(
  row: RoadRow,
  byRef: ReadonlyMap<string, readonly IrishTiiObservation[]>,
): IrishTiiObservation | null {
  let closest: IrishTiiObservation | null = null
  let closestDistance = MAXIMUM_MATCH_DISTANCE_M
  for (const token of (row.ref ?? '').split(/[;,]/)) {
    const ref = normalizeIrishRoadRef(token.trim())
    for (const observation of ref ? byRef.get(ref) ?? [] : []) {
      const distance = haversineM(row.midLat, row.midLon, observation.latitude, observation.longitude)
      if (distance < closestDistance) {
        closest = observation
        closestDistance = distance
      }
    }
  }
  return closest
}

export async function enrichIrishRoads(
  preparedDirectory: string,
  observations: readonly IrishTiiObservation[],
) {
  if (observations.length === 0) throw new Error('TII source has no usable traffic observations')
  const squares = listPreparedSquares(preparedDirectory, IRELAND_BBOX)
  if (squares.length === 0) throw new Error(`no Irish roads.arrow squares found under ${preparedDirectory}`)
  const byRef = indexIrishTii(observations)
  const match = (row: RoadRow) => matchIrishTii(row, byRef)
  const result = { rows: 0, matched: 0, retracted: 0, skippedForeign: 0,
    squares: squares.length, squaresUpdated: 0 }
  for (const square of squares) {
    const write = await writeRoadAadt(resolve(preparedDirectory, square, 'roads.arrow'), row => {
      if (!shouldOverwrite(row.existingSourceId, SOURCE_ID)) return null
      const observation = match(row)
      return observation ? { light: observation.light, medium: observation.medium,
        heavy: observation.heavy, moto: observation.moto, sourceId: SOURCE_ID } : null
    }, undefined, undefined, { sourceIds: [SOURCE_ID], when: row => match(row) === null })
    result.rows += write.rows
    result.matched += write.matched
    result.retracted += write.retracted
    result.skippedForeign += write.skippedForeign
    if (write.updated) result.squaresUpdated++
  }
  return result
}

export async function runIrishRoadEnrichment(options: RoadLoaderArguments) {
  const source = loadIrishTiiSource(options)
  return { siteRows: source.siteRows, countRows: source.countRows,
    usableSites: source.usableSites, countedSites: source.countedSites,
    unsupportedClassRows: source.unsupportedClassRows, observations: source.observations.length,
    ...await enrichIrishRoads(options.preparedDirectory, source.observations) }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  runIrishRoadEnrichment(parseRoadLoaderArguments(process.argv.slice(2), 'enrich-roads-ie.ts'))
    .then(result => console.log(JSON.stringify(result)))
    .catch((error: unknown) => {
      console.error(error instanceof Error ? error.message : error)
      process.exitCode = 1
    })
}
