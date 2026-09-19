/** Enrich z9 New Zealand roads with NZTA and Auckland Transport AADT. */

import { shouldOverwrite } from './lib/provenance.js'
import { runRoadLoaderCli, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { loadNewZealandRoadSources, type NewZealandRoadObservation } from './lib/roads-nz-source.js'
import { buildOneHundredthDegreePointGrid } from './lib/spatial.js'
import { SOURCE_ID_NZ_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { nearestCountWithin200Metres, writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { writeNationalRoadSquares } from './lib/square-pool.js'

const SOURCE_ID = SOURCE_ID_NZ_NATIONAL_ROADS
const NEW_ZEALAND_BBOX = [-47.5, 165, -34, 179] as const
const COVERED_ROAD_CLASSES: ReadonlySet<number> = new Set([0, 1, 2, 3, 4, 10, 11, 12])

export async function enrichNewZealandRoads(
  preparedDirectory: string,
  observations: readonly NewZealandRoadObservation[],
) {
  if (observations.length === 0) throw new Error('New Zealand road sources have no usable measurements')
  const grid = buildOneHundredthDegreePointGrid(observations)
  const match = (row: RoadRow) => nearestCountWithin200Metres(row, grid)
  return writeNationalRoadSquares(preparedDirectory, NEW_ZEALAND_BBOX, 'New Zealand', {}, path =>
    writeRoadAadt(path, row => {
      if (!shouldOverwrite(row.existingSourceId, SOURCE_ID)) return null
      const observation = match(row)
      return observation ? { countBasis: observation.countBasis, observationId: observation.observationId, light: observation.light, medium: observation.medium,
        heavy: observation.heavy, moto: observation.moto, sourceId: SOURCE_ID } : null
    }, undefined, COVERED_ROAD_CLASSES,
    { sourceIds: [SOURCE_ID], when: row =>
      !COVERED_ROAD_CLASSES.has(row.roadClass) || match(row) === null }))
}

export async function runNewZealandRoadEnrichment(options: RoadLoaderArguments) {
  const source = loadNewZealandRoadSources(options)
  return { nztaRows: source.nztaRows, atRows: source.atRows,
    observations: source.observations.length,
    unavailableTrafficSkipped: source.unavailableTrafficSkipped,
    unsupportedClassSkipped: source.unsupportedClassSkipped,
    invalidGeometrySkipped: source.invalidGeometrySkipped,
    ...await enrichNewZealandRoads(options.preparedDirectory, source.observations) }
}

runRoadLoaderCli(import.meta.url, runNewZealandRoadEnrichment)
