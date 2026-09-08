/** Enrich z9 Amsterdam roads with the pinned EU-city 2025 AADT source. */

import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { SOURCE_ID_EU_CITY_TRAFFIC } from './lib/source-ids.generated.js'
import { listPreparedSquares, type PreparedBbox } from './lib/prepared-grid.js'
import { shouldOverwrite } from './lib/provenance.js'
import { parseRoadLoaderArguments } from './lib/road-loader-cli.js'
import {
  loadAmsterdamTrafficCensus, type AmsterdamTrafficRecord,
} from './lib/roads-nl-source.js'
import { writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { flatDist } from './lib/spatial.js'

const SOURCE_ID = SOURCE_ID_EU_CITY_TRAFFIC
const GRID_CELL_DEGREES = 0.001
const MAXIMUM_MATCH_DISTANCE_M = 50

export interface NlEnrichmentResult {
  rows: number
  matched: number
  squares: number
  squaresUpdated: number
}

const gridKey = (latitudeCell: number, longitudeCell: number): string =>
  `${latitudeCell},${longitudeCell}`

export function indexAmsterdamTraffic(
  records: readonly AmsterdamTrafficRecord[],
): ReadonlyMap<string, readonly AmsterdamTrafficRecord[]> {
  const grid = new Map<string, AmsterdamTrafficRecord[]>()
  for (const record of records) {
    const key = gridKey(
      Math.floor(record.latitude / GRID_CELL_DEGREES),
      Math.floor(record.longitude / GRID_CELL_DEGREES),
    )
    const bucket = grid.get(key)
    if (bucket) bucket.push(record)
    else grid.set(key, [record])
  }
  return grid
}

/** Match the road midpoint to the nearest treated-source representative point. */
export function matchAmsterdamTrafficRecord(
  row: RoadRow,
  grid: ReadonlyMap<string, readonly AmsterdamTrafficRecord[]>,
): AmsterdamTrafficRecord | null {
  const latitudeCell = Math.floor(row.midLat / GRID_CELL_DEGREES)
  const longitudeCell = Math.floor(row.midLon / GRID_CELL_DEGREES)
  let closest: AmsterdamTrafficRecord | null = null
  let closestDistance = MAXIMUM_MATCH_DISTANCE_M
  for (let latitudeOffset = -1; latitudeOffset <= 1; latitudeOffset++) {
    for (let longitudeOffset = -1; longitudeOffset <= 1; longitudeOffset++) {
      const candidates = grid.get(gridKey(
        latitudeCell + latitudeOffset,
        longitudeCell + longitudeOffset,
      ))
      if (!candidates) continue
      for (const candidate of candidates) {
        const distance = flatDist(
          row.midLat, row.midLon, candidate.latitude, candidate.longitude,
        )
        if (distance < closestDistance) {
          closest = candidate
          closestDistance = distance
        }
      }
    }
  }
  return closest
}

function sourceBbox(records: readonly AmsterdamTrafficRecord[]): PreparedBbox {
  if (records.length === 0) throw new Error('Amsterdam traffic source has no records')
  let south = 90
  let west = 180
  let north = -90
  let east = -180
  for (const record of records) {
    south = Math.min(south, record.latitude)
    west = Math.min(west, record.longitude)
    north = Math.max(north, record.latitude)
    east = Math.max(east, record.longitude)
  }
  // At Amsterdam latitudes, one grid cell includes every midpoint eligible under 50 m.
  return [
    Math.max(-90, south - GRID_CELL_DEGREES),
    Math.max(-180, west - GRID_CELL_DEGREES),
    Math.min(90, north + GRID_CELL_DEGREES),
    Math.min(180, east + GRID_CELL_DEGREES),
  ]
}

export async function enrichNetherlandsRoads(
  preparedDirectory: string,
  records: readonly AmsterdamTrafficRecord[],
): Promise<NlEnrichmentResult> {
  const squares = listPreparedSquares(preparedDirectory, sourceBbox(records))
  if (squares.length === 0) {
    throw new Error(`no Amsterdam roads.arrow squares found under ${preparedDirectory}`)
  }
  const grid = indexAmsterdamTraffic(records)
  const result: NlEnrichmentResult = {
    rows: 0, matched: 0, squares: squares.length, squaresUpdated: 0,
  }
  for (const square of squares) {
    const write = await writeRoadAadt(
      resolve(preparedDirectory, square, 'roads.arrow'),
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, SOURCE_ID)) return null
        const record = matchAmsterdamTrafficRecord(row, grid)
        return record ? {
          light: record.aadt_light,
          medium: record.aadt_medium,
          heavy: record.aadt_heavy,
          moto: record.aadt_moto,
          sourceId: SOURCE_ID,
        } : null
      },
    )
    result.rows += write.rows
    result.matched += write.matched
    if (write.updated) result.squaresUpdated++
  }
  return result
}

async function main(): Promise<void> {
  const options = parseRoadLoaderArguments(process.argv.slice(2), 'enrich-roads-nl.ts')
  const census = await loadAmsterdamTrafficCensus(options)
  const result = await enrichNetherlandsRoads(options.preparedDirectory, census.records)
  const { records, ...source } = census
  console.log(JSON.stringify({ ...source, records: records.length, ...result }))
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : error)
    process.exitCode = 1
  })
}
