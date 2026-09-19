/** Enrich Colombia roads from pinned INVIAS TPDA and Red Vial sources. */

import { COLOMBIA_ROAD_BBOX, COLOMBIA_ROAD_COVERAGE, loadColombiaRoadSource, matchColombiaRoad } from './lib/roads-co-source.js'
import { runRoadLoaderCli, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { SOURCE_ID_CO_ROAD_CLASSIFICATION_FALLBACK, SOURCE_ID_CO_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { writeNationalRoadSquares } from './lib/square-pool.js'

export async function runColombiaRoadEnrichment(options: RoadLoaderArguments) {
  const source = loadColombiaRoadSource(options)
  const tally = { matchedTpda: 0, matchedNetwork: 0 }
  const match = (row: RoadRow) => matchColombiaRoad(row, source)
  const counters = await writeNationalRoadSquares(options.preparedDirectory, COLOMBIA_ROAD_BBOX, 'Colombia', tally, path =>
    writeRoadAadt(path, row => {
      const traffic = match(row)
      return traffic ? { countBasis: traffic.countBasis, observationId: traffic.observationId, light: traffic.light, medium: traffic.medium, heavy: traffic.heavy,
        moto: traffic.moto, sourceId: traffic.kind === 'tpda' ? SOURCE_ID_CO_NATIONAL_ROADS : SOURCE_ID_CO_ROAD_CLASSIFICATION_FALLBACK } : null
    }, row => { if (match(row)?.kind === 'tpda') tally.matchedTpda++; else tally.matchedNetwork++ },
    COLOMBIA_ROAD_COVERAGE, { sourceIds: [SOURCE_ID_CO_NATIONAL_ROADS, SOURCE_ID_CO_ROAD_CLASSIFICATION_FALLBACK],
      when: row => { const traffic = match(row); return traffic === null ||
        (traffic.kind === 'tpda' ? SOURCE_ID_CO_NATIONAL_ROADS : SOURCE_ID_CO_ROAD_CLASSIFICATION_FALLBACK) !== row.existingSourceId } }))
  return { sourceRows: source.sourceRows, sourceLines: source.sourceLines,
    invalidGeometrySkipped: source.invalidGeometrySkipped, unavailableTrafficRows: source.unavailableTrafficRows,
    ...counters, ...tally }
}

runRoadLoaderCli(import.meta.url, runColombiaRoadEnrichment)
