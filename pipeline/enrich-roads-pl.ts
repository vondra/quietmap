/** Enrich z9 Polish roads with GDDKiA GPR 2020/2021 measurements. */

import { roadObservation } from './lib/road-observation.js'
import { shouldOverwrite } from './lib/provenance.js'
import { runRoadLoaderCli, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { loadPolishGprSource, type PolishGprSegment } from './lib/roads-pl-source.js'
import { pointToPolylineDist } from './lib/spatial.js'
import { SOURCE_ID_PL_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { isSlipRoadClass, writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { writeNationalRoadSquares } from './lib/square-pool.js'

const SOURCE_ID = SOURCE_ID_PL_NATIONAL_ROADS
const POLAND_BBOX = [49, 14, 55, 24.5] as const
const MAXIMUM_NATIONAL_DISTANCE_M = 30_000
const COVERED_ROAD_CLASSES: ReadonlySet<number> = new Set([0, 1, 2, 3, 4, 10, 11, 12])

export interface PolishRoadIndex {
  byRef: ReadonlyMap<string, readonly PolishGprSegment[]>
}

export function indexPolishGpr(segments: readonly PolishGprSegment[]): PolishRoadIndex {
  const byRef = new Map<string, PolishGprSegment[]>()
  for (const segment of segments) {
    const keys = new Set(segment.ref.split(';')
      .map(ref => ref.trim().toUpperCase().replace(/\s+/g, '')).filter(Boolean))
    for (const key of keys) {
      const bucket = byRef.get(key)
      if (bucket) bucket.push(segment)
      else byRef.set(key, [segment])
    }
  }
  return { byRef }
}

function candidateRefs(ref: string | null): string[] {
  const candidates = new Set<string>()
  for (const token of (ref ?? '').split(/[;,]/)) {
    const key = token.trim().toUpperCase().replace(/\s+/g, '')
    if (!key) continue
    candidates.add(key)
    if (/^\d+$/.test(key)) {
      candidates.add(`DK${key}`)
      candidates.add(`DW${key}`)
    }
  }
  return [...candidates]
}


/** National polylines win by distance; geometry-less DW rows retain dev1 ref matching. */
export function matchPolishGpr(
  row: RoadRow,
  index: PolishRoadIndex,
): PolishGprSegment | null {
  if (isSlipRoadClass(row.roadClass)) return null
  let closestNational: PolishGprSegment | null = null
  let closestDistance = MAXIMUM_NATIONAL_DISTANCE_M
  let provincial: PolishGprSegment | null = null
  for (const ref of candidateRefs(row.ref)) {
    for (const segment of index.byRef.get(ref) ?? []) {
      if (segment.isProvincial) {
        provincial ??= segment
        continue
      }
      const distance = pointToPolylineDist(row.midLat, row.midLon, segment.coordinates ?? [])
      if (distance < closestDistance) {
        closestNational = segment
        closestDistance = distance
      }
    }
  }
  return closestNational ?? provincial
}

export async function enrichPolishRoads(
  preparedDirectory: string,
  segments: readonly PolishGprSegment[],
) {
  if (segments.length === 0) throw new Error('Polish GPR source has no usable measurements')
  const index = indexPolishGpr(segments)
  const match = (row: RoadRow): PolishGprSegment | null => matchPolishGpr(row, index)
  return writeNationalRoadSquares(preparedDirectory, POLAND_BBOX, 'Polish', {}, path =>
    writeRoadAadt(
      path,
      row => {
        if (!shouldOverwrite(row.existingSourceId, SOURCE_ID)) return null
        const segment = match(row)
        return segment ? { ...roadObservation(segment.sourceId, 'both-directions'),
          light: segment.light, medium: segment.medium, heavy: segment.heavy,
          moto: segment.moto, sourceId: SOURCE_ID, estimatedClasses: 0, // GPR publishes every category
        } : null
      },
      undefined,
      COVERED_ROAD_CLASSES,
      { sourceIds: [SOURCE_ID], when: row =>
        !COVERED_ROAD_CLASSES.has(row.roadClass) || match(row) === null },
    ))
}

export async function runPolishRoadEnrichment(options: RoadLoaderArguments) {
  const source = loadPolishGprSource(options)
  return { sourceRows: source.sourceRows, segments: source.segments.length,
    zeroTrafficSkipped: source.zeroTrafficSkipped,
    ...await enrichPolishRoads(options.preparedDirectory, source.segments) }
}

runRoadLoaderCli(import.meta.url, runPolishRoadEnrichment)
