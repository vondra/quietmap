/** Enrich z9 Swiss roads with ASTRA SASVZ 2024 measurements. */

import { roadObservation } from './lib/road-observation.js'
import { shouldOverwrite } from './lib/provenance.js'
import { runRoadLoaderCli, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { loadSwissSasvzSource, type SwissSasvzObservation } from './lib/roads-ch-source.js'
import { buildOneHundredthDegreePointGrid } from './lib/spatial.js'
import { SOURCE_ID_CH_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { nearestCountWithin200Metres, writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { writeNationalRoadSquares } from './lib/square-pool.js'

const SOURCE_ID = SOURCE_ID_CH_NATIONAL_ROADS
const SWITZERLAND_BBOX = [45.7, 5.8, 47.9, 10.6] as const
const COVERED_ROAD_CLASSES: ReadonlySet<number> = new Set([0, 1, 2, 3, 4, 10, 11, 12])

export async function enrichSwissRoads(preparedDirectory: string, observations: readonly SwissSasvzObservation[]) {
  if (observations.length === 0) throw new Error('Swiss SASVZ source has no usable traffic observations')
  const grid = buildOneHundredthDegreePointGrid(observations.map(observation => ({ ...observation, isRamp: false })))
  const match = (row: RoadRow) => nearestCountWithin200Metres(row, grid)
  return writeNationalRoadSquares(preparedDirectory, SWITZERLAND_BBOX, 'Swiss', {}, path =>
    writeRoadAadt(
      path,
      row => {
        if (!shouldOverwrite(row.existingSourceId, SOURCE_ID)) return null
        const observation = match(row)
        return observation
          ? {
              ...roadObservation(`sasvz2024:${observation.station}`, 'both-directions'),
              light: observation.light,
              medium: observation.medium,
              heavy: observation.heavy,
              moto: observation.moto,
              sourceId: SOURCE_ID,
              estimatedClasses: observation.estimatedClasses,
            }
          : null
      },
      undefined,
      COVERED_ROAD_CLASSES,
      {
        sourceIds: [SOURCE_ID],
        when: row => !COVERED_ROAD_CLASSES.has(row.roadClass) || match(row) === null,
      },
    ),
  )
}

export async function runSwissRoadEnrichment(options: RoadLoaderArguments) {
  const source = await loadSwissSasvzSource(options)
  return {
    stationRows: source.stationRows,
    observations: source.observations.length,
    unusableSkipped: source.unusableSkipped,
    unlocatedSkipped: source.unlocatedSkipped,
    unrankedSkipped: source.unrankedSkipped,
    inconsistentClassesSkipped: source.inconsistentClassesSkipped,
    ...(await enrichSwissRoads(options.preparedDirectory, source.observations)),
  }
}

runRoadLoaderCli(import.meta.url, runSwissRoadEnrichment)
