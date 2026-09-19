/** Enrich Indonesia roads from pinned Bina Marga LHRT and classified networks. */

import { runRoadLoaderCli, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { INDONESIA_ROAD_BBOX, INDONESIA_ROAD_COVERAGE, loadIndonesiaRoadSource,
  matchIndonesiaRoad } from './lib/roads-id-source.js'
import { SOURCE_ID_ID_ROAD_CLASSIFICATION_FALLBACK, SOURCE_ID_ID_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { writeNationalRoadSquares } from './lib/square-pool.js'

export async function runIndonesiaRoadEnrichment(options: RoadLoaderArguments) {
  const source = loadIndonesiaRoadSource(options)
  const tally = { matchedLhrt: 0, matchedToll: 0, matchedRegional: 0, matchedNational: 0 }
  const match = (row: RoadRow) => matchIndonesiaRoad(row, source)
  const counters = await writeNationalRoadSquares(options.preparedDirectory, INDONESIA_ROAD_BBOX, 'Indonesia', tally, path =>
    writeRoadAadt(path, row => {
      const traffic = match(row)
      return traffic ? { countBasis: traffic.countBasis, observationId: traffic.observationId, light: traffic.light, medium: traffic.medium, heavy: traffic.heavy,
        moto: traffic.moto, sourceId: traffic.kind === 'lhrt' ? SOURCE_ID_ID_NATIONAL_ROADS : SOURCE_ID_ID_ROAD_CLASSIFICATION_FALLBACK } : null
    }, row => {
      const kind = match(row)?.kind
      if (kind === 'lhrt') tally.matchedLhrt++
      else if (kind === 'toll') tally.matchedToll++
      else if (kind === 'regional') tally.matchedRegional++
      else if (kind === 'national') tally.matchedNational++
    }, INDONESIA_ROAD_COVERAGE, {
      sourceIds: [SOURCE_ID_ID_NATIONAL_ROADS, SOURCE_ID_ID_ROAD_CLASSIFICATION_FALLBACK], when: row => { const traffic = match(row); return traffic === null ||
        (traffic.kind === 'lhrt' ? SOURCE_ID_ID_NATIONAL_ROADS : SOURCE_ID_ID_ROAD_CLASSIFICATION_FALLBACK) !== row.existingSourceId },
    }))
  return { sourceRows: source.sourceRows, sourceLines: source.sourceLines,
    invalidGeometrySkipped: source.invalidGeometrySkipped,
    ...counters, ...tally }
}

runRoadLoaderCli(import.meta.url, runIndonesiaRoadEnrichment)
