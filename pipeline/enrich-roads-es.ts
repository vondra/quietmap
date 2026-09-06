/** Enrich z9 Spanish roads with MITMA 2022 state-road traffic measurements. */

import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { SOURCE_ID_ES_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { listPreparedSquares } from './lib/prepared-grid.js'
import { shouldOverwrite } from './lib/provenance.js'
import { parseRoadLoaderArguments } from './lib/road-loader-cli.js'
import {
  loadMitmaRoadCensus, normalizeSpanishRoadRef, SPAIN_ROAD_SOURCE_BBOX,
  type MitmaRoadSection,
} from './lib/roads-es-source.js'
import { writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { pointToPolylineDist } from './lib/spatial.js'

const SOURCE_ID = SOURCE_ID_ES_NATIONAL_ROADS
const MAXIMUM_MATCH_DISTANCE_M = 30_000

export interface EsEnrichmentResult {
  rows: number
  matched: number
  retracted: number
  skippedForeign: number
  squares: number
  squaresUpdated: number
}

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
): Promise<EsEnrichmentResult> {
  const squares = listPreparedSquares(preparedDirectory, SPAIN_ROAD_SOURCE_BBOX)
  if (squares.length === 0) throw new Error(`no Spanish roads.arrow squares found under ${preparedDirectory}`)
  const sectionsByRef = indexMitmaRoadCensus(sections)
  const match = (row: RoadRow): MitmaRoadSection | null =>
    matchMitmaRoadSection(row, sectionsByRef)
  const result: EsEnrichmentResult = {
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
  const options = parseRoadLoaderArguments(process.argv.slice(2), 'enrich-roads-es.ts')
  const census = await loadMitmaRoadCensus(options)
  const result = await enrichSpanishRoads(options.preparedDirectory, census.sections)
  const { sections, ...source } = census
  console.log(JSON.stringify({ ...source, sections: sections.length, ...result }))
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : error)
    process.exitCode = 1
  })
}
