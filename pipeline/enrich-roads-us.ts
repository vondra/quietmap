/** Enrich z9 US roads with class-compatible FHWA HPMS 2022 traffic measurements. */

import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { writeCacheAtomically } from './lib/atomic-cache.js'
import { DATASETS } from './lib/enrichment-datasets.js'
import { listPreparedSquares } from './lib/prepared-grid.js'
import { parseRoadLoaderArguments, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { osmRoadClassRank, ROAD_CLASS_RANK_TOLERANCE, writeRoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { SOURCE_ID_US_FHWA_HPMS, shouldOverwrite } from './lib/sources.js'
import {
  buildOneHundredthDegreePointGrid, nearestCompatiblePointWithin200Metres, type RankedPoint,
} from './lib/spatial.js'

const SOURCE_ID = SOURCE_ID_US_FHWA_HPMS
const coverage = DATASETS.find(dataset => dataset.id === SOURCE_ID)?.roadCoverage
if (!coverage) throw new Error('FHWA source has no registered road coverage')
const COVERED_ROAD_CLASSES = new Set(coverage)
const US_BBOX = [17.5, -180, 71.5, -65] as const
const PAGE_SIZE = 2000
const HPMS_BASE = 'https://services.arcgis.com/xOi1kZaI0eWDREZv/ArcGIS/rest/services/HPMS_FULL_US_2022_Sysnomulti_view/FeatureServer/0'
// Preserve dev1's class split: the national view publishes total AADT, not classes.
const HEAVY_SHARES = [0.12, 0.10, 0.08, 0.06, 0.05] as const

export interface UsRoadSegment extends RankedPoint {
  aadt: number
  light: number
  medium: number
  heavy: number
  moto: number
}

function sourceNumber(value: unknown, name: string): number {
  if ((typeof value !== 'number' && typeof value !== 'string') ||
      (typeof value === 'string' && value.trim() === '') ||
      !Number.isFinite(Number(value)) || Number(value) < 0) {
    throw new Error(`invalid FHWA ${name}: ${JSON.stringify(value)}`)
  }
  return Number(value)
}

function geometryCentroid(value: unknown): readonly [number, number] {
  if (!value || typeof value !== 'object') throw new Error('FHWA geometry is missing')
  const geometry = value as { type?: unknown; coordinates?: unknown }
  const lines = geometry.type === 'LineString' ? [geometry.coordinates]
    : geometry.type === 'MultiLineString' ? geometry.coordinates : null
  if (!Array.isArray(lines)) throw new Error('FHWA geometry must contain line coordinates')
  let latitude = 0
  let longitude = 0
  let count = 0
  for (const line of lines) {
    if (!Array.isArray(line)) throw new Error('invalid FHWA line coordinates')
    for (const coordinate of line) {
      if (!Array.isArray(coordinate) || coordinate.length < 2 ||
          typeof coordinate[0] !== 'number' || typeof coordinate[1] !== 'number' ||
          !Number.isFinite(coordinate[0]) || !Number.isFinite(coordinate[1]) ||
          coordinate[0] < -180 || coordinate[0] > 180 ||
          coordinate[1] < -90 || coordinate[1] > 90) {
        throw new Error(`invalid FHWA coordinate: ${JSON.stringify(coordinate)}`)
      }
      longitude += coordinate[0]
      latitude += coordinate[1]
      count++
    }
  }
  if (count === 0) throw new Error('FHWA line geometry is empty')
  return [latitude / count, longitude / count]
}

export function parseUsPage(page: unknown): { segments: UsRoadSegment[]; features: number } {
  if (!page || typeof page !== 'object' || 'error' in page ||
      !Array.isArray((page as { features?: unknown }).features)) {
    throw new Error('FHWA page has no valid features array')
  }
  const features = (page as { features: unknown[] }).features
  if (features.length > PAGE_SIZE) throw new Error('FHWA page exceeds requested record count')
  const segments: UsRoadSegment[] = []
  for (const feature of features) {
    if (!feature || typeof feature !== 'object') throw new Error('invalid FHWA feature')
    const properties = (feature as { properties?: unknown }).properties
    if (!properties || typeof properties !== 'object') throw new Error('FHWA properties are missing')
    const values = properties as Record<string, unknown>
    // Fourteen archived records have genuine .5 totals; dev1 truncates the
    // positive total BEFORE splitting classes. Do not reject or round them.
    const aadt = Math.trunc(sourceNumber(values.AADT, 'AADT'))
    if (!Number.isSafeInteger(aadt) || aadt > 2_147_483_647) throw new Error('FHWA AADT exceeds Int32')
    if (aadt === 0) continue
    const functionalClass = sourceNumber(values.F_SYSTEM, 'F_SYSTEM')
    if (!Number.isInteger(functionalClass)) throw new Error('FHWA F_SYSTEM must be an integer')
    const [latitude, longitude] = geometryCentroid((feature as { geometry?: unknown }).geometry)
    if (latitude < US_BBOX[0] || latitude > US_BBOX[2] ||
        longitude < US_BBOX[1] || longitude > US_BBOX[3] ||
        functionalClass < 1 || functionalClass > HEAVY_SHARES.length) continue
    const rank = functionalClass - 1
    const moto = Math.round(aadt * 0.01)
    const totalHeavy = Math.round(aadt * HEAVY_SHARES[rank])
    const medium = Math.round(totalHeavy * 0.20)
    segments.push({
      latitude, longitude, rank, aadt,
      light: aadt - totalHeavy - moto, medium, heavy: totalHeavy - medium, moto,
    })
  }
  return { segments, features: features.length }
}

/** Read the contiguous snapshot through its explicit empty terminal page. */
export async function loadUsSegments(options: RoadLoaderArguments): Promise<UsRoadSegment[]> {
  const directory = resolve(options.enrichmentDirectory, 'us')
  const segments: UsRoadSegment[] = []
  const pendingDownloads: Array<readonly [string, Buffer]> = []
  let previousPageWasPartial = false
  for (let offset = 0; ; offset += PAGE_SIZE) {
    const path = resolve(directory, `hpms-page-${offset}.json`)
    let bytes: Buffer
    const download = options.forceDownload || !existsSync(path)
    if (download) {
      if (options.enrichOnly) throw new Error(`FHWA cache page missing: ${path}`)
      const query = new URLSearchParams({
        where: 'AADT>0', outFields: 'AADT,F_SYSTEM',
        f: 'geojson', outSR: '4326', resultOffset: String(offset),
        resultRecordCount: String(PAGE_SIZE), orderByFields: 'OBJECTID',
      })
      const response = await fetch(`${HPMS_BASE}/query?${query}`, { signal: AbortSignal.timeout(120_000) })
      if (!response.ok) throw new Error(`FHWA HTTP ${response.status} at offset ${offset}`)
      bytes = Buffer.from(await response.arrayBuffer())
    } else {
      bytes = readFileSync(path)
    }
    const parsed = parseUsPage(JSON.parse(bytes.toString('utf8')) as unknown)
    if (previousPageWasPartial && parsed.features !== 0) {
      throw new Error(`FHWA nonempty page follows a short page at offset ${offset}`)
    }
    if (download) pendingDownloads.push([path, bytes])
    if (parsed.features === 0) {
      const trailing = existsSync(directory) ? readdirSync(directory).filter(name => {
        const match = /^hpms-page-(\d+)\.json$/.exec(name)
        return match && Number(match[1]) > offset
      }) : []
      if (trailing.length) throw new Error(`FHWA cache continues beyond terminal page: ${trailing.join(', ')}`)
      if (segments.length === 0) throw new Error('FHWA snapshot has no usable traffic measurements')
      for (const [pendingPath, pendingBytes] of pendingDownloads) {
        writeCacheAtomically(pendingPath, pendingBytes)
      }
      return segments
    }
    segments.push(...parsed.segments)
    previousPageWasPartial = parsed.features < PAGE_SIZE
  }
}

export async function enrichUsRoads(preparedDirectory: string, segments: readonly UsRoadSegment[]) {
  if (segments.length === 0) throw new Error('FHWA snapshot has no usable traffic measurements')
  const squares = listPreparedSquares(preparedDirectory, US_BBOX)
  if (squares.length === 0) throw new Error(`no US roads.arrow squares found under ${preparedDirectory}`)
  const grid = buildOneHundredthDegreePointGrid(segments)
  const match = (row: RoadRow) => {
    if (!shouldOverwrite(row.existingSourceId, SOURCE_ID)) return null
    const segment = nearestCompatiblePointWithin200Metres(
      row.midLat, row.midLon, osmRoadClassRank(row.roadClass), ROAD_CLASS_RANK_TOLERANCE, grid,
    )
    return segment ? {
      light: segment.light, medium: segment.medium, heavy: segment.heavy,
      moto: segment.moto, sourceId: SOURCE_ID,
    } : null
  }
  const result = { rows: 0, matched: 0, retracted: 0, skipped: 0, skippedForeign: 0,
    squares: squares.length, squaresUpdated: 0 }
  for (const square of squares) {
    const write = await writeRoadAadt(resolve(preparedDirectory, square, 'roads.arrow'),
      match, undefined, COVERED_ROAD_CLASSES,
      { sourceIds: [SOURCE_ID], when: row => !COVERED_ROAD_CLASSES.has(row.roadClass) || match(row) === null })
    result.rows += write.rows
    result.matched += write.matched
    result.retracted += write.retracted
    result.skipped += write.skipped
    result.skippedForeign += write.skippedForeign
    if (write.updated) result.squaresUpdated++
  }
  return result
}

export async function runUsEnrichment(options: RoadLoaderArguments) {
  const segments = await loadUsSegments(options)
  return { segments: segments.length, ...await enrichUsRoads(options.preparedDirectory, segments) }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const options = parseRoadLoaderArguments(process.argv.slice(2), 'enrich-roads-us.ts')
  runUsEnrichment(options).then(result => console.log(JSON.stringify(result))).catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : error)
    process.exitCode = 1
  })
}
