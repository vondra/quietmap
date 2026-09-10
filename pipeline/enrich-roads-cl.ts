/** Enrich Chile roads from pinned TMDA observations and Red Vial classifications. */

import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { listPreparedSquares } from './lib/prepared-grid.js'
import { CHILE_ROAD_BBOX, CHILE_ROAD_COVERAGE, loadChileRoadSource, matchChileRoad } from './lib/roads-cl-source.js'
import { parseRoadLoaderArguments, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { SOURCE_ID_CL_ROAD_CLASSIFICATION_FALLBACK, SOURCE_ID_CL_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'

export async function runChileRoadEnrichment(options: RoadLoaderArguments) {
  const source = loadChileRoadSource(options)
  const squares = listPreparedSquares(options.preparedDirectory, CHILE_ROAD_BBOX)
  if (squares.length === 0) throw new Error(`no Chile roads.arrow squares found under ${options.preparedDirectory}`)
  const result = { sourceRows: source.sourceRows, sourceLines: source.sourceLines, tmdaPoints: source.tmdaPoints,
    invalidGeometrySkipped: source.invalidGeometrySkipped, unavailableTrafficSkipped: source.unavailableTrafficSkipped,
    rows: 0, matched: 0, retracted: 0, skipped: 0, skippedForeign: 0, squares: squares.length,
    squaresUpdated: 0, matchedTmda: 0, matchedNetwork: 0 }
  const match = (row: RoadRow) => matchChileRoad(row, source)
  for (const square of squares) {
    const write = await writeRoadAadt(resolve(options.preparedDirectory, square, 'roads.arrow'), row => {
      const traffic = match(row)
      return traffic ? { light: traffic.light, medium: traffic.medium, heavy: traffic.heavy,
        moto: traffic.moto, sourceId: traffic.kind === 'tmda' ? SOURCE_ID_CL_NATIONAL_ROADS : SOURCE_ID_CL_ROAD_CLASSIFICATION_FALLBACK } : null
    }, row => { if (match(row)?.kind === 'tmda') result.matchedTmda++; else result.matchedNetwork++ },
    CHILE_ROAD_COVERAGE, { sourceIds: [SOURCE_ID_CL_NATIONAL_ROADS, SOURCE_ID_CL_ROAD_CLASSIFICATION_FALLBACK],
      when: row => { const traffic = match(row); return traffic === null ||
        (traffic.kind === 'tmda' ? SOURCE_ID_CL_NATIONAL_ROADS : SOURCE_ID_CL_ROAD_CLASSIFICATION_FALLBACK) !== row.existingSourceId } })
    result.rows += write.rows; result.matched += write.matched; result.retracted += write.retracted
    result.skipped += write.skipped; result.skippedForeign += write.skippedForeign
    if (write.updated) result.squaresUpdated++
  }
  return result
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  runChileRoadEnrichment(parseRoadLoaderArguments(process.argv.slice(2), 'enrich-roads-cl.ts'))
    .then(result => console.log(JSON.stringify(result)))
    .catch((error: unknown) => { console.error(error instanceof Error ? error.message : error); process.exitCode = 1 })
}
