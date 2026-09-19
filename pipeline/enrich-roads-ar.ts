/** Enrich Argentina roads from pinned DNV classifications and observed TMDA. */

import { ARGENTINA_ROAD_BBOX, ARGENTINA_ROAD_COVERAGE, loadArgentinaRoadSource,
  matchArgentinaRoad } from './lib/roads-ar-source.js'
import { runRoadLoaderCli, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { SOURCE_ID_AR_ROAD_CLASSIFICATION_FALLBACK, SOURCE_ID_AR_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { writeNationalRoadSquares } from './lib/square-pool.js'

export async function runArgentinaRoadEnrichment(options: RoadLoaderArguments) {
  const source = loadArgentinaRoadSource(options)
  const tally = { matchedTmda: 0, matchedDnv: 0 }
  const match = (row: RoadRow) => matchArgentinaRoad(row, source)
  const counters = await writeNationalRoadSquares(options.preparedDirectory, ARGENTINA_ROAD_BBOX, 'Argentina', tally, path =>
    writeRoadAadt(path, row => {
      const traffic = match(row)
      return traffic ? { countBasis: traffic.countBasis, observationId: traffic.observationId, light: traffic.light, medium: traffic.medium, heavy: traffic.heavy,
        moto: traffic.moto, sourceId: traffic.kind === 'tmda' ? SOURCE_ID_AR_NATIONAL_ROADS : SOURCE_ID_AR_ROAD_CLASSIFICATION_FALLBACK } : null
    }, row => { if (match(row)?.kind === 'tmda') tally.matchedTmda++; else tally.matchedDnv++ },
    ARGENTINA_ROAD_COVERAGE, { sourceIds: [SOURCE_ID_AR_NATIONAL_ROADS, SOURCE_ID_AR_ROAD_CLASSIFICATION_FALLBACK],
      when: row => { const traffic = match(row); return traffic === null ||
        (traffic.kind === 'tmda' ? SOURCE_ID_AR_NATIONAL_ROADS : SOURCE_ID_AR_ROAD_CLASSIFICATION_FALLBACK) !== row.existingSourceId } }))
  return { sourceRows: source.sourceRows, sourceLines: source.sourceLines,
    invalidGeometrySkipped: source.invalidGeometrySkipped, unavailableTrafficRows: source.unavailableTrafficRows,
    ...counters, ...tally }
}

runRoadLoaderCli(import.meta.url, runArgentinaRoadEnrichment)
