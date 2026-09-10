/** Enrich Indonesia roads from pinned Bina Marga LHRT and classified networks. */

import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { listPreparedSquares } from './lib/prepared-grid.js'
import { parseRoadLoaderArguments, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { INDONESIA_ROAD_BBOX, INDONESIA_ROAD_COVERAGE, loadIndonesiaRoadSource,
  matchIndonesiaRoad } from './lib/roads-id-source.js'
import { SOURCE_ID_ID_ROAD_CLASSIFICATION_FALLBACK, SOURCE_ID_ID_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'

export async function runIndonesiaRoadEnrichment(options: RoadLoaderArguments) {
  const source = loadIndonesiaRoadSource(options)
  const squares = listPreparedSquares(options.preparedDirectory, INDONESIA_ROAD_BBOX)
  if (squares.length === 0) throw new Error(`no Indonesia roads.arrow squares found under ${options.preparedDirectory}`)
  const result = { sourceRows: source.sourceRows, sourceLines: source.sourceLines,
    invalidGeometrySkipped: source.invalidGeometrySkipped, rows: 0, matched: 0, retracted: 0,
    skipped: 0, skippedForeign: 0, squares: squares.length, squaresUpdated: 0,
    matchedLhrt: 0, matchedToll: 0, matchedRegional: 0, matchedNational: 0 }
  const match = (row: RoadRow) => matchIndonesiaRoad(row, source)
  for (const square of squares) {
    const write = await writeRoadAadt(resolve(options.preparedDirectory, square, 'roads.arrow'), row => {
      const traffic = match(row)
      return traffic ? { light: traffic.light, medium: traffic.medium, heavy: traffic.heavy,
        moto: traffic.moto, sourceId: traffic.kind === 'lhrt' ? SOURCE_ID_ID_NATIONAL_ROADS : SOURCE_ID_ID_ROAD_CLASSIFICATION_FALLBACK } : null
    }, row => {
      const kind = match(row)?.kind
      if (kind === 'lhrt') result.matchedLhrt++
      else if (kind === 'toll') result.matchedToll++
      else if (kind === 'regional') result.matchedRegional++
      else if (kind === 'national') result.matchedNational++
    }, INDONESIA_ROAD_COVERAGE, {
      sourceIds: [SOURCE_ID_ID_NATIONAL_ROADS, SOURCE_ID_ID_ROAD_CLASSIFICATION_FALLBACK], when: row => { const traffic = match(row); return traffic === null ||
        (traffic.kind === 'lhrt' ? SOURCE_ID_ID_NATIONAL_ROADS : SOURCE_ID_ID_ROAD_CLASSIFICATION_FALLBACK) !== row.existingSourceId },
    })
    result.rows += write.rows; result.matched += write.matched; result.retracted += write.retracted
    result.skipped += write.skipped; result.skippedForeign += write.skippedForeign
    if (write.updated) result.squaresUpdated++
  }
  return result
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  runIndonesiaRoadEnrichment(parseRoadLoaderArguments(process.argv.slice(2), 'enrich-roads-id.ts'))
    .then(result => console.log(JSON.stringify(result)))
    .catch((error: unknown) => { console.error(error instanceof Error ? error.message : error); process.exitCode = 1 })
}
