/** Enrich z9 metropolitan French roads with corrected Cerema RRN TMJA data. */

import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { SOURCE_ID_FR_CEREMA_TMJA } from './lib/source-ids.generated.js'
import { listPreparedSquares } from './lib/prepared-grid.js'
import { shouldOverwrite } from './lib/provenance.js'
import { parseRoadLoaderArguments } from './lib/road-loader-cli.js'
import {
  loadCeremaCensus, type CeremaCensusSection,
} from './lib/roads-fr-source.js'
import { writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { pointToPolylineDist } from './lib/spatial.js'

const SOURCE_ID = SOURCE_ID_FR_CEREMA_TMJA
const FRANCE_BBOX = [41, -5.5, 51.5, 10] as const
const MAXIMUM_MATCH_DISTANCE_M = 20_000

export interface FrEnrichmentResult {
  rows: number
  matched: number
  retracted: number
  skippedForeign: number
  squares: number
  squaresUpdated: number
}

export function indexCeremaCensus(
  sections: readonly CeremaCensusSection[],
): ReadonlyMap<string, readonly CeremaCensusSection[]> {
  const byRef = new Map<string, CeremaCensusSection[]>()
  for (const section of sections) {
    const candidates = byRef.get(section.ref)
    if (candidates) candidates.push(section)
    else byRef.set(section.ref, [section])
  }
  return byRef
}

/** Match the OSM midpoint to the closest published section line with the same ref. */
export function matchCeremaSection(
  row: RoadRow,
  sectionsByRef: ReadonlyMap<string, readonly CeremaCensusSection[]>,
): CeremaCensusSection | null {
  const normalizedRef = row.ref?.replace(/\s+/g, '') ?? ''
  const candidates = normalizedRef ? sectionsByRef.get(normalizedRef) : undefined
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
): Promise<FrEnrichmentResult> {
  const squares = listPreparedSquares(preparedDirectory, FRANCE_BBOX)
  if (squares.length === 0) throw new Error(`no French roads.arrow squares found under ${preparedDirectory}`)
  const sectionsByRef = indexCeremaCensus(sections)
  const match = (row: RoadRow): CeremaCensusSection | null =>
    matchCeremaSection(row, sectionsByRef)
  const result: FrEnrichmentResult = {
    rows: 0, matched: 0, retracted: 0, skippedForeign: 0,
    squares: squares.length, squaresUpdated: 0,
  }
  for (const square of squares) {
    const write = await writeRoadAadt(
      resolve(preparedDirectory, square, 'roads.arrow'),
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, SOURCE_ID)) return null
        const section = match(row)
        return section ? {
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
    )
    result.rows += write.rows
    result.matched += write.matched
    result.retracted += write.retracted
    result.skippedForeign += write.skippedForeign
    if (write.updated) result.squaresUpdated++
  }
  return result
}

async function main(): Promise<void> {
  const options = parseRoadLoaderArguments(process.argv.slice(2), 'enrich-roads-fr.ts')
  const census = await loadCeremaCensus(options)
  const result = await enrichFrenchRoads(options.preparedDirectory, census.sections)
  console.log(JSON.stringify({
    sections: census.sections.length,
    sourceRows: census.files.reduce((sum, file) => sum + file.sourceRows, 0),
    files: census.files,
    ...result,
  }))
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : error)
    process.exitCode = 1
  })
}
