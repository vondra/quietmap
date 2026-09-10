/** Enrich z9 Quebec roads with MTQ DJMA measurements. */

import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { listPreparedSquares } from './lib/prepared-grid.js'
import { shouldOverwrite } from './lib/provenance.js'
import { parseRoadLoaderArguments, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import {
  loadQuebecDjmaSource, normalizeQuebecOsmRef, type QuebecDjmaSection,
} from './lib/roads-ca-source.js'
import { haversineM } from './lib/spatial.js'
import { SOURCE_ID_CA_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import {
  osmRoadClassRank, ROAD_CLASS_RANK_TOLERANCE, writeRoadAadt, type RoadRow,
} from './lib/roads-arrow.js'

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
  const roadRank = osmRoadClassRank(row.roadClass)
  let closest: QuebecDjmaSection | null = null
  let closestDistance = MAXIMUM_MATCH_DISTANCE_M
  for (const section of candidates) {
    if (Math.abs(roadRank - section.rank) > ROAD_CLASS_RANK_TOLERANCE) continue
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
  const squares = listPreparedSquares(preparedDirectory, QUEBEC_BBOX)
  if (squares.length === 0) throw new Error(`no Quebec roads.arrow squares found under ${preparedDirectory}`)
  const byRoute = indexQuebecDjma(sections)
  const match = (row: RoadRow) => matchQuebecDjma(row, byRoute)
  const result = { rows: 0, matched: 0, retracted: 0, skipped: 0, skippedForeign: 0,
    squares: squares.length, squaresUpdated: 0 }
  for (const square of squares) {
    const write = await writeRoadAadt(resolve(preparedDirectory, square, 'roads.arrow'), row => {
      if (!shouldOverwrite(row.existingSourceId, SOURCE_ID)) return null
      const section = match(row)
      return section ? { light: section.light, medium: section.medium, heavy: section.heavy,
        moto: section.moto, sourceId: SOURCE_ID } : null
    }, undefined, COVERED_ROAD_CLASSES,
    { sourceIds: [SOURCE_ID], when: row => match(row) === null })
    result.rows += write.rows
    result.matched += write.matched
    result.retracted += write.retracted
    result.skipped += write.skipped
    result.skippedForeign += write.skippedForeign
    if (write.updated) result.squaresUpdated++
  }
  return result
}

export async function runCanadianRoadEnrichment(options: RoadLoaderArguments) {
  const source = loadQuebecDjmaSource(options)
  return { sourceRows: source.sourceRows, sections: source.sections.length,
    unavailableTrafficSkipped: source.unavailableTrafficSkipped,
    invalidRouteSkipped: source.invalidRouteSkipped,
    invalidGeometrySkipped: source.invalidGeometrySkipped,
    ...await enrichCanadianRoads(options.preparedDirectory, source.sections) }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  runCanadianRoadEnrichment(parseRoadLoaderArguments(process.argv.slice(2), 'enrich-roads-ca.ts'))
    .then(result => console.log(JSON.stringify(result)))
    .catch((error: unknown) => {
      console.error(error instanceof Error ? error.message : error)
      process.exitCode = 1
    })
}
