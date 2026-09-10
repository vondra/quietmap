/** Enrich Saudi roads from pinned MoT counts, Riyadh PMS and national atlas. */

import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { listPreparedSquares } from './lib/prepared-grid.js'
import { loadSaudiRoadSource, matchSaudiRoad, SAUDI_ROAD_BBOX, SAUDI_ROAD_COVERAGE } from './lib/roads-sa-source.js'
import { parseRoadLoaderArguments, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { SOURCE_ID_SA_ROAD_CLASSIFICATION_FALLBACK, SOURCE_ID_SA_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'

export async function runSaudiRoadEnrichment(options: RoadLoaderArguments) {
  const source = loadSaudiRoadSource(options)
  const squares = listPreparedSquares(options.preparedDirectory, SAUDI_ROAD_BBOX)
  if (squares.length === 0) throw new Error(`no Saudi roads.arrow squares found under ${options.preparedDirectory}`)
  const result = { sourceRows: source.sourceRows, sourceLines: source.sourceLines, stations: source.stations,
    unavailableTrafficRows: source.unavailableTrafficRows, invalidGeometrySkipped: source.invalidGeometrySkipped,
    rows: 0, matched: 0, retracted: 0, skipped: 0, skippedForeign: 0, squares: squares.length,
    squaresUpdated: 0, matchedMot: 0, matchedRiyadh: 0, matchedAtlas: 0 }
  const match = (row: RoadRow) => matchSaudiRoad(row, source)
  for (const square of squares) {
    const write = await writeRoadAadt(resolve(options.preparedDirectory, square, 'roads.arrow'), row => {
      const traffic = match(row)
      return traffic ? { light: traffic.light, medium: traffic.medium, heavy: traffic.heavy,
        moto: traffic.moto, sourceId: traffic.kind === 'mot' ? SOURCE_ID_SA_NATIONAL_ROADS : SOURCE_ID_SA_ROAD_CLASSIFICATION_FALLBACK } : null
    }, row => { const kind = match(row)?.kind; if (kind === 'mot') result.matchedMot++
      else if (kind === 'riyadh') result.matchedRiyadh++; else result.matchedAtlas++ },
    SAUDI_ROAD_COVERAGE, { sourceIds: [SOURCE_ID_SA_NATIONAL_ROADS, SOURCE_ID_SA_ROAD_CLASSIFICATION_FALLBACK], when: row => { const traffic = match(row); return traffic === null ||
        (traffic.kind === 'mot' ? SOURCE_ID_SA_NATIONAL_ROADS : SOURCE_ID_SA_ROAD_CLASSIFICATION_FALLBACK) !== row.existingSourceId } })
    result.rows += write.rows; result.matched += write.matched; result.retracted += write.retracted
    result.skipped += write.skipped; result.skippedForeign += write.skippedForeign
    if (write.updated) result.squaresUpdated++
  }
  return result
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  runSaudiRoadEnrichment(parseRoadLoaderArguments(process.argv.slice(2), 'enrich-roads-sa.ts'))
    .then(result => console.log(JSON.stringify(result)))
    .catch((error: unknown) => { console.error(error instanceof Error ? error.message : error); process.exitCode = 1 })
}
