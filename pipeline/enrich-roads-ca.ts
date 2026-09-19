/** Enrich z9 Quebec roads with MTQ DJMA measurements. */

import { shouldOverwrite } from './lib/provenance.js'
import { runRoadLoaderCli, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import {
  loadQuebecDjmaSource, normalizeQuebecOsmRef, type QuebecDjmaSection,
} from './lib/roads-ca-source.js'
import { haversineM } from './lib/spatial.js'
import { SOURCE_ID_CA_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { roadClassTakesCount, writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { writeNationalRoadSquares } from './lib/square-pool.js'

const SOURCE_ID = SOURCE_ID_CA_NATIONAL_ROADS
const QUEBEC_BBOX = [44.5, -80, 63, -56] as const
const COVERED_ROAD_CLASSES: ReadonlySet<number> = new Set([0, 1, 2, 3, 4, 10, 11, 12])
const MAXIMUM_MATCH_DISTANCE_M = 25_000

export function indexQuebecDjma(
  sections: readonly QuebecDjmaSection[],
): ReadonlyMap<number, readonly QuebecDjmaSection[]> {
  const byRoute = new Map<number, QuebecDjmaSection[]>()
  for (const section of sections) {
    const bucket = byRoute.get(section.route)
    if (bucket) bucket.push(section)
    else byRoute.set(section.route, [section])
  }
  return byRoute
}

export function matchQuebecDjma(
  row: RoadRow,
  byRoute: ReadonlyMap<number, readonly QuebecDjmaSection[]>,
): QuebecDjmaSection | null {
  if (!COVERED_ROAD_CLASSES.has(row.roadClass)) return null
  const route = normalizeQuebecOsmRef(row.ref ?? '')
  const candidates = route === null ? undefined : byRoute.get(route)
  if (!candidates) return null
  let closest: QuebecDjmaSection | null = null
  let closestDistance = MAXIMUM_MATCH_DISTANCE_M
  for (const section of candidates) {
    if (!roadClassTakesCount(row.roadClass, section)) continue
    const distance = haversineM(row.midLat, row.midLon, section.latitude, section.longitude)
    if (distance < closestDistance) {
      closest = section
      closestDistance = distance
    }
  }
  return closest
}

export async function enrichCanadianRoads(
  preparedDirectory: string,
  sections: readonly QuebecDjmaSection[],
) {
  if (sections.length === 0) throw new Error('Quebec DJMA source has no usable measurements')
  const byRoute = indexQuebecDjma(sections)
  const match = (row: RoadRow) => matchQuebecDjma(row, byRoute)
  return writeNationalRoadSquares(preparedDirectory, QUEBEC_BBOX, 'Quebec', {}, path =>
    writeRoadAadt(path, row => {
      if (!shouldOverwrite(row.existingSourceId, SOURCE_ID)) return null
      const section = match(row)
      return section ? { countBasis: section.countBasis, observationId: section.observationId, light: section.light, medium: section.medium, heavy: section.heavy,
        moto: section.moto, sourceId: SOURCE_ID } : null
    }, undefined, COVERED_ROAD_CLASSES,
    { sourceIds: [SOURCE_ID], when: row => match(row) === null }))
}

export async function runCanadianRoadEnrichment(options: RoadLoaderArguments) {
  const source = loadQuebecDjmaSource(options)
  return { sourceRows: source.sourceRows, sections: source.sections.length,
    unavailableTrafficSkipped: source.unavailableTrafficSkipped,
    invalidRouteSkipped: source.invalidRouteSkipped,
    invalidGeometrySkipped: source.invalidGeometrySkipped,
    ...await enrichCanadianRoads(options.preparedDirectory, source.sections) }
}

runRoadLoaderCli(import.meta.url, runCanadianRoadEnrichment)
