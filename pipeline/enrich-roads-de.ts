/** Enrich z9 German roads with BASt SVZ 2021 measured vehicle classes. */

import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import {
  SOURCE_ID_DE_BAST_AUTOBAHN, SOURCE_ID_DE_BAST_BUNDESSTRASSEN,
} from './lib/source-ids.generated.js'
import { listPreparedSquares } from './lib/prepared-grid.js'
import { parseRoadLoaderArguments } from './lib/road-loader-cli.js'
import {
  loadBastCensus, type BastCensusSection,
} from './lib/roads-de-source.js'
import { writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { haversineM } from './lib/spatial.js'

const GERMANY_BBOX = [46, 4, 56, 16] as const
const FALLBACK_DISTANCE_M = 2_000
const REF_DISTANCE_M = 15_000
const GRID_SCALE = 100
const GRID_SEARCH_RADIUS = 4

export interface BastCensusIndex {
  byRef: ReadonlyMap<string, readonly BastCensusSection[]>
  grid: ReadonlyMap<string, readonly BastCensusSection[]>
}

export interface DeEnrichmentResult {
  rows: number
  matched: number
  retracted: number
  matchedAutobahn: number
  matchedBundesstrasse: number
  skippedForeign: number
  squares: number
  squaresUpdated: number
}

const gridKey = (lat: number, lon: number): string =>
  `${Math.floor(lat * GRID_SCALE)},${Math.floor(lon * GRID_SCALE)}`

export function indexBastCensus(sections: readonly BastCensusSection[]): BastCensusIndex {
  const byRef = new Map<string, BastCensusSection[]>()
  const grid = new Map<string, BastCensusSection[]>()
  for (const section of sections) {
    const refs = byRef.get(section.ref)
    if (refs) refs.push(section)
    else byRef.set(section.ref, [section])
    const key = gridKey(section.lat, section.lon)
    const nearby = grid.get(key)
    if (nearby) nearby.push(section)
    else grid.set(key, [section])
  }
  return { byRef, grid }
}

function nearest(
  row: RoadRow,
  candidates: readonly BastCensusSection[],
  maximumDistance: number,
  accepts: (section: BastCensusSection) => boolean = () => true,
): BastCensusSection | null {
  let closest: BastCensusSection | null = null
  let closestDistance = maximumDistance
  for (const section of candidates) {
    if (!accepts(section)) continue
    const distance = haversineM(row.midLat, row.midLon, section.lat, section.lon)
    if (distance < closestDistance) {
      closest = section
      closestDistance = distance
    }
  }
  return closest
}

function nearbySections(
  grid: ReadonlyMap<string, readonly BastCensusSection[]>, lat: number, lon: number,
): BastCensusSection[] {
  const gy = Math.floor(lat * GRID_SCALE)
  const gx = Math.floor(lon * GRID_SCALE)
  const found: BastCensusSection[] = []
  // Four 0.01° longitude cells cover 2 km at Germany's northern edge.
  for (let dy = -GRID_SEARCH_RADIUS; dy <= GRID_SEARCH_RADIUS; dy++) {
    for (let dx = -GRID_SEARCH_RADIUS; dx <= GRID_SEARCH_RADIUS; dx++) {
      const bucket = grid.get(`${gy + dy},${gx + dx}`)
      if (bucket) found.push(...bucket)
    }
  }
  return found
}

/** Dev1's exact-ref match, then its class-compatible two-kilometre fallback. */
export function matchBastSection(row: RoadRow, census: BastCensusIndex): BastCensusSection | null {
  const normalizedRef = row.ref?.replace(/\s+/g, '') ?? ''
  const byRef = normalizedRef ? census.byRef.get(normalizedRef) : undefined
  if (byRef) return nearest(row, byRef, REF_DISTANCE_M)
  if ((row.roadClass !== 0 && row.roadClass !== 1) ||
      row.midLat <= 47 || row.midLat >= 55.5) return null
  return nearest(
    row,
    nearbySections(census.grid, row.midLat, row.midLon),
    FALLBACK_DISTANCE_M,
    section => row.roadClass === 0 ? section.ref.startsWith('A') : section.ref.startsWith('B'),
  )
}

function sourceId(section: BastCensusSection): number {
  return section.ref.startsWith('A')
    ? SOURCE_ID_DE_BAST_AUTOBAHN
    : SOURCE_ID_DE_BAST_BUNDESSTRASSEN
}

export async function enrichGermanRoads(
  preparedDirectory: string,
  sections: readonly BastCensusSection[],
): Promise<DeEnrichmentResult> {
  const squares = listPreparedSquares(preparedDirectory, GERMANY_BBOX)
  if (squares.length === 0) throw new Error(`no German roads.arrow squares found under ${preparedDirectory}`)
  const census = indexBastCensus(sections)
  const result: DeEnrichmentResult = {
    rows: 0, matched: 0, retracted: 0, matchedAutobahn: 0, matchedBundesstrasse: 0,
    skippedForeign: 0, squares: squares.length, squaresUpdated: 0,
  }
  for (const square of squares) {
    const write = await writeRoadAadt(
      resolve(preparedDirectory, square, 'roads.arrow'),
      (row) => {
        const section = matchBastSection(row, census)
        if (!section) return null
        const pickedSourceId = sourceId(section)
        return {
          light: section.aadt_light,
          medium: section.aadt_medium,
          heavy: section.aadt_heavy,
          moto: section.aadt_moto,
          sourceId: pickedSourceId,
        }
      },
      (_row, _index, applied) => {
        if (applied.sourceId === SOURCE_ID_DE_BAST_AUTOBAHN) result.matchedAutobahn++
        else result.matchedBundesstrasse++
      },
      undefined,
      { sourceIds: [SOURCE_ID_DE_BAST_AUTOBAHN, SOURCE_ID_DE_BAST_BUNDESSTRASSEN],
        when: row => {
          const section = matchBastSection(row, census)
          return section === null || sourceId(section) !== row.existingSourceId
        } },
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
  const options = parseRoadLoaderArguments(process.argv.slice(2), 'enrich-roads-de.ts')
  const census = await loadBastCensus(options)
  const result = await enrichGermanRoads(options.preparedDirectory, census.sections)
  console.log(JSON.stringify({
    sourceRows: census.sourceRows,
    sections: census.sections.length,
    invalidRowsSkipped: census.invalidRowsSkipped,
    zeroClassSplitsSkipped: census.zeroClassSplitsSkipped,
    inconsistentClassTotalsSkipped: census.inconsistentClassTotalsSkipped,
    ...result,
  }))
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : error)
    process.exitCode = 1
  })
}
