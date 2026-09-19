/** Enrich z9 Spanish roads with MITMA 2022 state-road traffic measurements. */

import { roadObservation } from './lib/road-observation.js'
import { SOURCE_ID_ES_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { shouldOverwrite } from './lib/provenance.js'
import { runRoadLoaderCli, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import {
  loadMitmaRoadCensus, normalizeSpanishRoadRef, SPAIN_ROAD_SOURCE_BBOX,
  type MitmaRoadSection,
} from './lib/roads-es-source.js'
import { isSlipRoadClass, writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { writeNationalRoadSquares } from './lib/square-pool.js'
import { pointToPolylineDist } from './lib/spatial.js'

const SOURCE_ID = SOURCE_ID_ES_NATIONAL_ROADS
const MAXIMUM_MATCH_DISTANCE_M = 30_000

export function indexMitmaRoadCensus(
  sections: readonly MitmaRoadSection[],
): ReadonlyMap<string, readonly MitmaRoadSection[]> {
  const byRef = new Map<string, MitmaRoadSection[]>()
  for (const section of sections) {
    const candidates = byRef.get(section.ref)
    if (candidates) candidates.push(section)
    else byRef.set(section.ref, [section])
  }
  return byRef
}

/** Match the OSM midpoint to the closest real MITMA line with the same first ref. */
export function matchMitmaRoadSection(
  row: RoadRow,
  sectionsByRef: ReadonlyMap<string, readonly MitmaRoadSection[]>,
): MitmaRoadSection | null {
  if (isSlipRoadClass(row.roadClass)) return null
  const normalizedRef = normalizeSpanishRoadRef(row.ref ?? '')
  const candidates = normalizedRef ? sectionsByRef.get(normalizedRef) : undefined
  if (!candidates) return null
  let closest: MitmaRoadSection | null = null
  let closestDistance = MAXIMUM_MATCH_DISTANCE_M
  for (const section of candidates) {
    for (const line of section.lines) {
      const distance = pointToPolylineDist(row.midLat, row.midLon, line)
      if (distance < closestDistance) {
        closest = section
        closestDistance = distance
      }
    }
  }
  return closest
}

export async function enrichSpanishRoads(
  preparedDirectory: string,
  sections: readonly MitmaRoadSection[],
) {
  const sectionsByRef = indexMitmaRoadCensus(sections)
  const match = (row: RoadRow): MitmaRoadSection | null =>
    matchMitmaRoadSection(row, sectionsByRef)
  return writeNationalRoadSquares(preparedDirectory, SPAIN_ROAD_SOURCE_BBOX, 'Spanish', {}, path =>
    writeRoadAadt(
      path,
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, SOURCE_ID)) return null
        const section = match(row)
        return section ? { ...roadObservation(section.featureId, 'both-directions'),
          light: section.aadt_light,
          medium: section.aadt_medium,
          heavy: section.aadt_heavy,
          moto: section.aadt_moto,
          sourceId: SOURCE_ID,
        } : null
      },
      undefined,
      undefined,
      { sourceIds: [SOURCE_ID], when: row => match(row) === null },
    ))
}

async function main(options: RoadLoaderArguments) {
  const census = await loadMitmaRoadCensus(options)
  const result = await enrichSpanishRoads(options.preparedDirectory, census.sections)
  const { sections, ...source } = census
  return { ...source, sections: sections.length, ...result }
}

runRoadLoaderCli(import.meta.url, main)
