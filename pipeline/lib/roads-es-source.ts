/** MITMA 2022 state-road census download and source-faithful parsing. */

import { existsSync, readFileSync } from 'node:fs'
import { createHash } from 'node:crypto'
import { resolve } from 'node:path'
import { writeCacheAtomically } from './atomic-cache.js'
import type { RoadLoaderArguments } from './road-loader-cli.js'
import { inBbox } from './spatial.js'

const CACHE_DIRECTORY = 'es'
const SOURCE_FILE = 'mitma-tramos-2022.js'
const SOURCE_URL = 'https://mapatrafico.transportes.gob.es/2022/Visor/datos/GIS/tramos.js'
const SOURCE_SHA256 = 'a2703db803ba36d4d233a94752ee1f30fc62927b71c804b15307cbe179f823da'
export const SPAIN_ROAD_SOURCE_BBOX = [27, -19, 44, 5] as const
const MOTORCYCLE_SHARE_OF_TOTAL = 0.01
const MEDIUM_SHARE_OF_PUBLISHED_HEAVY = 0.05

export type GeoJsonCoordinate = readonly [longitude: number, latitude: number]

export interface MitmaRoadSection {
  featureId: string
  province: string
  via: string
  ref: string
  lines: ReadonlyArray<ReadonlyArray<GeoJsonCoordinate>>
  pkStart: number | null
  pkEnd: number | null
  lengthKm: number | null
  roadType: string
  imdTotal: number
  imdLight: number
  imdHeavy: number
  aadt_light: number
  aadt_medium: number
  aadt_heavy: number
  aadt_moto: number
}

export interface MitmaRoadCensus {
  sections: MitmaRoadSection[]
  sourceRows: number
  accepted: number
  missingTrafficSkipped: number
  invalidTrafficSkipped: number
  invalidMetadataSkipped: number
  invalidGeometrySkipped: number
  outsideCoverageSkipped: number
  multipartSections: number
}

type UnknownRecord = Record<string, unknown>
type SourceInteger = number | 'missing' | 'invalid'

const isRecord = (value: unknown): value is UnknownRecord =>
  typeof value === 'object' && value !== null && !Array.isArray(value)

function finiteNumber(value: unknown): number | null {
  if (typeof value !== 'number' && typeof value !== 'string') return null
  const text = String(value).trim()
  if (!text) return null
  const parsed = Number(text.replace(',', '.'))
  return Number.isFinite(parsed) ? parsed : null
}

function sourceInteger(value: unknown): SourceInteger {
  if (value === null || value === undefined) return 'missing'
  const text = String(value).trim()
  if (!text || text.toUpperCase() === 'NULL') return 'missing'
  if (!/^-?\d+$/.test(text)) return 'invalid'
  const parsed = Number(text)
  return Number.isSafeInteger(parsed) ? parsed : 'invalid'
}

/** Normalize the MITMA and OSM forms `AP-7`, `A 5` and first multi-ref. */
export function normalizeSpanishRoadRef(value: string): string {
  return value.trim().replace(/[-\s]/g, '').toUpperCase().split(';')[0]
}

function featureCollection(raw: string): unknown[] {
  const objectStart = raw.indexOf('{')
  if (objectStart < 0 || !/^\s*var\s+tramos_data\s*=\s*$/.test(raw.slice(0, objectStart))) {
    throw new Error("MITMA source must be a 'var tramos_data = {...}' assignment")
  }
  let json = raw.slice(objectStart).trim()
  if (json.endsWith(';')) json = json.slice(0, -1).trimEnd()
  const parsed = JSON.parse(json) as unknown
  if (!isRecord(parsed) || parsed.type !== 'FeatureCollection' || !Array.isArray(parsed.features)) {
    throw new Error('MITMA source must contain a GeoJSON FeatureCollection')
  }
  if (!Number.isSafeInteger(parsed.totalFeatures) || parsed.totalFeatures !== parsed.features.length) {
    throw new Error(`MITMA totalFeatures does not match ${parsed.features.length} features`)
  }
  return parsed.features
}

function geometryLines(value: unknown): GeoJsonCoordinate[][] | null {
  if (!isRecord(value) || value.type !== 'MultiLineString' || !Array.isArray(value.coordinates) ||
      value.coordinates.length === 0) return null
  const lines: GeoJsonCoordinate[][] = []
  for (const rawLine of value.coordinates) {
    if (!Array.isArray(rawLine) || rawLine.length < 2) return null
    const line: GeoJsonCoordinate[] = []
    for (const rawCoordinate of rawLine) {
      if (!Array.isArray(rawCoordinate) || rawCoordinate.length < 2 ||
          typeof rawCoordinate[0] !== 'number' || typeof rawCoordinate[1] !== 'number' ||
          !Number.isFinite(rawCoordinate[0]) || !Number.isFinite(rawCoordinate[1])) return null
      const longitude = rawCoordinate[0] as number
      const latitude = rawCoordinate[1] as number
      if (longitude < -180 || longitude > 180 || latitude < -90 || latitude > 90) return null
      line.push([longitude, latitude])
    }
    lines.push(line)
  }
  return lines
}

/** Parse the raw official JS wrapper; derived JSON caches are intentionally not another truth. */
export function parseMitmaRoadSource(raw: string): MitmaRoadCensus {
  const features = featureCollection(raw)
  const census: MitmaRoadCensus = {
    sections: [], sourceRows: features.length, accepted: 0,
    missingTrafficSkipped: 0, invalidTrafficSkipped: 0,
    invalidMetadataSkipped: 0, invalidGeometrySkipped: 0,
    outsideCoverageSkipped: 0, multipartSections: 0,
  }

  for (let index = 0; index < features.length; index++) {
    const feature = features[index]
    const properties = isRecord(feature) && isRecord(feature.properties) ? feature.properties : null
    const via = typeof properties?.via === 'string' ? properties.via.trim() : ''
    const ref = normalizeSpanishRoadRef(via)
    if (!properties || !via || !/^[A-Z0-9]+$/.test(ref)) {
      census.invalidMetadataSkipped++
      continue
    }

    const total = sourceInteger(properties.imdtot)
    const publishedLight = sourceInteger(properties.imdlig)
    const publishedHeavy = sourceInteger(properties.imdpes)
    if (total === 'missing' || publishedLight === 'missing' || publishedHeavy === 'missing') {
      census.missingTrafficSkipped++
      continue
    }
    if (total === 'invalid' || publishedLight === 'invalid' || publishedHeavy === 'invalid' ||
        total <= 0 || publishedLight < 0 || publishedHeavy < 0 ||
        publishedLight + publishedHeavy !== total) {
      census.invalidTrafficSkipped++
      continue
    }

    const lines = geometryLines(isRecord(feature) ? feature.geometry : null)
    if (!lines) {
      census.invalidGeometrySkipped++
      continue
    }
    if (lines.some(line => line.some(([longitude, latitude]) =>
      !inBbox(latitude, longitude, SPAIN_ROAD_SOURCE_BBOX)))) {
      census.outsideCoverageSkipped++
      continue
    }

    const aadt_moto = Math.round(total * MOTORCYCLE_SHARE_OF_TOTAL)
    const aadt_medium = Math.round(publishedHeavy * MEDIUM_SHARE_OF_PUBLISHED_HEAVY)
    const aadt_heavy = publishedHeavy - aadt_medium
    const aadt_light = publishedLight - aadt_moto
    if (aadt_light < 0) {
      census.invalidTrafficSkipped++
      continue
    }

    census.sections.push({
      featureId: isRecord(feature) ? String(feature.id ?? index) : String(index),
      province: typeof properties.provincia === 'string' ? properties.provincia : '',
      via, ref, lines,
      pkStart: finiteNumber(properties.pkinicio_t),
      pkEnd: finiteNumber(properties.pkfin_t),
      lengthKm: finiteNumber(properties.longitud),
      roadType: typeof properties.clase === 'string' ? properties.clase : '',
      imdTotal: total, imdLight: publishedLight, imdHeavy: publishedHeavy,
      aadt_light, aadt_medium, aadt_heavy, aadt_moto,
    })
    census.accepted++
    if (lines.length > 1) census.multipartSections++
  }
  return census
}

function parseCanonicalSource(bytes: Buffer): MitmaRoadCensus {
  const digest = createHash('sha256').update(bytes).digest('hex')
  if (digest !== SOURCE_SHA256) {
    throw new Error(`MITMA source SHA-256 ${digest} does not match immutable 2022 census`)
  }
  return parseMitmaRoadSource(bytes.toString('utf8'))
}

async function download(): Promise<Buffer> {
  const response = await fetch(SOURCE_URL, { signal: AbortSignal.timeout(120_000) })
  if (!response.ok) throw new Error(`MITMA download returned HTTP ${response.status}`)
  return Buffer.from(await response.arrayBuffer())
}

/** Load the immutable official source; parsing it directly avoids a stale derived cache. */
export async function loadMitmaRoadCensus(options: RoadLoaderArguments): Promise<MitmaRoadCensus> {
  const path = resolve(options.enrichmentDirectory, CACHE_DIRECTORY, SOURCE_FILE)
  if (options.forceDownload || !existsSync(path)) {
    if (options.enrichOnly) throw new Error(`MITMA road source missing: ${path}`)
    const bytes = await download()
    const census = parseCanonicalSource(bytes)
    writeCacheAtomically(path, bytes)
    return census
  }
  return parseCanonicalSource(readFileSync(path))
}
