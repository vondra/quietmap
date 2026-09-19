/** Enrich z9 metropolitan French roads with corrected Cerema RRN TMJA data. */

import { SOURCE_ID_FR_CEREMA_TMJA } from './lib/source-ids.generated.js'
import { shouldOverwrite } from './lib/provenance.js'
import { runRoadLoaderCli, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import {
  loadCeremaCensus, type CeremaCensusSection,
} from './lib/roads-fr-source.js'
import { isSlipRoadClass, writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { writeNationalRoadSquares } from './lib/square-pool.js'
import { pointToPolylineDist } from './lib/spatial.js'

const SOURCE_ID = SOURCE_ID_FR_CEREMA_TMJA
const FRANCE_BBOX = [41, -5.5, 51.5, 10] as const
const MAXIMUM_MATCH_DISTANCE_M = 20_000

/** OSM writes "A 5a" where Cerema publishes A0005A; both sides meet as "A5A". */
const comparableRef = (ref: string): string => ref.replace(/\s+/g, '').toUpperCase()

export function indexCeremaCensus(
  sections: readonly CeremaCensusSection[],
): ReadonlyMap<string, readonly CeremaCensusSection[]> {
  const byRef = new Map<string, CeremaCensusSection[]>()
  for (const section of sections) {
    const ref = comparableRef(section.ref)
    const candidates = byRef.get(ref)
    if (candidates) candidates.push(section)
    else byRef.set(ref, [section])
  }
  return byRef
}

/** Match the OSM midpoint to the closest published section line with the same ref. */
export function matchCeremaSection(
  row: RoadRow,
  sectionsByRef: ReadonlyMap<string, readonly CeremaCensusSection[]>,
): CeremaCensusSection | null {
  if (isSlipRoadClass(row.roadClass)) return null
  const candidates = row.ref ? sectionsByRef.get(comparableRef(row.ref)) : undefined
  if (!candidates) return null
  let closest: CeremaCensusSection | null = null
  let closestDistance = MAXIMUM_MATCH_DISTANCE_M
  for (const section of candidates) {
    const distance = pointToPolylineDist(row.midLat, row.midLon, section.coords)
    if (distance < closestDistance) {
      closest = section
      closestDistance = distance
    }
  }
  return closest
}

export async function enrichFrenchRoads(
  preparedDirectory: string,
  sections: readonly CeremaCensusSection[],
) {
  const sectionsByRef = indexCeremaCensus(sections)
  const match = (row: RoadRow): CeremaCensusSection | null =>
    matchCeremaSection(row, sectionsByRef)
  return writeNationalRoadSquares(preparedDirectory, FRANCE_BBOX, 'French', {}, path =>
    writeRoadAadt(
      path,
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, SOURCE_ID)) return null
        const section = match(row)
        return section ? { countBasis: section.countBasis, observationId: section.observationId,
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
  const census = await loadCeremaCensus(options)
  const result = await enrichFrenchRoads(options.preparedDirectory, census.sections)
  return {
    sections: census.sections.length,
    sourceRows: census.files.reduce((sum, file) => sum + file.sourceRows, 0),
    files: census.files,
    ...result,
  }
}

runRoadLoaderCli(import.meta.url, main)
