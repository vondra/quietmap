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
// A row takes a national section only along it: at the former 30 km, 2,467 km of rows took a
// section more than 5 km away (Warsaw S8 at 27,857 from sections 11-18 km off, r260919).
const MAXIMUM_NATIONAL_DISTANCE_M = 500
const COVERED_ROAD_CLASSES: ReadonlySet<number> = new Set([0, 1, 2, 3, 4, 10, 11, 12])

export interface PolishRoadIndex {
  national: ReadonlyMap<string, readonly PolishGprSegment[]>
  /** Provincial sections carry no geometry, so a ref stands for its median section by total. */
  provincialMedian: ReadonlyMap<string, PolishGprSegment>
}

const total = (segment: PolishGprSegment): number => segment.light + segment.medium + segment.heavy + segment.moto

/** A lettered section (S8F, DK12N, 62B) is a carriageway or variant of its numbered road: OSM tags the number. */
function indexKeys(ref: string): Set<string> {
  const keys = new Set<string>()
  for (const token of ref.split(';')) {
    const key = token.trim().toUpperCase().replace(/\s+/g, '')
    if (!key) continue
    keys.add(key)
    const lettered = /^([A-Z]*\d+)[A-Z]$/.exec(key)
    if (lettered) keys.add(lettered[1])
  }
  return keys
}

export function indexPolishGpr(segments: readonly PolishGprSegment[]): PolishRoadIndex {
  const national = new Map<string, PolishGprSegment[]>(), provincial = new Map<string, PolishGprSegment[]>()
  for (const segment of segments) {
    const byRef = segment.isProvincial ? provincial : national
    for (const key of indexKeys(segment.ref)) {
      const bucket = byRef.get(key)
      if (bucket) bucket.push(segment)
      else byRef.set(key, [segment])
    }
  }
  const provincialMedian = new Map([...provincial].map(([ref, sections]) => {
    const sorted = [...sections].sort((a, b) => total(a) - total(b) || (a.sourceId < b.sourceId ? -1 : 1))
    return [ref, sorted[(sorted.length - 1) >> 1]] as const
  }))
  return { national, provincialMedian }
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


/** The nearest national polyline of the row's ref wins; a geometry-less provincial ref gives its median section. */
export function matchPolishGpr(
  row: RoadRow,
  index: PolishRoadIndex,
): PolishGprSegment | null {
  if (isSlipRoadClass(row.roadClass)) return null
  let closestNational: PolishGprSegment | null = null
  let closestDistance = MAXIMUM_NATIONAL_DISTANCE_M
  let provincial: PolishGprSegment | null = null
  for (const ref of candidateRefs(row.ref)) {
    provincial ??= index.provincialMedian.get(ref) ?? null
    for (const segment of index.national.get(ref) ?? []) {
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
          // GPR counts every category; a provincial ref median is not this row's own section.
          moto: segment.moto, sourceId: SOURCE_ID, estimatedClasses: segment.isProvincial ? 15 : 0,
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
