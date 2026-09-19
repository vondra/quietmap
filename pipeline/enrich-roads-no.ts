/** Enrich z9 Norwegian roads with NVDB Trafikkmengde measurements. */

import { roadObservation } from './lib/road-observation.js'
import { shouldOverwrite } from './lib/provenance.js'
import { runRoadLoaderCli, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { loadNorwegianNvdbSource, type NorwegianNvdbSegment } from './lib/roads-no-source.js'
import { buildOneHundredthDegreePointGrid } from './lib/spatial.js'
import { SOURCE_ID_NO_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { nearestCountWithin200Metres, writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { writeNationalRoadSquares } from './lib/square-pool.js'

const SOURCE_ID = SOURCE_ID_NO_NATIONAL_ROADS
const NORWAY_BBOX = [57.9, 4.5, 71.2, 31.2] as const
const COVERED_ROAD_CLASSES: ReadonlySet<number> = new Set([0, 1, 2, 3, 4, 10, 11, 12])

export async function enrichNorwegianRoads(
  preparedDirectory: string,
  segments: readonly NorwegianNvdbSegment[],
) {
  if (segments.length === 0) throw new Error('Norwegian NVDB source has no usable measurements')
  const grid = buildOneHundredthDegreePointGrid(segments)
  const match = (row: RoadRow): NorwegianNvdbSegment | null => nearestCountWithin200Metres(row, grid)
  return writeNationalRoadSquares(preparedDirectory, NORWAY_BBOX, 'Norwegian', {}, path =>
    writeRoadAadt(
      path,
      row => {
        if (!shouldOverwrite(row.existingSourceId, SOURCE_ID)) return null
        const segment = match(row)
        return segment ? { ...roadObservation(String(segment.sourceId), 'both-directions'),
          light: segment.light, medium: segment.medium, heavy: segment.heavy,
          moto: segment.moto, sourceId: SOURCE_ID,
        } : null
      },
      undefined,
      COVERED_ROAD_CLASSES,
      { sourceIds: [SOURCE_ID], when: row =>
        !COVERED_ROAD_CLASSES.has(row.roadClass) || match(row) === null },
    ))
}

export async function runNorwegianRoadEnrichment(options: RoadLoaderArguments) {
  const source = loadNorwegianNvdbSource(options)
  return { sourceRows: source.sourceRows, segments: source.segments.length,
    unsupportedRoadCategorySkipped: source.unsupportedRoadCategorySkipped,
    invalidGeometrySkipped: source.invalidGeometrySkipped,
    ...await enrichNorwegianRoads(options.preparedDirectory, source.segments) }
}

runRoadLoaderCli(import.meta.url, runNorwegianRoadEnrichment)
