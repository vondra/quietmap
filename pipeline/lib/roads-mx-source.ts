/** Parse admitted SICT/IMT Datos Viales 2025 U1 traffic and composition. */

import { parse } from 'csv-parse/sync'
import type { RoadLoaderArguments } from './road-loader-cli.js'
import { readPinnedRoadSource } from './pinned-road-source.js'

const SEGMENTS_PATH = 'mx/sict-datosviales/u1_segmentos.geojson'
const SEGMENTS_SHA256 = '269ad4f01f09e7483043ce66d0c835d14d6526a59f77d32db27077231e61d7a5'
const COMPOSITION_PATH = 'mx/sict-datosviales/u1_segmentos.csv'
const COMPOSITION_SHA256 = '4ce7ac4f8729ddb6f8bbbba88fddc85fa2090632e46722b5e37b0e10a207452e'
const NATIONAL_SPLIT = { light: 0.795, medium: 0.071, heavy: 0.082, moto: 0.052 }

export interface MexicanSictSegment {
  sourceRow: number
  lines: ReadonlyArray<ReadonlyArray<readonly [number, number]>>
  total: number
  fractions: typeof NATIONAL_SPLIT
  allowedRoadClassMask: number
}

export interface MexicanSictSource {
  segments: MexicanSictSegment[]
  sourceRows: number
  compositionRows: number
  invalidGeometrySkipped: number
  unavailableTrafficSkipped: number
  missingCompositionRows: number
  fallbackCompositionRows: number
}

type UnknownRecord = Record<string, unknown>
const isRecord = (value: unknown): value is UnknownRecord =>
  typeof value === 'object' && value !== null && !Array.isArray(value)

export function mexicanAllowedRoadClassMask(red: string, operation: string): number {
  if (red === 'Federal' && operation === 'Cuota') return (1 << 0) | (1 << 1)
  if (red === 'Federal') return (1 << 1) | (1 << 2)
  return (1 << 2) | (1 << 3)
}

function number(value: unknown): number {
  const parsed = typeof value === 'number' ? value : Number(String(value ?? '').trim())
  return Number.isFinite(parsed) ? parsed : 0
}

function composition(row: UnknownRecord): typeof NATIONAL_SPLIT | null {
  const light = number(row.comp_A) + number(row.comp_OTROS)
  const medium = number(row.comp_B) + number(row.comp_C2)
  const heavy = number(row.comp_C3) + number(row.comp_T3S2) + number(row.comp_T3S3) +
    number(row.comp_T3S2R4)
  const moto = number(row.comp_M)
  const sum = light + medium + heavy + moto
  if (sum <= 0) return null
  if ([light, medium, heavy, moto].some(value => value < 0)) return null
  return { light: light / sum, medium: medium / sum, heavy: heavy / sum, moto: moto / sum }
}

function line(pointValues: unknown): ReadonlyArray<readonly [number, number]> | null {
  if (!Array.isArray(pointValues) || pointValues.length < 2) return null
  const points: Array<readonly [number, number]> = []
  for (const point of pointValues) {
    if (!Array.isArray(point) || typeof point[0] !== 'number' || typeof point[1] !== 'number' ||
        !Number.isFinite(point[0]) || !Number.isFinite(point[1]) ||
        point[0] < -180 || point[0] > 180 || point[1] < -90 || point[1] > 90) return null
    points.push([point[0], point[1]])
  }
  return points
}

function lines(geometry: unknown): MexicanSictSegment['lines'] | null {
  if (!isRecord(geometry) || !Array.isArray(geometry.coordinates)) return null
  const rawLines = geometry.type === 'LineString' ? [geometry.coordinates] :
    geometry.type === 'MultiLineString' ? geometry.coordinates : null
  if (!rawLines) return null
  const parsed = rawLines.map(line)
  return parsed.length && parsed.every(value => value !== null) ?
    parsed as Array<ReadonlyArray<readonly [number, number]>> : null
}

export function splitMexicanTdpa(total: number, fractions: typeof NATIONAL_SPLIT) {
  const medium = Math.round(total * fractions.medium)
  const heavy = Math.round(total * fractions.heavy)
  const moto = Math.round(total * fractions.moto)
  return { light: total - medium - heavy - moto, medium, heavy, moto }
}

export function parseMexicanSictSource(segmentsRaw: string, compositionRaw: string): MexicanSictSource {
  const rows = parse(compositionRaw, { columns: true, skip_empty_lines: true, bom: true }) as UnknownRecord[]
  const compositionById = new Map<string, typeof NATIONAL_SPLIT | null>()
  for (const row of rows) {
    if (typeof row.segment_mongo_id === 'string' && row.segment_mongo_id) {
      compositionById.set(row.segment_mongo_id, composition(row))
    }
  }
  const parsed = JSON.parse(segmentsRaw) as unknown
  if (!isRecord(parsed) || parsed.type !== 'FeatureCollection' || !Array.isArray(parsed.features)) {
    throw new Error('Mexican SICT source must be a GeoJSON FeatureCollection')
  }
  const result: MexicanSictSource = { segments: [], sourceRows: parsed.features.length,
    compositionRows: rows.length, invalidGeometrySkipped: 0, unavailableTrafficSkipped: 0,
    missingCompositionRows: 0, fallbackCompositionRows: 0 }
  for (let sourceRow = 0; sourceRow < parsed.features.length; sourceRow++) {
    const feature = parsed.features[sourceRow]
    const properties = isRecord(feature) && isRecord(feature.properties) ? feature.properties : null
    const geometryLines = lines(isRecord(feature) ? feature.geometry : null)
    if (!geometryLines) {
      result.invalidGeometrySkipped++
      continue
    }
    const totalValue = number(properties?.tdpa_2024)
    if (totalValue <= 0 || !Number.isSafeInteger(Math.round(totalValue))) {
      result.unavailableTrafficSkipped++
      continue
    }
    const id = typeof properties?.segment_mongo_id === 'string' ? properties.segment_mongo_id : ''
    if (!compositionById.has(id)) result.missingCompositionRows++
    const fractions = compositionById.get(id) ?? null
    if (!fractions) result.fallbackCompositionRows++
    result.segments.push({ sourceRow, lines: geometryLines, total: Math.round(totalValue),
      fractions: fractions ?? NATIONAL_SPLIT,
      allowedRoadClassMask: mexicanAllowedRoadClassMask(
        String(properties?.red_ok ?? ''), String(properties?.operacion ?? '')) })
  }
  if (result.segments.length === 0) throw new Error('Mexican SICT source has no usable measurements')
  return result
}

export function loadMexicanSictSource(options: RoadLoaderArguments): MexicanSictSource {
  const segments = readPinnedRoadSource(options, SEGMENTS_PATH, SEGMENTS_SHA256).toString('utf8')
  const compositionRows = readPinnedRoadSource(options, COMPOSITION_PATH, COMPOSITION_SHA256).toString('utf8')
  return parseMexicanSictSource(segments, compositionRows)
}
