/** Enrich z9 Irish roads with TII counter class totals. */

import { roadObservation } from './lib/road-observation.js'
import { shouldOverwrite } from './lib/provenance.js'
import { runRoadLoaderCli, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { loadIrishTiiSource, normalizeIrishRoadRef, type IrishTiiObservation } from './lib/roads-ie-source.js'
import { haversineM } from './lib/spatial.js'
import { SOURCE_ID_IE_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { isSlipRoadClass, writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { writeNationalRoadSquares } from './lib/square-pool.js'

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
  if (isSlipRoadClass(row.roadClass)) return null
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
  const byRef = indexIrishTii(observations)
  const match = (row: RoadRow) => matchIrishTii(row, byRef)
  return writeNationalRoadSquares(preparedDirectory, IRELAND_BBOX, 'Irish', {}, path =>
    writeRoadAadt(path, row => {
      if (!shouldOverwrite(row.existingSourceId, SOURCE_ID)) return null
      const observation = match(row)
      return observation ? { ...roadObservation(observation.cosit, 'both-directions'), light: observation.light, medium: observation.medium,
        heavy: observation.heavy, moto: observation.moto, sourceId: SOURCE_ID,
        estimatedClasses: 0 } : null // TII publishes every class bin
    }, undefined, undefined, { sourceIds: [SOURCE_ID], when: row => match(row) === null }))
}

export async function runIrishRoadEnrichment(options: RoadLoaderArguments) {
  const source = loadIrishTiiSource(options)
  return { siteRows: source.siteRows, countRows: source.countRows,
    usableSites: source.usableSites, countedSites: source.countedSites,
    unsupportedClassRows: source.unsupportedClassRows, observations: source.observations.length,
    ...await enrichIrishRoads(options.preparedDirectory, source.observations) }
}

runRoadLoaderCli(import.meta.url, runIrishRoadEnrichment)
