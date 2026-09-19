/** Enrich z9 Japanese roads with MLIT census route medians and class fallbacks. */

import type { RoadObservation } from './lib/road-observation.js'
import { runRoadLoaderCli, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { leadingJapaneseRoadDigits, loadJapaneseRoadCensus, normalizeJapaneseRoadIdentity,
  type JapaneseRoadCensus, type JapaneseVehicleCounts } from './lib/roads-jp-source.js'
import { SOURCE_ID_JP_CLASS_MEDIAN_FALLBACK, SOURCE_ID_JP_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { writeRoadAadt, type RoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { writeNationalRoadSquares } from './lib/square-pool.js'

const MEASURED_SOURCE_ID = SOURCE_ID_JP_NATIONAL_ROADS
const FALLBACK_SOURCE_ID = SOURCE_ID_JP_CLASS_MEDIAN_FALLBACK
const JAPAN_BBOX = [24, 122, 46, 146] as const
const COVERED_ROAD_CLASSES: ReadonlySet<number> = new Set([0, 1, 2, 3, 4, 10, 11, 12])

function half(counts: JapaneseVehicleCounts & RoadObservation): JapaneseVehicleCounts & RoadObservation {
  return { ...counts, small: Math.round(counts.small / 2), large: Math.round(counts.large / 2) }
}

function traffic(counts: JapaneseVehicleCounts & RoadObservation, sourceId: number): RoadAadt {
  const medium = Math.round(counts.large * 0.25)
  return { countBasis: counts.countBasis, observationId: counts.observationId, light: counts.small, medium, heavy: counts.large - medium, moto: 0, sourceId }
}

export function buildJapaneseRoadMatcher(census: JapaneseRoadCensus): (row: RoadRow) => RoadAadt | null {
  const expresswayCache = new Map<string, (JapaneseVehicleCounts & RoadObservation) | null>()
  const expressway = (rawName: string): (JapaneseVehicleCounts & RoadObservation) | null => {
    const name = normalizeJapaneseRoadIdentity(rawName)
    if (name.length < 4) return null
    if (expresswayCache.has(name)) return expresswayCache.get(name) ?? null
    let counts = census.expresswayByName.get(name) ?? null
    if (!counts) {
      for (const censusName of census.expresswayNames) {
        if (censusName.length < 4) break
        if (censusName.includes(name) || name.includes(censusName)) {
          counts = census.expresswayByName.get(censusName) ?? null
          break
        }
      }
    }
    expresswayCache.set(name, counts)
    return counts
  }
  const national = (row: RoadRow): (JapaneseVehicleCounts & RoadObservation) | null => {
    if (row.roadClass === 1) {
      for (const token of (row.ref ?? '').split(/[;,/]/)) {
        const ref = leadingJapaneseRoadDigits(token)
        const counts = ref ? census.nationalByRef.get(ref) : undefined
        if (counts) return counts
      }
    }
    const match = row.name ? normalizeJapaneseRoadIdentity(row.name).match(/(?:一般)?国道(\d+)号/) : null
    return match ? census.nationalByRef.get(match[1]) ?? null : null
  }
  return (row: RoadRow): RoadAadt | null => {
    if (!COVERED_ROAD_CLASSES.has(row.roadClass)) return null
    // A census section is counted on the through road; a slip road keeps the class prior.
    if (row.roadClass === 0) {
      const counts = row.name ? expressway(row.name) : null
      if (counts) return traffic(counts, MEASURED_SOURCE_ID)
    } else if (row.roadClass <= 2) {
      const counts = national(row)
      if (counts) return traffic(counts, MEASURED_SOURCE_ID)
    }
    const fallback = census.classMedian.get(row.roadClass)
    return fallback ? traffic(row.roadClass >= 10 ? half(fallback) : fallback, FALLBACK_SOURCE_ID) : null
  }
}

export async function enrichJapaneseRoads(preparedDirectory: string, census: JapaneseRoadCensus) {
  if (census.admittedSections === 0) throw new Error('Japanese census has no usable sections')
  const match = buildJapaneseRoadMatcher(census)
  return writeNationalRoadSquares(preparedDirectory, JAPAN_BBOX, 'Japanese', {}, path =>
    writeRoadAadt(path, match,
      undefined, COVERED_ROAD_CLASSES, { sourceIds: [MEASURED_SOURCE_ID, FALLBACK_SOURCE_ID],
        when: row => match(row)?.sourceId !== row.existingSourceId }))
}

export async function runJapaneseRoadEnrichment(options: RoadLoaderArguments) {
  const census = loadJapaneseRoadCensus(options)
  return { sourceRows: census.sourceRows, admittedSections: census.admittedSections,
    unsupportedTypeRows: census.unsupportedTypeRows,
    unavailableTrafficRows: census.unavailableTrafficRows, invalidRows: census.invalidRows,
    nationalRoutes: census.nationalByRef.size, expresswayNames: census.expresswayByName.size,
    classMedians: census.classMedian.size,
    ...await enrichJapaneseRoads(options.preparedDirectory, census) }
}

runRoadLoaderCli(import.meta.url, runJapaneseRoadEnrichment)
