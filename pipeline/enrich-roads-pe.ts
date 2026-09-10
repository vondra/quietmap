/** Enrich Peru roads from pinned MTC Red Vial classifications and dIMD. */

import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { listPreparedSquares } from './lib/prepared-grid.js'
import { loadPeruRoadSource, matchPeruRoad, PERU_ROAD_BBOX, PERU_ROAD_COVERAGE } from './lib/roads-pe-source.js'
import { parseRoadLoaderArguments, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { SOURCE_ID_PE_ROAD_CLASSIFICATION_FALLBACK, SOURCE_ID_PE_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'

export async function runPeruRoadEnrichment(options: RoadLoaderArguments) {
  const source = loadPeruRoadSource(options)
  const squares = listPreparedSquares(options.preparedDirectory, PERU_ROAD_BBOX)
  if (squares.length === 0) throw new Error(`no Peru roads.arrow squares found under ${options.preparedDirectory}`)
  const result = { sourceRows: source.sourceRows, sourceLines: source.sourceLines,
    invalidGeometrySkipped: source.invalidGeometrySkipped, observedTrafficLines: source.observedTrafficLines,
    rows: 0, matched: 0, retracted: 0, skipped: 0, skippedForeign: 0, squares: squares.length,
    squaresUpdated: 0, matchedImd: 0, matchedNetwork: 0 }
  const match = (row: RoadRow) => matchPeruRoad(row, source)
  for (const square of squares) {
    const write = await writeRoadAadt(resolve(options.preparedDirectory, square, 'roads.arrow'), row => {
      const traffic = match(row)
      return traffic ? { light: traffic.light, medium: traffic.medium, heavy: traffic.heavy,
        moto: traffic.moto, sourceId: traffic.kind === 'imd' ? SOURCE_ID_PE_NATIONAL_ROADS : SOURCE_ID_PE_ROAD_CLASSIFICATION_FALLBACK } : null
    }, row => { if (match(row)?.kind === 'imd') result.matchedImd++; else result.matchedNetwork++ },
    PERU_ROAD_COVERAGE, { sourceIds: [SOURCE_ID_PE_NATIONAL_ROADS, SOURCE_ID_PE_ROAD_CLASSIFICATION_FALLBACK], when: row => { const traffic = match(row); return traffic === null ||
        (traffic.kind === 'imd' ? SOURCE_ID_PE_NATIONAL_ROADS : SOURCE_ID_PE_ROAD_CLASSIFICATION_FALLBACK) !== row.existingSourceId } })
    result.rows += write.rows; result.matched += write.matched; result.retracted += write.retracted
    result.skipped += write.skipped; result.skippedForeign += write.skippedForeign
    if (write.updated) result.squaresUpdated++
  }
  return result
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  runPeruRoadEnrichment(parseRoadLoaderArguments(process.argv.slice(2), 'enrich-roads-pe.ts'))
    .then(result => console.log(JSON.stringify(result)))
    .catch((error: unknown) => { console.error(error instanceof Error ? error.message : error); process.exitCode = 1 })
}
