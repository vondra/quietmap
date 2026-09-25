/** Enrich z9 Swedish roads with Trafikverket NVDB Trafik measurements. */

import { shouldOverwrite } from './lib/provenance.js'
import { runRoadLoaderCli, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { loadSwedishNvdbSource, type SwedishNvdbObservation } from './lib/roads-se-source.js'
import { SOURCE_ID_SE_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { roadClassTakesCount, writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { writeNationalRoadSquares } from './lib/square-pool.js'
import {
  buildOneHundredthDegreeSegmentGrid,
  pointGridCandidates,
  pointToSegmentDist,
  runsAlongSegment,
  type SegmentCoordinates,
} from './lib/spatial.js'

const SOURCE_ID = SOURCE_ID_SE_NATIONAL_ROADS
const SWEDEN_BBOX = [55.3, 10.9, 69.1, 24.2] as const
const COVERED_ROAD_CLASSES: ReadonlySet<number> = new Set([0, 1, 2, 3, 4, 10, 11, 12])
const MAXIMUM_DISTANCE_METRES = 50

interface ObservationEdge extends SegmentCoordinates {
  observation: SwedishNvdbObservation
}

export interface SwedishNvdbIndex {
  edges: ReadonlyMap<string, readonly ObservationEdge[]>
}

export function indexSwedishNvdb(observations: readonly SwedishNvdbObservation[]): SwedishNvdbIndex {
  const edges: ObservationEdge[] = []
  for (const observation of observations) {
    for (let index = 1; index < observation.line.length; index++) {
      const [startLon, startLat] = observation.line[index - 1]
      const [endLon, endLat] = observation.line[index]
      edges.push({
        observation,
        startLatitude: startLat,
        startLongitude: startLon,
        endLatitude: endLat,
        endLongitude: endLon,
      })
    }
  }
  return { edges: buildOneHundredthDegreeSegmentGrid(edges) }
}

/** A row takes the nearest link part running along it. NVDB publishes no road number or
 *  functional class, so the line, the heading and the nearest win alone: the publisher
 *  geometry sits on the same carriageway the OSM row traces, metres away, while a
 *  parallel road runs tens of metres off. Slip roads take nothing: without a ramp flag
 *  a link part cannot prove it was counted on the slip rather than the mainline. */
export function matchSwedishNvdb(row: RoadRow, index: SwedishNvdbIndex): SwedishNvdbObservation | null {
  let closest: SwedishNvdbObservation | null = null
  let closestDistance = MAXIMUM_DISTANCE_METRES
  let closestId = ''
  for (const edge of pointGridCandidates(row.midLat, row.midLon, MAXIMUM_DISTANCE_METRES, index.edges)) {
    const { observation } = edge
    if (!runsAlongSegment(row, edge)) continue
    if (!roadClassTakesCount(row.roadClass, observation)) continue
    const distance = pointToSegmentDist(
      row.midLat,
      row.midLon,
      edge.startLatitude,
      edge.startLongitude,
      edge.endLatitude,
      edge.endLongitude,
    )
    if (
      distance > MAXIMUM_DISTANCE_METRES ||
      distance > closestDistance ||
      (distance === closestDistance && observation.observationId >= closestId)
    )
      continue
    closest = observation
    closestDistance = distance
    closestId = observation.observationId
  }
  return closest
}

export async function enrichSwedishRoads(preparedDirectory: string, observations: readonly SwedishNvdbObservation[]) {
  if (observations.length === 0) throw new Error('Swedish NVDB source has no usable measurements')
  const index = indexSwedishNvdb(observations)
  const match = (row: RoadRow): SwedishNvdbObservation | null => matchSwedishNvdb(row, index)
  return writeNationalRoadSquares(preparedDirectory, SWEDEN_BBOX, 'Swedish', {}, path =>
    writeRoadAadt(
      path,
      row => {
        if (!shouldOverwrite(row.existingSourceId, SOURCE_ID)) return null
        const observation = match(row)
        return observation
          ? {
              countBasis: observation.countBasis,
              observationId: observation.observationId,
              light: observation.light,
              medium: observation.medium,
              heavy: observation.heavy,
              // Loops classify the length classes; only the 1 % moto share is imputed.
              moto: observation.moto,
              sourceId: SOURCE_ID,
              estimatedClasses: 8,
            }
          : null
      },
      undefined,
      COVERED_ROAD_CLASSES,
      {
        sourceIds: [SOURCE_ID],
        when: row => !COVERED_ROAD_CLASSES.has(row.roadClass) || match(row) === null,
      },
    ),
  )
}

export async function runSwedishRoadEnrichment(options: RoadLoaderArguments) {
  const source = loadSwedishNvdbSource(options)
  return {
    sourceRows: source.sourceRows,
    observations: source.observations.length,
    twoWayObservations: source.twoWayObservations,
    directionalObservations: source.directionalObservations,
    assessedSkipped: source.assessedSkipped,
    incompleteSplitsSkipped: source.incompleteSplitsSkipped,
    zeroTrafficSkipped: source.zeroTrafficSkipped,
    inconsistentClassesSkipped: source.inconsistentClassesSkipped,
    invalidGeometrySkipped: source.invalidGeometrySkipped,
    ...(await enrichSwedishRoads(options.preparedDirectory, source.observations)),
  }
}

runRoadLoaderCli(import.meta.url, runSwedishRoadEnrichment)
