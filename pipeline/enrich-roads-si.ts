/** Enrich z9 Slovenian roads with DRSI PLDP 2024 measurements. */

import { roadObservation } from './lib/road-observation.js'
import { shouldOverwrite } from './lib/provenance.js'
import { runRoadLoaderCli, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { loadSlovenianPldpSource, type SlovenianPldpObservation } from './lib/roads-si-source.js'
import { buildOneHundredthDegreePointGrid } from './lib/spatial.js'
import { SOURCE_ID_SI_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { nearestCountWithin200Metres, writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { writeNationalRoadSquares } from './lib/square-pool.js'

const SOURCE_ID = SOURCE_ID_SI_NATIONAL_ROADS
const SLOVENIA_BBOX = [45.4, 13.3, 46.9, 16.7] as const
const COVERED_ROAD_CLASSES: ReadonlySet<number> = new Set([0, 1, 2, 3, 4, 10, 11, 12])

export async function enrichSlovenianRoads(
  preparedDirectory: string,
  observations: readonly SlovenianPldpObservation[],
) {
  if (observations.length === 0) throw new Error('Slovenian PLDP source has no usable traffic observations')
  const grid = buildOneHundredthDegreePointGrid(observations.map(observation => ({ ...observation, isRamp: false })))
  const match = (row: RoadRow) => nearestCountWithin200Metres(row, grid)
  return writeNationalRoadSquares(preparedDirectory, SLOVENIA_BBOX, 'Slovenian', {}, path =>
    writeRoadAadt(
      path,
      row => {
        if (!shouldOverwrite(row.existingSourceId, SOURCE_ID)) return null
        const observation = match(row)
        return observation
          ? {
              ...roadObservation(`pldp2024:${observation.site}`, 'both-directions'),
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

export async function runSlovenianRoadEnrichment(options: RoadLoaderArguments) {
  const source = loadSlovenianPldpSource(options)
  return {
    countRows: source.countRows,
    siteRows: source.siteRows,
    observations: source.observations.length,
    estimatedSkipped: source.estimatedSkipped,
    uncategorizedSkipped: source.uncategorizedSkipped,
    unlocatedSkipped: source.unlocatedSkipped,
    zeroTrafficSkipped: source.zeroTrafficSkipped,
    ...(await enrichSlovenianRoads(options.preparedDirectory, source.observations)),
  }
}

runRoadLoaderCli(import.meta.url, runSlovenianRoadEnrichment)
