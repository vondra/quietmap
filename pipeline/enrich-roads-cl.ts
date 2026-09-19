/** Enrich Chile roads from pinned TMDA observations and Red Vial classifications. */

import { CHILE_ROAD_BBOX, CHILE_ROAD_COVERAGE, loadChileRoadSource, matchChileRoad } from './lib/roads-cl-source.js'
import { runRoadLoaderCli, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { SOURCE_ID_CL_ROAD_CLASSIFICATION_FALLBACK, SOURCE_ID_CL_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { writeNationalRoadSquares } from './lib/square-pool.js'

export async function runChileRoadEnrichment(options: RoadLoaderArguments) {
  const source = loadChileRoadSource(options)
  const tally = { matchedTmda: 0, matchedNetwork: 0 }
  const match = (row: RoadRow) => matchChileRoad(row, source)
  const counters = await writeNationalRoadSquares(options.preparedDirectory, CHILE_ROAD_BBOX, 'Chile', tally, path =>
    writeRoadAadt(path, row => {
      const traffic = match(row)
      return traffic ? { countBasis: traffic.countBasis, observationId: traffic.observationId, light: traffic.light, medium: traffic.medium, heavy: traffic.heavy,
        moto: traffic.moto, sourceId: traffic.kind === 'tmda' ? SOURCE_ID_CL_NATIONAL_ROADS : SOURCE_ID_CL_ROAD_CLASSIFICATION_FALLBACK } : null
    }, row => { if (match(row)?.kind === 'tmda') tally.matchedTmda++; else tally.matchedNetwork++ },
    CHILE_ROAD_COVERAGE, { sourceIds: [SOURCE_ID_CL_NATIONAL_ROADS, SOURCE_ID_CL_ROAD_CLASSIFICATION_FALLBACK],
      when: row => { const traffic = match(row); return traffic === null ||
        (traffic.kind === 'tmda' ? SOURCE_ID_CL_NATIONAL_ROADS : SOURCE_ID_CL_ROAD_CLASSIFICATION_FALLBACK) !== row.existingSourceId } }))
  return { sourceRows: source.sourceRows, sourceLines: source.sourceLines,
    tmdaPoints: source.tmdaPoints, invalidGeometrySkipped: source.invalidGeometrySkipped,
    unavailableTrafficSkipped: source.unavailableTrafficSkipped,
    ...counters, ...tally }
}
runRoadLoaderCli(import.meta.url, runChileRoadEnrichment)
