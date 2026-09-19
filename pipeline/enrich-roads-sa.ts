/** Enrich Saudi roads from pinned MoT counts, Riyadh PMS and national atlas. */

import { loadSaudiRoadSource, matchSaudiRoad, SAUDI_ROAD_BBOX, SAUDI_ROAD_COVERAGE } from './lib/roads-sa-source.js'
import { runRoadLoaderCli, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { SOURCE_ID_SA_ROAD_CLASSIFICATION_FALLBACK, SOURCE_ID_SA_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { writeNationalRoadSquares } from './lib/square-pool.js'

export async function runSaudiRoadEnrichment(options: RoadLoaderArguments) {
  const source = loadSaudiRoadSource(options)
  const tally = { matchedMot: 0, matchedRiyadh: 0, matchedAtlas: 0 }
  const match = (row: RoadRow) => matchSaudiRoad(row, source)
  const counters = await writeNationalRoadSquares(options.preparedDirectory, SAUDI_ROAD_BBOX, 'Saudi', tally, path =>
    writeRoadAadt(path, row => {
      const traffic = match(row)
      return traffic ? { countBasis: traffic.countBasis, observationId: traffic.observationId, light: traffic.light, medium: traffic.medium, heavy: traffic.heavy,
        moto: traffic.moto, sourceId: traffic.kind === 'mot' ? SOURCE_ID_SA_NATIONAL_ROADS : SOURCE_ID_SA_ROAD_CLASSIFICATION_FALLBACK } : null
    }, row => { const kind = match(row)?.kind; if (kind === 'mot') tally.matchedMot++
      else if (kind === 'riyadh') tally.matchedRiyadh++; else tally.matchedAtlas++ },
    SAUDI_ROAD_COVERAGE, { sourceIds: [SOURCE_ID_SA_NATIONAL_ROADS, SOURCE_ID_SA_ROAD_CLASSIFICATION_FALLBACK], when: row => { const traffic = match(row); return traffic === null ||
        (traffic.kind === 'mot' ? SOURCE_ID_SA_NATIONAL_ROADS : SOURCE_ID_SA_ROAD_CLASSIFICATION_FALLBACK) !== row.existingSourceId } }))
  return { sourceRows: source.sourceRows, sourceLines: source.sourceLines,
    stations: source.stations, unavailableTrafficRows: source.unavailableTrafficRows,
    invalidGeometrySkipped: source.invalidGeometrySkipped,
    ...counters, ...tally }
}

runRoadLoaderCli(import.meta.url, runSaudiRoadEnrichment)
