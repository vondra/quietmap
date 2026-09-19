/** Enrich Peru roads from pinned MTC Red Vial classifications and dIMD. */

import { loadPeruRoadSource, matchPeruRoad, PERU_ROAD_BBOX, PERU_ROAD_COVERAGE } from './lib/roads-pe-source.js'
import { runRoadLoaderCli, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { SOURCE_ID_PE_ROAD_CLASSIFICATION_FALLBACK, SOURCE_ID_PE_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { writeNationalRoadSquares } from './lib/square-pool.js'

export async function runPeruRoadEnrichment(options: RoadLoaderArguments) {
  const source = loadPeruRoadSource(options)
  const tally = { matchedImd: 0, matchedNetwork: 0 }
  const match = (row: RoadRow) => matchPeruRoad(row, source)
  const counters = await writeNationalRoadSquares(options.preparedDirectory, PERU_ROAD_BBOX, 'Peru', tally, path =>
    writeRoadAadt(path, row => {
      const traffic = match(row)
      return traffic ? { countBasis: traffic.countBasis, observationId: traffic.observationId, light: traffic.light, medium: traffic.medium, heavy: traffic.heavy,
        moto: traffic.moto, sourceId: traffic.kind === 'imd' ? SOURCE_ID_PE_NATIONAL_ROADS : SOURCE_ID_PE_ROAD_CLASSIFICATION_FALLBACK } : null
    }, row => { if (match(row)?.kind === 'imd') tally.matchedImd++; else tally.matchedNetwork++ },
    PERU_ROAD_COVERAGE, { sourceIds: [SOURCE_ID_PE_NATIONAL_ROADS, SOURCE_ID_PE_ROAD_CLASSIFICATION_FALLBACK], when: row => { const traffic = match(row); return traffic === null ||
        (traffic.kind === 'imd' ? SOURCE_ID_PE_NATIONAL_ROADS : SOURCE_ID_PE_ROAD_CLASSIFICATION_FALLBACK) !== row.existingSourceId } }))
  return { sourceRows: source.sourceRows, sourceLines: source.sourceLines,
    invalidGeometrySkipped: source.invalidGeometrySkipped, observedTrafficLines: source.observedTrafficLines,
    ...counters, ...tally }
}

runRoadLoaderCli(import.meta.url, runPeruRoadEnrichment)
