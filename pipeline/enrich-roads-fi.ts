/** Enrich z9 road vectors with Finnish Väylävirasto 2024 KVL measurements. */

import { existsSync, readFileSync, statSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { writeCacheAtomically } from './lib/atomic-cache.js'
import { SOURCE_ID_FI_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { listPreparedSquares } from './lib/prepared-grid.js'
import { shouldOverwrite } from './lib/provenance.js'
import { parseRoadLoaderArguments, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import {
  ROAD_CLASS_RANK_TOLERANCE, disjointVehicleClassCountsFitPublishedTotal,
  osmRoadClassRank, writeRoadAadt, type RoadRow,
} from './lib/roads-arrow.js'
import {
  buildOneHundredthDegreePointGrid, nearestCompatiblePointWithin200Metres,
  type RankedPoint,
} from './lib/spatial.js'

const SOURCE_ID = SOURCE_ID_FI_NATIONAL_ROADS
const COVERED_ROAD_CLASSES: ReadonlySet<number> = new Set([0, 1, 2, 3, 4, 10, 11, 12])
const FINLAND_BBOX = [59.7, 19.1, 70.1, 31.6] as const
const CACHE_DIRECTORY = 'fi'
const PAGE_SIZE = 1000
const PAGE_OFFSETS = [...Array(19).keys()].map(index => index * PAGE_SIZE)
const WFS_BASE = 'https://avoinapi.vaylapilvi.fi/vaylatiedot/wfs'

export interface FiRoadSegment extends RankedPoint {
  roadNumber: number
  aadt: number
  light: number
  medium: number
  heavy: number
  moto: number
}

export interface FiEnrichmentResult {
  rows: number
  matched: number
  retracted: number
  skippedForeign: number
  squares: number
  squaresUpdated: number
}

export interface ParsedFiPages {
  segments: FiRoadSegment[]
  inconsistentClassTotalsSkipped: number
}

function nonNegativeInteger(value: unknown, name: string): number {
  if (value !== null && value !== undefined &&
      typeof value !== 'number' && typeof value !== 'string') {
    throw new Error(`invalid Finnish KVL integer '${name}': ${JSON.stringify(value)}`)
  }
  const text = typeof value === 'string' ? value.trim() : value
  const parsed = text === '' || text === null || text === undefined ? 0 : Number(text)
  if (!Number.isSafeInteger(parsed) || parsed < 0) {
    throw new Error(`invalid Finnish KVL integer '${name}': ${JSON.stringify(value)}`)
  }
  return parsed
}

function geometryCentroid(value: unknown): readonly [number, number] | null {
  if (!value || typeof value !== 'object') return null
  const geometry = value as { type?: unknown; coordinates?: unknown }
  let lines: unknown[]
  if (geometry.type === 'LineString' && Array.isArray(geometry.coordinates)) {
    lines = [geometry.coordinates]
  } else if (geometry.type === 'MultiLineString' && Array.isArray(geometry.coordinates)) {
    lines = geometry.coordinates
  } else {
    return null
  }
  let latitudeTotal = 0
  let longitudeTotal = 0
  let count = 0
  for (const line of lines) {
    if (!Array.isArray(line)) continue
    for (const coordinate of line) {
      if (!Array.isArray(coordinate) || coordinate.length < 2 ||
          typeof coordinate[0] !== 'number' || typeof coordinate[1] !== 'number' ||
          !Number.isFinite(coordinate[0]) || !Number.isFinite(coordinate[1]) ||
          coordinate[0] < -180 || coordinate[0] > 180 ||
          coordinate[1] < -90 || coordinate[1] > 90) {
        throw new Error(`invalid Finnish KVL coordinate: ${JSON.stringify(coordinate)}`)
      }
      longitudeTotal += coordinate[0]
      latitudeTotal += coordinate[1]
      count++
    }
  }
  return count === 0 ? null : [latitudeTotal / count, longitudeTotal / count]
}

/** Finnish national numbering provides the functional class absent from the feed. */
export function fiRoadNumberRank(roadNumber: number): number {
  if (!Number.isSafeInteger(roadNumber) || roadNumber <= 0) {
    throw new Error(`invalid Finnish road number: ${roadNumber}`)
  }
  if (roadNumber === 101 || roadNumber === 102 || roadNumber < 100) return 0
  return roadNumber < 1000 ? 2 : 4
}

/** Parse complete WFS pages using the current dev1 CNOSSOS split and class rules. */
export function parseFiPages(pages: readonly unknown[]): ParsedFiPages {
  const segments: FiRoadSegment[] = []
  let inconsistentClassTotalsSkipped = 0
  for (const page of pages) {
    if (!page || typeof page !== 'object' ||
        !Array.isArray((page as { features?: unknown }).features)) {
      throw new Error('Finnish KVL page has no features array')
    }
    for (const feature of (page as { features: unknown[] }).features) {
      if (!feature || typeof feature !== 'object') continue
      const properties = (feature as { properties?: unknown }).properties
      if (!properties || typeof properties !== 'object') continue
      const values = properties as Record<string, unknown>
      const aadt = nonNegativeInteger(values.kvl, 'kvl')
      if (aadt === 0) continue
      const roadNumber = nonNegativeInteger(values.alkusijainti_tie, 'alkusijainti_tie')
      if (roadNumber === 0) continue
      const centroid = geometryCentroid((feature as { geometry?: unknown }).geometry)
      if (!centroid) continue
      const [latitude, longitude] = centroid
      if (latitude < FINLAND_BBOX[0] || latitude > FINLAND_BBOX[2] ||
          longitude < FINLAND_BBOX[1] || longitude > FINLAND_BBOX[3]) continue
      const heavy = nonNegativeInteger(values.kvl_raskas, 'kvl_raskas')
      const moto = Math.round(aadt * 0.01)
      const light = Math.max(0, aadt - heavy - moto)
      // Archived page 17000 has exactly two impossible measurements: internal_id
      // 17352 (road 21819, 109 total/520 heavy) and 17659 (21851, 159/247).
      // Drop rather than clamp them so no invented traffic value reaches a road.
      if (!disjointVehicleClassCountsFitPublishedTotal(
        aadt, [light, 0, heavy, moto], 'exact',
      )) {
        inconsistentClassTotalsSkipped++
        continue
      }
      segments.push({
        roadNumber,
        latitude,
        longitude,
        rank: fiRoadNumberRank(roadNumber),
        aadt,
        light,
        medium: 0,
        heavy,
        moto,
      })
    }
  }
  return { segments, inconsistentClassTotalsSkipped }
}

function pagePath(enrichmentDirectory: string, offset: number): string {
  return resolve(enrichmentDirectory, CACHE_DIRECTORY, `liikennemaarat-page-${offset}.json`)
}

async function loadFiSegments(options: RoadLoaderArguments): Promise<ParsedFiPages> {
  const pages: unknown[] = []
  for (const offset of PAGE_OFFSETS) {
    const path = pagePath(options.enrichmentDirectory, offset)
    const cached = existsSync(path) && statSync(path).size > 1000
    if (options.forceDownload || !cached) {
      if (options.enrichOnly) throw new Error(`Finnish KVL cache missing or empty: ${path}`)
      const query = new URLSearchParams({
        service: 'WFS', version: '2.0.0', request: 'GetFeature',
        typeNames: 'tiestotiedot:liikennemaarat_2024', outputFormat: 'application/json',
        srsName: 'EPSG:4326', count: String(PAGE_SIZE), startIndex: String(offset),
        sortBy: 'internal_id',
      })
      const response = await fetch(`${WFS_BASE}?${query}`, { signal: AbortSignal.timeout(120_000) })
      if (!response.ok) throw new Error(`Finnish KVL download returned HTTP ${response.status} at offset ${offset}`)
      writeCacheAtomically(path, Buffer.from(await response.arrayBuffer()))
    }
    pages.push(JSON.parse(readFileSync(path, 'utf8')) as unknown)
  }
  return parseFiPages(pages)
}

export async function enrichFinnishRoads(
  preparedDirectory: string,
  segments: readonly FiRoadSegment[],
): Promise<FiEnrichmentResult> {
  const squares = listPreparedSquares(preparedDirectory, FINLAND_BBOX)
  if (squares.length === 0) throw new Error(`no Finnish roads.arrow squares found under ${preparedDirectory}`)
  const grid = buildOneHundredthDegreePointGrid(segments)
  const match = (row: RoadRow): FiRoadSegment | null => nearestCompatiblePointWithin200Metres(
    row.midLat, row.midLon, osmRoadClassRank(row.roadClass), ROAD_CLASS_RANK_TOLERANCE, grid,
  )
  const result: FiEnrichmentResult = {
    rows: 0, matched: 0, retracted: 0, skippedForeign: 0,
    squares: squares.length, squaresUpdated: 0,
  }
  for (const square of squares) {
    const write = await writeRoadAadt(
      resolve(preparedDirectory, square, 'roads.arrow'),
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, SOURCE_ID)) return null
        const segment = match(row)
        return segment ? {
          light: segment.light, medium: segment.medium, heavy: segment.heavy,
          moto: segment.moto, sourceId: SOURCE_ID,
        } : null
      },
      undefined,
      COVERED_ROAD_CLASSES,
      { sourceId: SOURCE_ID, when: row => match(row) === null },
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
  const options = parseRoadLoaderArguments(process.argv.slice(2), 'enrich-roads-fi.ts')
  const parsed = await loadFiSegments(options)
  const result = await enrichFinnishRoads(options.preparedDirectory, parsed.segments)
  console.log(JSON.stringify({
    segments: parsed.segments.length,
    inconsistentClassTotalsSkipped: parsed.inconsistentClassTotalsSkipped,
    ...result,
  }))
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : error)
    process.exitCode = 1
  })
}
