/** Immutable Amsterdam 2025 AADT source loading and source-faithful parsing. */

import { createHash } from 'node:crypto'
import { existsSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { writeCacheAtomically } from './atomic-cache.js'
import type { RoadLoaderArguments } from './road-loader-cli.js'

const CACHE_DIRECTORY = 'global/eu-city-traffic'
const SOURCE_FILE = 'Amsterdam_AADT_2025.geojson'
const SOURCE_URL = 'https://raw.githubusercontent.com/XavB64/traffic-volume-data-EU-cities/main/Netherlands/Amsterdam/treated/Amsterdam_AADT_2025.geojson'
const SOURCE_SHA256 = 'f4bab37574a2bb7cc4fab74f0f822cc32725c8fd6619897ec01b6cf4e1864df5'
const MEDIUM_SHARE_OF_TOTAL = 0.02

export interface AmsterdamTrafficRecord {
  sourceRow: number
  latitude: number
  longitude: number
  aadtTotal: number
  aadt_light: number
  aadt_medium: number
  aadt_heavy: number
  aadt_moto: number
}

export interface AmsterdamTrafficCensus {
  records: AmsterdamTrafficRecord[]
  sourceRows: number
  accepted: number
  invalidMetadataSkipped: number
  invalidTrafficSkipped: number
  invalidGeometrySkipped: number
}

type UnknownRecord = Record<string, unknown>

const isRecord = (value: unknown): value is UnknownRecord =>
  typeof value === 'object' && value !== null && !Array.isArray(value)

function featuresFromGeoJson(raw: string): unknown[] {
  const parsed = JSON.parse(raw) as unknown
  if (!isRecord(parsed) || parsed.type !== 'FeatureCollection' || !Array.isArray(parsed.features)) {
    throw new Error('Amsterdam source must be a GeoJSON FeatureCollection')
  }
  return parsed.features
}

function representativeCoordinate(geometry: unknown): readonly [number, number] | null {
  if (!isRecord(geometry) || geometry.type !== 'LineString' ||
      !Array.isArray(geometry.coordinates) || geometry.coordinates.length < 2) return null
  const coordinates: Array<readonly [number, number]> = []
  for (const coordinate of geometry.coordinates) {
    if (!Array.isArray(coordinate) || coordinate.length < 2 ||
        typeof coordinate[0] !== 'number' || typeof coordinate[1] !== 'number' ||
        !Number.isFinite(coordinate[0]) || !Number.isFinite(coordinate[1]) ||
        coordinate[0] < -180 || coordinate[0] > 180 ||
        coordinate[1] < -90 || coordinate[1] > 90) return null
    coordinates.push([coordinate[0], coordinate[1]])
  }
  return coordinates[Math.floor(coordinates.length / 2)]
}

/** Parse the treated source's actual AADT-only contract without inventing classes. */
export function parseAmsterdamTrafficSource(raw: string): AmsterdamTrafficCensus {
  const features = featuresFromGeoJson(raw)
  const census: AmsterdamTrafficCensus = {
    records: [], sourceRows: features.length, accepted: 0,
    invalidMetadataSkipped: 0, invalidTrafficSkipped: 0, invalidGeometrySkipped: 0,
  }
  for (let sourceRow = 0; sourceRow < features.length; sourceRow++) {
    const feature = features[sourceRow]
    const object = isRecord(feature) ? feature : null
    const properties = object?.type === 'Feature' &&
      isRecord(object.properties) ? object.properties : null
    const geometry = object?.geometry
    if (!properties) {
      census.invalidMetadataSkipped++
      continue
    }
    const total = properties.AADT
    if (!Number.isSafeInteger(total) || (total as number) < 2 || (total as number) > 200_000) {
      census.invalidTrafficSkipped++
      continue
    }
    const coordinate = representativeCoordinate(geometry)
    if (!coordinate) {
      census.invalidGeometrySkipped++
      continue
    }

    const aadtTotal = total as number
    const aadt_medium = Math.round(aadtTotal * MEDIUM_SHARE_OF_TOTAL)
    // Dev1 rounded both complements independently, making 99 real rows sum to AADT+1.
    const aadt_light = aadtTotal - aadt_medium
    census.records.push({
      sourceRow,
      longitude: coordinate[0],
      latitude: coordinate[1],
      aadtTotal,
      aadt_light,
      aadt_medium,
      aadt_heavy: 0,
      aadt_moto: 0,
    })
    census.accepted++
  }
  return census
}

function parseCanonicalSource(bytes: Buffer): AmsterdamTrafficCensus {
  const digest = createHash('sha256').update(bytes).digest('hex')
  if (digest !== SOURCE_SHA256) {
    throw new Error(`Amsterdam source SHA-256 ${digest} does not match immutable 2025 census`)
  }
  const census = parseAmsterdamTrafficSource(bytes.toString('utf8'))
  if (census.sourceRows === 0 || census.accepted !== census.sourceRows) {
    throw new Error(`Amsterdam 2025 source incomplete: ${JSON.stringify({
      sourceRows: census.sourceRows,
      accepted: census.accepted,
      invalidMetadataSkipped: census.invalidMetadataSkipped,
      invalidTrafficSkipped: census.invalidTrafficSkipped,
      invalidGeometrySkipped: census.invalidGeometrySkipped,
    })}`)
  }
  return census
}

async function download(): Promise<Buffer> {
  const response = await fetch(SOURCE_URL, { signal: AbortSignal.timeout(120_000) })
  if (!response.ok) throw new Error(`Amsterdam traffic download returned HTTP ${response.status}`)
  return Buffer.from(await response.arrayBuffer())
}

/** Load one pinned treated artifact; the raw official periods do not sum to AADT. */
export async function loadAmsterdamTrafficCensus(
  options: RoadLoaderArguments,
): Promise<AmsterdamTrafficCensus> {
  const path = resolve(options.enrichmentDirectory, CACHE_DIRECTORY, SOURCE_FILE)
  if (options.forceDownload || !existsSync(path)) {
    if (options.enrichOnly) throw new Error(`Amsterdam road source missing: ${path}`)
    const bytes = await download()
    const census = parseCanonicalSource(bytes)
    writeCacheAtomically(path, bytes)
    return census
  }
  return parseCanonicalSource(readFileSync(path))
}
