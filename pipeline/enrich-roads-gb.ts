/** Enrich z9 road vectors with Great Britain DfT AADF count points. */

import { existsSync, mkdirSync, readFileSync } from 'node:fs'
import { execFileSync } from 'node:child_process'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { parse } from 'csv-parse/sync'
import { writeCacheAtomically } from './lib/atomic-cache.js'
import { SOURCE_ID_GB_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { listPreparedSquares } from './lib/prepared-grid.js'
import { shouldOverwrite } from './lib/provenance.js'
import { parseRoadLoaderArguments, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import {
  disjointVehicleClassCountsFitPublishedTotal, writeRoadAadt, type RoadRow,
} from './lib/roads-arrow.js'
import { haversineM } from './lib/spatial.js'

const SOURCE_ID = SOURCE_ID_GB_NATIONAL_ROADS
const GREAT_BRITAIN_BBOX = [49, -8.5, 61, 2.5] as const
const CACHE_DIRECTORY = 'gb'
const CACHE_JSON = 'dft-aadf.json'
const CACHE_ZIP = 'dft-aadf.zip'
const EXTRACTED_CSV = 'dft_traffic_counts_aadf.csv'
const DFT_URL = 'https://storage.googleapis.com/dft-statistics/road-traffic/downloads/data-gov-uk/dft_traffic_counts_aadf.zip'

export interface DftCountPoint {
  ref: string
  latitude: number
  longitude: number
  roadCategory: string
  light: number
  medium: number
  heavy: number
  moto: number
  total: number
  year: number
}

export interface GbEnrichmentResult {
  rows: number
  matched: number
  retracted: number
  skippedForeign: number
  squares: number
  squaresUpdated: number
}

type CsvRow = Record<string, string>

function nonNegativeInteger(row: CsvRow, name: string): number {
  const raw = row[name]?.trim() ?? ''
  if (raw === '') return 0
  const value = Number(raw)
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new Error(`invalid DfT integer '${name}': ${JSON.stringify(raw)}`)
  }
  return value
}

function countPoint(row: CsvRow): DftCountPoint | null {
  const id = row.count_point_id?.trim()
  const year = Number(row.year)
  const latitude = Number(row.latitude)
  const longitude = Number(row.longitude)
  if (!id || !Number.isSafeInteger(year) || !Number.isFinite(latitude) ||
      !Number.isFinite(longitude) || latitude < 49 || latitude > 61 ||
      longitude < -8.5 || longitude > 2.5) return null
  const point: DftCountPoint = {
    ref: (row.road_name ?? '').replace(/\s+/g, ''),
    latitude,
    longitude,
    roadCategory: row.road_category ?? '',
    light: nonNegativeInteger(row, 'cars_and_taxis') + nonNegativeInteger(row, 'LGVs'),
    medium: nonNegativeInteger(row, 'buses_and_coaches'),
    heavy: nonNegativeInteger(row, 'all_HGVs'),
    moto: nonNegativeInteger(row, 'two_wheeled_motor_vehicles'),
    total: nonNegativeInteger(row, 'all_motor_vehicles'),
    year,
  }
  // DfT independently rounds the four class AADFs and published total, so a
  // four-class sum may exceed total by at most two; larger excess is invalid.
  return disjointVehicleClassCountsFitPublishedTotal(
    point.total, [point.light, point.medium, point.heavy, point.moto], 'independently-rounded',
  ) ? point : null
}

/** Keep the latest recent row per physical count point and require a class split. */
export function parseDftCsv(csv: string): DftCountPoint[] {
  const rows = parse(csv, {
    bom: true,
    columns: true,
    skip_empty_lines: true,
  }) as CsvRow[]
  const latest = new Map<string, DftCountPoint>()
  for (const row of rows) {
    const point = countPoint(row)
    if (!point) continue
    const id = row.count_point_id.trim()
    const existing = latest.get(id)
    if (!existing || point.year > existing.year) latest.set(id, point)
  }
  const positive = [...latest.values()].filter(point => point.total > 0)
  if (positive.length === 0) return []
  const newestYear = Math.max(...positive.map(point => point.year))
  return positive.filter(point => point.year > newestYear - 10 &&
    point.light + point.medium + point.heavy + point.moto > 0)
}

function validateCachedPoints(value: unknown, path: string): DftCountPoint[] {
  if (!Array.isArray(value)) throw new Error(`DfT cache is not an array: ${path}`)
  return value.map((entry, index) => {
    if (!entry || typeof entry !== 'object') throw new Error(`invalid DfT cache row ${index}: ${path}`)
    const old = entry as Record<string, unknown>
    const point: DftCountPoint = {
      ref: String(old.ref ?? ''),
      latitude: Number(old.latitude ?? old.lat),
      longitude: Number(old.longitude ?? old.lon),
      roadCategory: String(old.roadCategory ?? old.road_category ?? ''),
      light: Number(old.light ?? old.aadt_light),
      medium: Number(old.medium ?? old.aadt_medium),
      heavy: Number(old.heavy ?? old.aadt_heavy),
      moto: Number(old.moto ?? old.aadt_moto),
      total: Number(old.total),
      year: Number(old.year),
    }
    const counts = [point.light, point.medium, point.heavy, point.moto, point.total]
    if (!point.ref || !Number.isFinite(point.latitude) || !Number.isFinite(point.longitude) ||
        !Number.isSafeInteger(point.year) || counts.some(count => !Number.isSafeInteger(count) || count < 0)) {
      throw new Error(`invalid DfT cache row ${index}: ${path}`)
    }
    return point
  }).filter(point => point.light + point.medium + point.heavy + point.moto > 0)
}

function isCanonicalCache(value: unknown): value is DftCountPoint[] {
  return Array.isArray(value) && (value.length === 0 ||
    (value[0] !== null && typeof value[0] === 'object' && 'latitude' in value[0] && 'light' in value[0]))
}

async function loadDftPoints(options: RoadLoaderArguments): Promise<DftCountPoint[]> {
  const directory = resolve(options.enrichmentDirectory, CACHE_DIRECTORY)
  const jsonPath = resolve(directory, CACHE_JSON)
  const csvPath = resolve(directory, EXTRACTED_CSV)
  if (!options.forceDownload && existsSync(jsonPath)) {
    const cached = JSON.parse(readFileSync(jsonPath, 'utf8')) as unknown
    if (isCanonicalCache(cached)) return validateCachedPoints(cached, jsonPath)
    if (!Array.isArray(cached)) throw new Error(`DfT cache is not an array: ${jsonPath}`)
    if (!existsSync(csvPath)) {
      throw new Error(`legacy DfT cache requires its raw CSV for a lossless rebuild: ${csvPath}`)
    }
    const rebuilt = parseDftCsv(readFileSync(csvPath, 'utf8'))
    if (!options.enrichOnly) writeCacheAtomically(jsonPath, JSON.stringify(rebuilt))
    return rebuilt
  }
  if (options.enrichOnly) {
    if (existsSync(csvPath)) return parseDftCsv(readFileSync(csvPath, 'utf8'))
    throw new Error(`DfT cache missing: ${jsonPath}`)
  }

  const zipPath = resolve(directory, CACHE_ZIP)
  if (options.forceDownload || !existsSync(zipPath)) {
    const response = await fetch(DFT_URL, { signal: AbortSignal.timeout(120_000) })
    if (!response.ok) throw new Error(`DfT download returned HTTP ${response.status}`)
    writeCacheAtomically(zipPath, Buffer.from(await response.arrayBuffer()))
  }
  if (options.forceDownload || !existsSync(csvPath)) {
    mkdirSync(directory, { recursive: true })
    execFileSync('unzip', ['-o', zipPath, '-d', directory], { stdio: 'pipe' })
  }
  const points = parseDftCsv(readFileSync(csvPath, 'utf8'))
  writeCacheAtomically(jsonPath, JSON.stringify(points))
  return points
}

function pointIndex(points: readonly DftCountPoint[]): ReadonlyMap<string, readonly DftCountPoint[]> {
  const index = new Map<string, DftCountPoint[]>()
  for (const point of points) {
    if (!point.ref) continue
    const bucket = index.get(point.ref)
    if (bucket) bucket.push(point)
    else index.set(point.ref, [point])
  }
  return index
}

export function matchDftPoint(
  row: RoadRow,
  pointsByRef: ReadonlyMap<string, readonly DftCountPoint[]>,
): DftCountPoint | null {
  const ref = row.ref?.replace(/\s+/g, '') ?? ''
  const candidates = pointsByRef.get(ref)
  if (!candidates) return null
  let closest: DftCountPoint | null = null
  let closestDistance = 15_000
  for (const candidate of candidates) {
    const distance = haversineM(row.midLat, row.midLon, candidate.latitude, candidate.longitude)
    if (distance < closestDistance) {
      closest = candidate
      closestDistance = distance
    }
  }
  return closest
}

export async function enrichGreatBritainRoads(
  preparedDirectory: string,
  points: readonly DftCountPoint[],
): Promise<GbEnrichmentResult> {
  const squares = listPreparedSquares(preparedDirectory, GREAT_BRITAIN_BBOX)
  if (squares.length === 0) throw new Error(`no Great Britain roads.arrow squares found under ${preparedDirectory}`)
  const pointsByRef = pointIndex(points)
  const result: GbEnrichmentResult = {
    rows: 0, matched: 0, retracted: 0, skippedForeign: 0,
    squares: squares.length, squaresUpdated: 0,
  }
  for (const square of squares) {
    const write = await writeRoadAadt(
      resolve(preparedDirectory, square, 'roads.arrow'),
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, SOURCE_ID)) return null
        const point = matchDftPoint(row, pointsByRef)
        return point ? {
          light: point.light, medium: point.medium, heavy: point.heavy,
          moto: point.moto, sourceId: SOURCE_ID,
        } : null
      },
      undefined,
      undefined,
      { sourceId: SOURCE_ID, when: row => matchDftPoint(row, pointsByRef) === null },
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
  const options = parseRoadLoaderArguments(process.argv.slice(2), 'enrich-roads-gb.ts')
  const points = await loadDftPoints(options)
  const result = await enrichGreatBritainRoads(options.preparedDirectory, points)
  console.log(JSON.stringify({ points: points.length, ...result }))
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : error)
    process.exitCode = 1
  })
}
