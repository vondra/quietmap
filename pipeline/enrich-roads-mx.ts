/** Enrich z9 Mexican roads with SICT/IMT Datos Viales 2025 TDPA. */

import { shouldOverwrite } from './lib/provenance.js'
import { runRoadLoaderCli, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { loadMexicanSictSource, splitMexicanTdpa, type MexicanSictSegment } from './lib/roads-mx-source.js'
import { pointToPolylineDist } from './lib/spatial.js'
import { SOURCE_ID_MX_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { writeNationalRoadSquares } from './lib/square-pool.js'

const SOURCE_ID = SOURCE_ID_MX_NATIONAL_ROADS
const MEXICO_BBOX = [14.3, -117.6, 32.9, -86.4] as const
const COVERED_ROAD_CLASSES: ReadonlySet<number> = new Set([0, 1, 2, 3])
const GRID_SCALE = 100
const GRID_STEP_DEGREES = 0.004
const MATCH_RADIUS_METRES = 200

type LineGrid = ReadonlyMap<string, readonly MexicanSictSegment[]>
const key = (latitude: number, longitude: number) =>
  `${Math.floor(latitude * GRID_SCALE)}_${Math.floor(longitude * GRID_SCALE)}`

/** Index every crossed grid cell so long sparse source edges remain discoverable. */
export function buildMexicanLineGrid(segments: readonly MexicanSictSegment[]): LineGrid {
  const grid = new Map<string, MexicanSictSegment[]>()
  for (const segment of segments) {
    const seen = new Set<string>()
    const add = (latitude: number, longitude: number) => {
      const cell = key(latitude, longitude)
      if (seen.has(cell)) return
      seen.add(cell)
      const bucket = grid.get(cell)
      if (bucket) bucket.push(segment)
      else grid.set(cell, [segment])
    }
    for (const line of segment.lines) {
      for (let index = 0; index < line.length; index++) {
        const [longitude, latitude] = line[index]
        add(latitude, longitude)
        if (index + 1 === line.length) continue
        const [nextLongitude, nextLatitude] = line[index + 1]
        const steps = Math.ceil(Math.max(Math.abs(nextLatitude - latitude),
          Math.abs(nextLongitude - longitude)) / GRID_STEP_DEGREES)
        for (let step = 1; step < steps; step++) {
          add(latitude + (nextLatitude - latitude) * step / steps,
            longitude + (nextLongitude - longitude) * step / steps)
        }
      }
    }
  }
  return grid
}

export function matchMexicanSict(row: RoadRow, grid: LineGrid): MexicanSictSegment | null {
  if (!COVERED_ROAD_CLASSES.has(row.roadClass)) return null
  const baseLatitude = Math.floor(row.midLat * GRID_SCALE)
  const baseLongitude = Math.floor(row.midLon * GRID_SCALE)
  let closest: MexicanSictSegment | null = null
  let closestDistance = MATCH_RADIUS_METRES
  const visited = new Set<MexicanSictSegment>()
  for (let latitudeOffset = -1; latitudeOffset <= 1; latitudeOffset++) {
    for (let longitudeOffset = -1; longitudeOffset <= 1; longitudeOffset++) {
      const candidates = grid.get(`${baseLatitude + latitudeOffset}_${baseLongitude + longitudeOffset}`) ?? []
      for (const segment of candidates) {
        if (visited.has(segment) || ((segment.allowedRoadClassMask >> row.roadClass) & 1) === 0) continue
        visited.add(segment)
        const distance = Math.min(...segment.lines.map(line =>
          pointToPolylineDist(row.midLat, row.midLon, line)))
        if (distance < closestDistance) {
          closest = segment
          closestDistance = distance
        }
      }
    }
  }
  return closest
}

export async function enrichMexicanRoads(
  preparedDirectory: string,
  segments: readonly MexicanSictSegment[],
) {
  if (segments.length === 0) throw new Error('Mexican SICT source has no usable measurements')
  const grid = buildMexicanLineGrid(segments)
  const match = (row: RoadRow) => matchMexicanSict(row, grid)
  return writeNationalRoadSquares(preparedDirectory, MEXICO_BBOX, 'Mexican', {}, path =>
    writeRoadAadt(path, row => {
      if (!shouldOverwrite(row.existingSourceId, SOURCE_ID)) return null
      const segment = match(row)
      return segment ? { countBasis: segment.countBasis, observationId: segment.observationId, ...splitMexicanTdpa(segment.total, segment.fractions), sourceId: SOURCE_ID } : null
    }, undefined, COVERED_ROAD_CLASSES,
    { sourceIds: [SOURCE_ID], when: row => match(row) === null }))
}

export async function runMexicanRoadEnrichment(options: RoadLoaderArguments) {
  const source = loadMexicanSictSource(options)
  return { sourceRows: source.sourceRows, compositionRows: source.compositionRows,
    segments: source.segments.length, invalidGeometrySkipped: source.invalidGeometrySkipped,
    unavailableTrafficSkipped: source.unavailableTrafficSkipped,
    missingCompositionRows: source.missingCompositionRows,
    fallbackCompositionRows: source.fallbackCompositionRows,
    ...await enrichMexicanRoads(options.preparedDirectory, source.segments) }
}

runRoadLoaderCli(import.meta.url, runMexicanRoadEnrichment)
