/** Enrich z9 Danish roads with Vejdirektoratet Mastra traffic counts. */

import { shouldOverwrite } from './lib/provenance.js'
import { runRoadLoaderCli, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { loadDanishMastraSource, type DanishMastraObservation } from './lib/roads-dk-source.js'
import { buildOneHundredthDegreePointGrid } from './lib/spatial.js'
import { SOURCE_ID_DK_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { nearestCountWithin200Metres, writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { writeNationalRoadSquares } from './lib/square-pool.js'

const SOURCE_ID = SOURCE_ID_DK_NATIONAL_ROADS
const DENMARK_BBOX = [54.5, 8, 57.8, 13] as const
const COVERED_ROAD_CLASSES: ReadonlySet<number> = new Set([0, 1, 2, 3, 4, 10, 11, 12])

export async function enrichDanishRoads(
  preparedDirectory: string,
  observations: readonly DanishMastraObservation[],
) {
  if (observations.length === 0) throw new Error('Mastra source has no usable traffic observations')
  const grid = buildOneHundredthDegreePointGrid(observations)
  const match = (row: RoadRow) => nearestCountWithin200Metres(row, grid)
  return writeNationalRoadSquares(preparedDirectory, DENMARK_BBOX, 'Danish', {}, path =>
    writeRoadAadt(path, row => {
      if (!shouldOverwrite(row.existingSourceId, SOURCE_ID)) return null
      const observation = match(row)
      return observation ? { countBasis: observation.countBasis, observationId: observation.observationId, light: observation.light, medium: observation.medium,
        heavy: observation.heavy, moto: observation.moto, sourceId: SOURCE_ID } : null
    }, undefined, COVERED_ROAD_CLASSES,
    { sourceIds: [SOURCE_ID], when: row =>
      !COVERED_ROAD_CLASSES.has(row.roadClass) || match(row) === null }))
}

export async function runDanishRoadEnrichment(options: RoadLoaderArguments) {
  const source = loadDanishMastraSource(options)
  return { sourceRows: source.sourceRows, admittedRecords: source.admittedRecords,
    observations: source.observations.length, supersededRecords: source.supersededRecords,
    nonMotorSkipped: source.nonMotorSkipped, invalidRowsSkipped: source.invalidRowsSkipped,
    outOfBoundsSkipped: source.outOfBoundsSkipped,
    ...await enrichDanishRoads(options.preparedDirectory, source.observations) }
}

runRoadLoaderCli(import.meta.url, runDanishRoadEnrichment)
