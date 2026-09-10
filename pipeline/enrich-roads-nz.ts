/** Enrich z9 New Zealand roads with NZTA and Auckland Transport AADT. */

import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { listPreparedSquares } from './lib/prepared-grid.js'
import { shouldOverwrite } from './lib/provenance.js'
import { parseRoadLoaderArguments, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { loadNewZealandRoadSources, type NewZealandRoadObservation } from './lib/roads-nz-source.js'
import { buildOneHundredthDegreePointGrid, nearestCompatiblePointWithin200Metres } from './lib/spatial.js'
import { SOURCE_ID_NZ_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { osmRoadClassRank, ROAD_CLASS_RANK_TOLERANCE, writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'

const SOURCE_ID = SOURCE_ID_NZ_NATIONAL_ROADS
const NEW_ZEALAND_BBOX = [-47.5, 165, -34, 179] as const
const COVERED_ROAD_CLASSES: ReadonlySet<number> = new Set([0, 1, 2, 3, 4, 10, 11, 12])

export async function enrichNewZealandRoads(
  preparedDirectory: string,
  observations: readonly NewZealandRoadObservation[],
) {
  if (observations.length === 0) throw new Error('New Zealand road sources have no usable measurements')
  const squares = listPreparedSquares(preparedDirectory, NEW_ZEALAND_BBOX)
  if (squares.length === 0) throw new Error(`no New Zealand roads.arrow squares found under ${preparedDirectory}`)
  const grid = buildOneHundredthDegreePointGrid(observations)
  const match = (row: RoadRow) => nearestCompatiblePointWithin200Metres(
    row.midLat, row.midLon, osmRoadClassRank(row.roadClass), ROAD_CLASS_RANK_TOLERANCE, grid)
  const result = { rows: 0, matched: 0, retracted: 0, skipped: 0, skippedForeign: 0,
    squares: squares.length, squaresUpdated: 0 }
  for (const square of squares) {
    const write = await writeRoadAadt(resolve(preparedDirectory, square, 'roads.arrow'), row => {
      if (!shouldOverwrite(row.existingSourceId, SOURCE_ID)) return null
      const observation = match(row)
      return observation ? { light: observation.light, medium: observation.medium,
        heavy: observation.heavy, moto: observation.moto, sourceId: SOURCE_ID } : null
    }, undefined, COVERED_ROAD_CLASSES,
    { sourceIds: [SOURCE_ID], when: row =>
      !COVERED_ROAD_CLASSES.has(row.roadClass) || match(row) === null })
    result.rows += write.rows
    result.matched += write.matched
    result.retracted += write.retracted
    result.skipped += write.skipped
    result.skippedForeign += write.skippedForeign
    if (write.updated) result.squaresUpdated++
  }
  return result
}

export async function runNewZealandRoadEnrichment(options: RoadLoaderArguments) {
  const source = loadNewZealandRoadSources(options)
  return { nztaRows: source.nztaRows, atRows: source.atRows,
    observations: source.observations.length,
    unavailableTrafficSkipped: source.unavailableTrafficSkipped,
    unsupportedClassSkipped: source.unsupportedClassSkipped,
    invalidGeometrySkipped: source.invalidGeometrySkipped,
    ...await enrichNewZealandRoads(options.preparedDirectory, source.observations) }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  runNewZealandRoadEnrichment(parseRoadLoaderArguments(process.argv.slice(2), 'enrich-roads-nz.ts'))
    .then(result => console.log(JSON.stringify(result)))
    .catch((error: unknown) => {
      console.error(error instanceof Error ? error.message : error)
      process.exitCode = 1
    })
}
