/** Enrich z9 Dutch roads with RWS INWEVA 2024 section measurements. */

import { shouldOverwrite } from './lib/provenance.js'
import { listPreparedSquares } from './lib/prepared-grid.js'
import { runRoadLoaderCli, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { loadDutchInwevaSource, type DutchInwevaObservation } from './lib/roads-nl-source.js'
import { SOURCE_ID_NL_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { applyRoadTimeProfiles, roadClassTakesCount, writeRoadAadt, type RoadRow, type RoadTimeProfileEntry } from './lib/roads-arrow.js'
import { writeNationalRoadSquares } from './lib/square-pool.js'
import {
  buildOneHundredthDegreeSegmentGrid,
  pointGridCandidates,
  pointToSegmentDist,
  runsAlongSegment,
  type SegmentCoordinates,
} from './lib/spatial.js'

const SOURCE_ID = SOURCE_ID_NL_NATIONAL_ROADS
const NETHERLANDS_BBOX = [50.7, 3.2, 53.7, 7.3] as const
const COVERED_ROAD_CLASSES: ReadonlySet<number> = new Set([0, 1, 2, 3, 4, 10, 11, 12])
const MAXIMUM_DISTANCE_METRES = 50

interface ObservationEdge extends SegmentCoordinates {
  observation: DutchInwevaObservation
}

export interface DutchInwevaIndex {
  edges: ReadonlyMap<string, readonly ObservationEdge[]>
}

export function indexDutchInweva(observations: readonly DutchInwevaObservation[]): DutchInwevaIndex {
  const edges: ObservationEdge[] = []
  for (const observation of observations) {
    for (const line of observation.lines) {
      for (let index = 1; index < line.length; index++) {
        const [startLon, startLat] = line[index - 1]
        const [endLon, endLat] = line[index]
        edges.push({
          observation,
          startLatitude: startLat,
          startLongitude: startLon,
          endLatitude: endLat,
          endLongitude: endLon,
        })
      }
    }
  }
  return { edges: buildOneHundredthDegreeSegmentGrid(edges) }
}

function rowRefs(ref: string | null): Set<string> {
  const refs = new Set<string>()
  for (const token of (ref ?? '').split(/[;,]/)) {
    const key = token.trim().toUpperCase().replace(/\s+/g, '')
    if (key) refs.add(key)
  }
  return refs
}

/** A row takes the nearest section of its own road number running along it; ramps carry no
 *  OSM ref, so they match by line and slip class alone. A cross street within 50 m never
 *  qualifies: the section and the row must run within 30 degrees of each other. */
export function matchDutchInweva(row: RoadRow, index: DutchInwevaIndex): DutchInwevaObservation | null {
  let closest: DutchInwevaObservation | null = null
  let closestDistance = MAXIMUM_DISTANCE_METRES
  let closestId = ''
  const refs = rowRefs(row.ref)
  for (const edge of pointGridCandidates(row.midLat, row.midLon, MAXIMUM_DISTANCE_METRES, index.edges)) {
    const { observation } = edge
    if (!observation.isRamp && ![...observation.refs].some(ref => refs.has(ref))) continue
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

export async function enrichDutchRoads(preparedDirectory: string, observations: readonly DutchInwevaObservation[]) {
  if (observations.length === 0) throw new Error('Dutch INWEVA source has no usable measurements')
  const index = indexDutchInweva(observations)
  const match = (row: RoadRow): DutchInwevaObservation | null => matchDutchInweva(row, index)
  return writeNationalRoadSquares(preparedDirectory, NETHERLANDS_BBOX, 'Dutch', {}, path =>
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
              // Loops count the length classes; only the 1 % moto share is imputed.
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

/** RWS INWEVA 2024 weekdag-gemiddelde section intensities, Nationaal Georegister
 *  record 93e99016-9b53-45d6-8b3c-fc9bf8086256 (CC0, "Geen beperkingen"). */
export const INWEVA_PROFILE_SOURCE_URL =
  'https://www.nationaalgeoregister.nl/geonetwork/srv/dut/catalog.search#/metadata/93e99016-9b53-45d6-8b3c-fc9bf8086256'
const INWEVA_PROFILE_WINDOW = '2024-01..2024-12'
// 2024 is a leap year; weekdag-gemiddelde averages all days (RWS Toelichting
// INWEVA distinguishes it from the werkdag average). Per-section valid-day
// coverage is unpublished — each entry's status carries its kwal flags.
const INWEVA_PROFILE_DAYS = 366

/** One dictionary entry per observation with published periods, with its
 *  1-based position for the row matcher (observations without periods match 0). */
export function inwevaProfileEntries(observations: readonly DutchInwevaObservation[]): {
  entries: RoadTimeProfileEntry[]
  indexOf: ReadonlyMap<string, number>
} {
  const entries: RoadTimeProfileEntry[] = []
  const indexOf = new Map<string, number>()
  for (const observation of observations) {
    if (!observation.timeProfile) continue
    indexOf.set(observation.observationId, entries.length + 1)
    entries.push({
      station: observation.observationId.replace(/^inweva2024:/, ''),
      window: INWEVA_PROFILE_WINDOW,
      days: INWEVA_PROFILE_DAYS,
      status: observation.timeProfile.status,
      profile: observation.timeProfile.shares,
    })
  }
  return { entries, indexOf }
}

/** Stamp observed INWEVA period profiles next to (never onto) the AADT columns. */
export async function enrichDutchTimeProfiles(
  preparedDirectory: string,
  observations: readonly DutchInwevaObservation[],
) {
  const { entries, indexOf } = inwevaProfileEntries(observations)
  if (entries.length === 0) return { rows: 0, matched: 0, squaresUpdated: 0 }
  const index = indexDutchInweva(observations)
  return applyRoadTimeProfiles(
    preparedDirectory,
    listPreparedSquares(preparedDirectory, NETHERLANDS_BBOX),
    INWEVA_PROFILE_SOURCE_URL,
    entries,
    row => {
      const matched = matchDutchInweva(row, index)
      return matched ? (indexOf.get(matched.observationId) ?? 0) : 0
    },
  )
}

export async function runDutchRoadEnrichment(options: RoadLoaderArguments) {
  const source = loadDutchInwevaSource(options)
  const traffic = await enrichDutchRoads(options.preparedDirectory, source.observations)
  const profiles = await enrichDutchTimeProfiles(options.preparedDirectory, source.observations)
  return {
    sourceRows: source.sourceRows,
    observations: source.observations.length,
    pairedSections: source.pairedSections,
    lonelySections: source.lonelySections,
    derivedSectionsSkipped: source.derivedSectionsSkipped,
    missingValuesSkipped: source.missingValuesSkipped,
    unsupportedBaansoortSkipped: source.unsupportedBaansoortSkipped,
    lonelyNationalRoadSkipped: source.lonelyNationalRoadSkipped,
    missingRefSkipped: source.missingRefSkipped,
    invalidGeometrySkipped: source.invalidGeometrySkipped,
    profileEntries: inwevaProfileEntries(source.observations).entries.length,
    profileMatched: profiles.matched,
    profileSquaresUpdated: profiles.squaresUpdated,
    ...traffic,
  }
}

runRoadLoaderCli(import.meta.url, runDutchRoadEnrichment)
